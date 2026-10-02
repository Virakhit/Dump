use crate::connectivity::{direct_address, public_address, MAX_ADDRESSES};
use libp2p::{
    autonat,
    core::{transport::PortUse, Endpoint},
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, NewExternalAddrCandidate,
        THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    task::{Context, Poll},
    time::{Duration, Instant},
};

const VALID_FOR: Duration = Duration::from_secs(10 * 60);
const RECHECK_AFTER: Duration = Duration::from_secs(5 * 60);

struct Candidate {
    observed: Instant,
    recheck: Instant,
}

/// AutoNAT's candidate limit bounds each probe batch, not its address cache.
/// Filter and deduplicate before the behaviour sees any untrusted observations.
pub(crate) struct Client {
    inner: autonat::v2::client::Behaviour,
    peer: PeerId,
    candidates: BTreeMap<Multiaddr, Candidate>,
    connections: BTreeSet<ConnectionId>,
    listeners: BTreeSet<Multiaddr>,
    #[cfg(test)]
    allow_loopback: bool,
}

impl Client {
    pub fn new(peer: PeerId) -> Self {
        Self {
            inner: Default::default(),
            peer,
            candidates: BTreeMap::new(),
            connections: BTreeSet::new(),
            listeners: BTreeSet::new(),
            #[cfg(test)]
            allow_loopback: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_probe(&mut self, address: Option<&Multiaddr>, revalidate: bool) {
        if let Some(address) = address {
            self.allow_loopback = true;
            self.on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: address },
            ));
        }
        if revalidate {
            self.revalidate(Instant::now() + RECHECK_AFTER);
        }
    }

    fn candidate(&mut self, address: &Multiaddr) -> Option<Multiaddr> {
        self.candidate_at(address, Instant::now())
    }

    fn candidate_at(&mut self, address: &Multiaddr, at: Instant) -> Option<Multiaddr> {
        let address = direct_address(self.peer, address.clone()).ok()?;
        let allowed = public_address(&address);
        #[cfg(test)]
        let allowed = allowed || self.allow_loopback;
        if !allowed {
            return None;
        }
        self.revalidate(at);
        if let Some(candidate) = self.candidates.get_mut(&address) {
            candidate.observed = at;
            return None;
        }
        if self.candidates.len() >= MAX_ADDRESSES {
            return None;
        }
        self.candidates.insert(
            address.clone(),
            Candidate {
                observed: at,
                recheck: at + RECHECK_AFTER,
            },
        );
        Some(address)
    }

    fn revalidate(&mut self, at: Instant) {
        self.candidates.retain(|address, candidate| {
            if self.listeners.contains(address) {
                candidate.observed = at;
            }
            if at.saturating_duration_since(candidate.observed) >= VALID_FOR
                && self.inner.remove_candidate(address)
            {
                return false;
            }
            if at >= candidate.recheck && self.inner.retry_candidate(address) {
                candidate.recheck = at + RECHECK_AFTER;
            }
            true
        });
    }
}

impl NetworkBehaviour for Client {
    type ConnectionHandler =
        <autonat::v2::client::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = autonat::v2::client::Event;

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner
            .handle_established_inbound_connection(id, peer, local, remote)
    }

    fn handle_established_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        address: &Multiaddr,
        role: Endpoint,
        port: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner
            .handle_established_outbound_connection(id, peer, address, role, port)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::NewListenAddr(e) => {
                if let Ok(address) = direct_address(self.peer, e.addr.clone()) {
                    if public_address(&address) && self.listeners.len() < MAX_ADDRESSES {
                        self.listeners.insert(address);
                    }
                }
            }
            FromSwarm::ExpiredListenAddr(e) => {
                self.listeners.remove(e.addr);
                if self.inner.remove_candidate(e.addr) {
                    self.candidates.remove(e.addr);
                } else if let Some(candidate) = self.candidates.get_mut(e.addr) {
                    candidate.observed = Instant::now() - VALID_FOR;
                }
            }
            FromSwarm::NewExternalAddrCandidate(NewExternalAddrCandidate { addr }) => {
                if let Some(address) = self.candidate(addr) {
                    self.inner
                        .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                            NewExternalAddrCandidate { addr: &address },
                        ));
                }
                return;
            }
            FromSwarm::ConnectionEstablished(e) => {
                self.connections.insert(e.connection_id);
            }
            FromSwarm::ConnectionClosed(e) => {
                self.connections.remove(&e.connection_id);
            }
            _ => {}
        }
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer: PeerId,
        id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner.on_connection_handler_event(peer, id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        let at = Instant::now();
        // The existing AutoNAT probe timer wakes this poll even when no peer disconnects.
        self.revalidate(at);
        let event = self.inner.poll(cx);
        if let Poll::Ready(ToSwarm::GenerateEvent(event)) = &event {
            if let Some(candidate) = self.candidates.get_mut(&event.tested_addr) {
                candidate.recheck = at + RECHECK_AFTER;
                if event.result.is_ok() {
                    candidate.observed = at;
                }
            }
        }
        event
    }
}

/// A successful dial-back is time-limited evidence, never a promise about future NAT behavior.
#[derive(Default)]
pub(crate) struct PublicAddresses(BTreeMap<Multiaddr, (PeerId, Instant)>);

impl PublicAddresses {
    pub fn remove(&mut self, address: &Multiaddr) -> bool {
        self.0.remove(address).is_some()
    }

    pub fn record(&mut self, peer: PeerId, address: Multiaddr, verified: bool, at: Instant) {
        if !verified {
            self.0.remove(&address);
        } else if public_address(&address)
            && (self.0.len() < MAX_ADDRESSES || self.0.contains_key(&address))
        {
            self.0.insert(address, (peer, at));
        }
    }

    pub fn expire(&mut self, disconnected: Option<PeerId>, at: Instant) -> Vec<Multiaddr> {
        let expired: Vec<_> = self
            .0
            .iter()
            .filter(|(_, (peer, checked))| {
                disconnected == Some(*peer) || at.saturating_duration_since(*checked) >= VALID_FOR
            })
            .map(|(address, _)| address.clone())
            .collect();
        for address in &expired {
            self.0.remove(address);
        }
        expired
    }

    pub fn addresses(&self) -> Vec<Multiaddr> {
        self.0.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use futures::StreamExt;
    use libp2p::{identify, swarm::SwarmEvent, SwarmBuilder};

    #[derive(NetworkBehaviour)]
    struct ProbeClient {
        nat: Client,
        identify: identify::Behaviour,
        streams: libp2p_stream::Behaviour,
    }

    #[derive(NetworkBehaviour)]
    struct ProbeServer {
        nat: autonat::v2::server::Behaviour,
        identify: identify::Behaviour,
        streams: libp2p_stream::Behaviour,
    }

    fn identify(key: &libp2p::identity::Keypair) -> identify::Behaviour {
        identify::Behaviour::new(
            identify::Config::new("/dump/test".into(), key.public())
                .with_hide_listen_addrs(true)
                .with_cache_size(0),
        )
    }

    #[test]
    fn candidates_and_advertisements_require_valid_public_evidence() -> Result<()> {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let mut client = Client::new(peer);
        for address in [
            "/ip4/10.0.0.1/udp/1000/quic-v1",
            "/ip4/100.64.0.1/udp/1000/quic-v1",
            "/ip4/8.8.8.8/tcp/0",
        ] {
            assert!(client.candidate(&address.parse()?).is_none());
        }
        let address: Multiaddr = "/ip4/8.8.8.8/udp/1000/quic-v1".parse()?;
        assert!(client.candidate(&address).is_some());
        assert!(client.candidate(&address).is_none());
        for port in 1..MAX_ADDRESSES {
            assert!(client
                .candidate(&format!("/ip4/8.8.8.8/udp/{port}/quic-v1").parse()?)
                .is_some());
        }
        assert!(client
            .candidate(&"/ip4/8.8.8.8/udp/9000/quic-v1".parse()?)
            .is_none());
        let mut public = PublicAddresses::default();
        let at = Instant::now();
        public.record(peer, address.clone(), false, at);
        assert!(public.addresses().is_empty());
        public.record(peer, "/ip4/127.0.0.1/udp/1000/quic-v1".parse()?, true, at);
        assert!(public.addresses().is_empty());
        public.record(peer, address.clone(), true, at);
        assert_eq!(public.addresses(), vec![address.clone()]);
        assert!(public
            .expire(None, at + VALID_FOR - Duration::from_secs(1))
            .is_empty());
        assert_eq!(public.expire(None, at + VALID_FOR), vec![address.clone()]);
        public.record(peer, address.clone(), true, at);
        assert_eq!(public.expire(Some(peer), at), vec![address]);
        Ok(())
    }

    #[test]
    fn long_lived_candidates_recheck_and_expire_with_bounded_address_churn() -> Result<()> {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let mut client = Client::new(peer);
        let address: Multiaddr = "/ip4/8.8.8.8/udp/9000/quic-v1".parse()?;
        let at = Instant::now();
        client.candidate_at(&address, at);
        client
            .inner
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: &address },
            ));
        // A permanently open transport does not gate the re-probe schedule.
        client.connections.insert(ConnectionId::new_unchecked(1));
        client.revalidate(at + RECHECK_AFTER - Duration::from_secs(1));
        assert_eq!(client.candidates[&address].recheck, at + RECHECK_AFTER);
        client.revalidate(at + RECHECK_AFTER);
        assert_eq!(client.candidates[&address].recheck, at + RECHECK_AFTER * 2);
        assert_eq!(client.connections.len(), 1);
        client.revalidate(at + VALID_FOR);
        assert!(client.candidates.is_empty());
        for cycle in 1..=32 {
            let now = at + VALID_FOR * cycle;
            for port in 1..=MAX_ADDRESSES + 1 {
                client.candidate_at(&format!("/ip4/8.8.8.8/udp/{port}/quic-v1").parse()?, now);
            }
            assert_eq!(client.candidates.len(), MAX_ADDRESSES);
        }
        assert_eq!(client.connections.len(), 1);
        // A still-listening public route can be retried when a server appears much later.
        let listener: Multiaddr = "/ip4/8.8.8.8/tcp/4242".parse()?;
        client.revalidate(at + VALID_FOR * 34);
        client.candidate_at(&listener, at + VALID_FOR * 34);
        client
            .inner
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: &listener },
            ));
        client.listeners.insert(listener.clone());
        client.revalidate(at + VALID_FOR * 40);
        assert!(client.candidates.contains_key(&listener));
        client.listeners.remove(&listener);
        client.revalidate(at + VALID_FOR * 41);
        assert!(!client.candidates.contains_key(&listener));
        Ok(())
    }

    #[test]
    fn renewed_evidence_extends_ttl_and_failed_reprobe_withdraws_it() -> Result<()> {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let address: Multiaddr = "/ip4/8.8.8.8/udp/9000/quic-v1".parse()?;
        let at = Instant::now();
        let mut public = PublicAddresses::default();
        public.record(peer, address.clone(), true, at);
        public.record(peer, address.clone(), true, at + RECHECK_AFTER);
        assert!(public.expire(None, at + VALID_FOR).is_empty());
        public.record(peer, address.clone(), false, at + VALID_FOR);
        assert!(public.addresses().is_empty());
        public.record(peer, address.clone(), true, at);
        assert_eq!(public.expire(None, at + VALID_FOR), vec![address]);
        Ok(())
    }

    #[test]
    fn persistent_public_listener_without_probe_server_has_no_confirmation() -> Result<()> {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let mut client = Client::new(peer);
        let address: Multiaddr = "/ip4/8.8.8.8/tcp/4242".parse()?;
        let at = Instant::now();
        client.on_swarm_event(FromSwarm::NewListenAddr(libp2p::swarm::NewListenAddr {
            listener_id: libp2p::core::transport::ListenerId::next(),
            addr: &address,
        }));
        client.on_swarm_event(FromSwarm::NewExternalAddrCandidate(
            NewExternalAddrCandidate { addr: &address },
        ));
        client.revalidate(at + VALID_FOR * 40);
        assert!(client.candidates.contains_key(&address));
        assert!(client.connections.is_empty());
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        // No probe server or inbound nonce can create a confirmed advertisement.
        assert!(client.poll(&mut cx).is_pending());
        assert!(PublicAddresses::default().addresses().is_empty());
        assert_eq!(
            crate::connectivity::Reachability::default().status,
            crate::connectivity::ReachabilityStatus::Unknown
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn real_autonat_v2_dialback_confirms_only_reachable_listener() -> Result<()> {
        let key = libp2p::identity::Keypair::generate_ed25519();
        let peer = key.public().to_peer_id();
        let mut client = SwarmBuilder::with_existing_identity(key)
            .with_tokio()
            .with_quic()
            .with_behaviour(|key| {
                let mut behaviour = Client::new(peer);
                // Only the test permits loopback candidates; shipping behaviour forbids them.
                behaviour.allow_loopback = true;
                ProbeClient {
                    nat: behaviour,
                    identify: identify(key),
                    streams: libp2p_stream::Behaviour::new(),
                }
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();
        let mut server = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_quic()
            .with_behaviour(|key| ProbeServer {
                nat: Default::default(),
                identify: identify(key),
                streams: libp2p_stream::Behaviour::new(),
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();
        client.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        server.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        let (client_address, server_address) = tokio::try_join!(
            async {
                loop {
                    if let SwarmEvent::NewListenAddr { address, .. } =
                        client.select_next_some().await
                    {
                        return Ok::<_, anyhow::Error>(address);
                    }
                }
            },
            async {
                loop {
                    if let SwarmEvent::NewListenAddr { address, .. } =
                        server.select_next_some().await
                    {
                        return Ok::<_, anyhow::Error>(address);
                    }
                }
            }
        )?;
        // This address speaks QUIC, but cannot authenticate as the probing client.
        let unavailable = server_address.clone();
        client
            .behaviour_mut()
            .nat
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate {
                    addr: &client_address,
                },
            ));
        client
            .behaviour_mut()
            .nat
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: &unavailable },
            ));
        client
            .dial(server_address.with(libp2p::multiaddr::Protocol::P2p(*server.local_peer_id())))?;
        tokio::time::timeout(Duration::from_secs(45), async {
            let mut reachable = false;
            let mut unreachable = false;
            loop {
                tokio::select! {
                    event = client.select_next_some() => {
                        if let SwarmEvent::Behaviour(ProbeClientEvent::Nat(event)) = event {
                            if event.tested_addr == client_address { assert!(event.result.is_ok()); reachable = true; }
                            if event.tested_addr == unavailable { assert!(event.result.is_err()); unreachable = true; }
                            if reachable && unreachable { break; }
                        }
                    },
                    _ = server.select_next_some() => {},
                }
            }
        }).await?;
        assert!(client.external_addresses().any(|a| a == &client_address));
        assert!(!client.external_addresses().any(|a| a == &unavailable));

        use futures::{AsyncReadExt, AsyncWriteExt};
        let protocol = libp2p::StreamProtocol::new("/dump/test/revalidation-transfer/1");
        let mut incoming = server
            .behaviour_mut()
            .streams
            .new_control()
            .accept(protocol.clone())?;
        let server_peer = *server.local_peer_id();
        let mut control = client.behaviour_mut().streams.new_control();
        let (started_tx, mut started_rx) = tokio::sync::oneshot::channel();
        let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
        // JoinSet aborts both stream tasks on any assertion, failure or timeout.
        let mut transfers = tokio::task::JoinSet::new();
        transfers.spawn(async move {
            let mut stream = control.open_stream(server_peer, protocol).await?;
            stream.write_all(&vec![42; 32768]).await?;
            stream.flush().await?;
            let _ = started_tx.send(());
            continue_rx.await?;
            stream.write_all(&vec![42; 32768]).await?;
            stream.close().await?;
            Ok::<_, anyhow::Error>(())
        });
        transfers.spawn(async move {
            let (_, mut stream) = incoming
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("Incoming transfer closed"))?;
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await?;
            anyhow::ensure!(
                bytes == vec![42; 65536],
                "Active transfer bytes changed during revalidation"
            );
            Ok::<_, anyhow::Error>(())
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    result = &mut started_rx => { result?; break; },
                    _ = client.select_next_some() => {},
                    _ = server.select_next_some() => {},
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        let established = client.behaviour().nat.connections.clone();
        client
            .behaviour_mut()
            .nat
            .revalidate(Instant::now() + RECHECK_AFTER);
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                tokio::select! {
                    event = client.select_next_some() => {
                        if let SwarmEvent::Behaviour(ProbeClientEvent::Nat(event)) = event {
                            if event.tested_addr == client_address {
                                anyhow::ensure!(event.result.is_ok(), "Re-probe failed: {:?}", event.result);
                                break;
                            }
                        }
                    },
                    _ = server.select_next_some() => {},
                }
            }
            Ok::<_, anyhow::Error>(())
        }).await??;
        assert!(established.is_subset(&client.behaviour().nat.connections));
        continue_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("Active stream stopped during re-probe"))?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    result = transfers.join_next() => match result { Some(result) => result??, None => break },
                    _ = client.select_next_some() => {},
                    _ = server.select_next_some() => {},
                }
            }
            Ok::<_, anyhow::Error>(())
        }).await??;
        Ok(())
    }
}
