use crate::connectivity::{direct_address, public_address};
use libp2p::{
    core::{transport::PortUse, Endpoint},
    dcutr,
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, NewExternalAddrCandidate,
        THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    task::{Context, Poll},
};

pub(crate) struct HolePunch {
    inner: dcutr::Behaviour,
    peer: PeerId,
    connections: BTreeSet<ConnectionId>,
    attempts: BTreeSet<ConnectionId>,
    events: BTreeMap<ConnectionId, u8>,
    total_events: usize,
    #[cfg(test)]
    allow_loopback: bool,
}

impl HolePunch {
    pub fn new(peer: PeerId) -> Self {
        Self {
            inner: dcutr::Behaviour::new(peer),
            peer,
            connections: BTreeSet::new(),
            attempts: BTreeSet::new(),
            events: BTreeMap::new(),
            total_events: 0,
            #[cfg(test)]
            allow_loopback: false,
        }
    }

    fn valid(&self, peer: PeerId, address: &Multiaddr) -> bool {
        let Ok(address) = direct_address(peer, address.clone()) else {
            return false;
        };
        let allowed = public_address(&address);
        #[cfg(test)]
        let allowed = allowed || self.allow_loopback;
        allowed
    }
}

impl NetworkBehaviour for HolePunch {
    type ConnectionHandler = <dcutr::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = dcutr::Event;

    fn handle_pending_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: Option<PeerId>,
        addresses: &[Multiaddr],
        _: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        if self.attempts.contains(&id)
            && !peer.is_some_and(|peer| {
                !addresses.is_empty() && addresses.iter().all(|a| self.valid(peer, a))
            })
        {
            return Err(ConnectionDenied::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid hole-punch address",
            )));
        }
        Ok(Vec::new())
    }

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
            FromSwarm::NewExternalAddrCandidate(NewExternalAddrCandidate { addr })
                if !self.valid(self.peer, addr) =>
            {
                return
            }
            FromSwarm::ConnectionEstablished(e) => {
                self.connections.insert(e.connection_id);
            }
            FromSwarm::ConnectionClosed(e) => {
                self.connections.remove(&e.connection_id);
                self.events.remove(&e.connection_id);
                self.attempts.remove(&e.connection_id);
            }
            FromSwarm::DialFailure(e) => {
                self.attempts.remove(&e.connection_id);
            }
            _ => {}
        }
        self.inner.on_swarm_event(event);
        if matches!(event, FromSwarm::ConnectionClosed(_)) && self.connections.is_empty() {
            self.inner = dcutr::Behaviour::new(self.peer);
            self.attempts.clear();
            self.events.clear();
            self.total_events = 0;
        }
    }
    fn on_connection_handler_event(
        &mut self,
        peer: PeerId,
        id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        // Native DCUtR retains attempt history. Bound hostile repeated negotiations;
        // ponytail: 1024 handler events per connected lifetime, reset when all peers disconnect.
        if self.total_events >= 1024 {
            return;
        }
        let count = self.events.entry(id).or_default();
        if *count >= 8 {
            return;
        }
        *count += 1;
        self.total_events += 1;
        self.inner.on_connection_handler_event(peer, id, event);
    }
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        let event = self.inner.poll(cx);
        if let Poll::Ready(ToSwarm::Dial { opts }) = &event {
            self.attempts.insert(opts.connection_id());
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use futures::StreamExt;
    use libp2p::{
        identify, multiaddr::Protocol, noise, relay, swarm::SwarmEvent, yamux, Swarm, SwarmBuilder,
    };
    use std::time::Duration;

    #[derive(NetworkBehaviour)]
    struct Peer {
        relay: relay::client::Behaviour,
        holepunch: HolePunch,
        identify: crate::identify::Identify,
    }
    #[derive(NetworkBehaviour)]
    struct Relay {
        relay: relay::Behaviour,
    }
    fn peer() -> Result<Swarm<Peer>> {
        Ok(SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay| {
                let mut holepunch = HolePunch::new(key.public().to_peer_id());
                holepunch.allow_loopback = true; // Test only; shipping candidates must be public.
                Peer {
                    relay,
                    holepunch,
                    identify: crate::identify::Identify::new(
                        identify::Config::new("/dump/test".into(), key.public())
                            .with_hide_listen_addrs(true)
                            .with_cache_size(0),
                    ),
                }
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build())
    }
    async fn listen<B: NetworkBehaviour>(swarm: &mut Swarm<B>) -> Result<Multiaddr> {
        swarm.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        Ok(tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                    return address;
                }
            }
        })
        .await?)
    }

    #[test]
    fn private_remote_holepunch_addresses_are_denied() -> Result<()> {
        let local = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let remote = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let mut punch = HolePunch::new(local);
        let address: Multiaddr = "/ip4/10.0.0.1/udp/9000/quic-v1".parse()?;
        assert!(!punch.valid(remote, &address));
        assert!(!punch.valid(remote, &"/dns4/example.com/udp/9000/quic-v1".parse()?));
        let address: Multiaddr = "/ip4/8.8.8.8/udp/9000/quic-v1".parse()?;
        assert!(punch.valid(remote, &address));
        assert!(!punch.valid(remote, &address.with(Protocol::P2p(local))));
        let id = ConnectionId::new_unchecked(1);
        punch.attempts.insert(id);
        assert!(punch
            .handle_pending_outbound_connection(
                id,
                Some(remote),
                &["/ip4/127.0.0.1/udp/9000/quic-v1".parse()?],
                Endpoint::Dialer
            )
            .is_err());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dcutr_negotiates_a_direct_quic_upgrade_through_a_local_relay() -> Result<()> {
        let mut a = peer()?;
        let mut b = peer()?;
        let mut relay = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_quic()
            .with_behaviour(|key| Relay {
                relay: relay::Behaviour::new(key.public().to_peer_id(), Default::default()),
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();
        let a_address = listen(&mut a).await?;
        let b_address = listen(&mut b).await?;
        let relay_address = listen(&mut relay).await?;
        relay.add_external_address(relay_address.clone()); // Local simulation only.
        a.behaviour_mut()
            .holepunch
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: &a_address },
            ));
        b.behaviour_mut()
            .holepunch
            .on_swarm_event(FromSwarm::NewExternalAddrCandidate(
                NewExternalAddrCandidate { addr: &b_address },
            ));
        let relay_address = relay_address.with(Protocol::P2p(*relay.local_peer_id()));
        b.listen_on(relay_address.clone().with(Protocol::P2pCircuit))?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    event = b.select_next_some() => if matches!(event, SwarmEvent::NewListenAddr { .. }) { break; },
                    _ = relay.select_next_some() => {},
                }
            }
        }).await?;
        let a_id = *a.local_peer_id();
        let b_id = *b.local_peer_id();
        // No direct dial: the only explicit route supplied is the circuit.
        a.dial(
            relay_address
                .with(Protocol::P2pCircuit)
                .with(Protocol::P2p(b_id)),
        )?;
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut a_direct = false; let mut b_direct = false; let mut upgraded = false;
            loop {
                tokio::select! {
                    event = a.select_next_some() => match event {
                        SwarmEvent::Behaviour(PeerEvent::Holepunch(event)) => upgraded |= event.result.is_ok(),
                        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } if peer_id == b_id && !endpoint.is_relayed() => a_direct = true,
                        _ => {},
                    },
                    event = b.select_next_some() => match event {
                        SwarmEvent::Behaviour(PeerEvent::Holepunch(event)) => upgraded |= event.result.is_ok(),
                        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } if peer_id == a_id && !endpoint.is_relayed() => b_direct = true,
                        _ => {},
                    },
                    _ = relay.select_next_some() => {},
                }
                if a_direct && b_direct && upgraded { break; }
            }
        }).await?;
        Ok(())
    }
}
