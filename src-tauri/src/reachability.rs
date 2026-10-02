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

/// AutoNAT's candidate limit bounds each probe batch, not its address cache.
/// Filter and deduplicate before the behaviour sees any untrusted observations.
pub(crate) struct Client {
    inner: autonat::v2::client::Behaviour,
    peer: PeerId,
    candidates: BTreeSet<Multiaddr>,
    connections: BTreeSet<ConnectionId>,
    #[cfg(test)]
    allow_loopback: bool,
}

impl Client {
    pub fn new(peer: PeerId) -> Self {
        Self {
            inner: Default::default(),
            peer,
            candidates: BTreeSet::new(),
            connections: BTreeSet::new(),
            #[cfg(test)]
            allow_loopback: false,
        }
    }

    fn candidate(&mut self, address: &Multiaddr) -> Option<Multiaddr> {
        let address = direct_address(self.peer, address.clone()).ok()?;
        let allowed = public_address(&address);
        #[cfg(test)]
        let allowed = allowed || self.allow_loopback;
        if !allowed || self.candidates.len() >= MAX_ADDRESSES || self.candidates.contains(&address)
        {
            return None;
        }
        // ponytail: eight distinct candidates per connected lifetime; reset after all
        // connections close. Add upstream cache eviction if real address churn hits this cap.
        self.candidates.insert(address.clone());
        Some(address)
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
        if matches!(event, FromSwarm::ConnectionClosed(_)) && self.connections.is_empty() {
            self.inner = Default::default();
            self.candidates.clear();
        }
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
        self.inner.poll(cx)
    }
}

/// A successful dial-back is time-limited evidence, never a promise about future NAT behavior.
#[derive(Default)]
pub(crate) struct PublicAddresses(BTreeMap<Multiaddr, (PeerId, Instant)>);

impl PublicAddresses {
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
    }

    #[derive(NetworkBehaviour)]
    struct ProbeServer {
        nat: autonat::v2::server::Behaviour,
        identify: identify::Behaviour,
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
            "/ip4/8.8.8.8/tcp/1000",
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
        Ok(())
    }
}
