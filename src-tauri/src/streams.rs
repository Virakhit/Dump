use crate::connectivity::direct_address;
use libp2p::{
    core::{transport::PortUse, ConnectedPoint, Endpoint},
    swarm::{
        behaviour::toggle::Toggle, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour,
        THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use std::{
    collections::BTreeSet,
    task::{Context, Poll},
};

/// libp2p-stream chooses randomly among a peer's connections. Keep separate controls
/// for direct and circuit connections so new streams prefer direct without closing
/// a circuit that still carries an active transfer. Temporary invitations use a
/// third control whose handlers exist only on private-address QUIC connections.
pub(crate) struct Streams {
    inner: Toggle<libp2p_stream::Behaviour>,
    route: Route,
    connections: BTreeSet<ConnectionId>,
}

#[derive(Clone, Copy)]
enum Route {
    Direct,
    Relay,
    Lan { allow_loopback: bool },
}

pub(crate) fn lan_address(
    peer: PeerId,
    address: &Multiaddr,
    allow_loopback: bool,
) -> Option<Multiaddr> {
    let address = direct_address(peer, address.clone()).ok()?;
    let mut parts = address.iter();
    let local = match parts.next()? {
        libp2p::multiaddr::Protocol::Ip4(ip) => {
            ip.is_private()
                || ip.is_link_local()
                || (cfg!(test) && allow_loopback && ip.is_loopback())
        }
        libp2p::multiaddr::Protocol::Ip6(ip) => {
            ip.is_unique_local()
                || ip.is_unicast_link_local()
                || (cfg!(test) && allow_loopback && ip.is_loopback())
        }
        _ => false,
    };
    let quic = matches!(parts.next(), Some(libp2p::multiaddr::Protocol::Udp(_)))
        && matches!(parts.next(), Some(libp2p::multiaddr::Protocol::QuicV1))
        && parts.next().is_none();
    (local && quic).then_some(address)
}

pub(crate) fn lan_endpoint(peer: PeerId, endpoint: &ConnectedPoint, allow_loopback: bool) -> bool {
    !endpoint.is_relayed()
        && lan_address(peer, endpoint.get_remote_address(), allow_loopback).is_some()
}

impl Streams {
    pub fn new(relayed: bool) -> Self {
        Self::with_route(if relayed { Route::Relay } else { Route::Direct })
    }

    pub fn lan(allow_loopback: bool) -> Self {
        Self::with_route(Route::Lan {
            allow_loopback: allow_loopback && cfg!(test),
        })
    }

    fn with_route(route: Route) -> Self {
        Self {
            inner: Some(libp2p_stream::Behaviour::new()).into(),
            route,
            connections: BTreeSet::new(),
        }
    }

    pub fn control(&self) -> libp2p_stream::Control {
        // Always constructed enabled; no remote input can toggle the behaviour.
        self.inner
            .as_ref()
            .expect("stream behaviour is enabled")
            .new_control()
    }

    fn accepts(&self, peer: PeerId, address: &Multiaddr) -> bool {
        let relayed = address
            .iter()
            .any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit);
        match self.route {
            Route::Direct => !relayed,
            Route::Relay => relayed,
            Route::Lan { allow_loopback } => lan_address(peer, address, allow_loopback).is_some(),
        }
    }
}

impl NetworkBehaviour for Streams {
    type ConnectionHandler =
        <Toggle<libp2p_stream::Behaviour> as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = ();

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        let accepted = match self.route {
            Route::Lan { .. } => {
                !local
                    .iter()
                    .any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit)
                    && self.accepts(peer, remote)
            }
            _ => self.accepts(peer, local),
        };
        if accepted {
            self.inner
                .handle_established_inbound_connection(id, peer, local, remote)
        } else {
            Toggle::<libp2p_stream::Behaviour>::from(None)
                .handle_established_inbound_connection(id, peer, local, remote)
        }
    }

    fn handle_established_outbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        address: &Multiaddr,
        role: Endpoint,
        port: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        if self.accepts(peer, address) {
            self.inner
                .handle_established_outbound_connection(id, peer, address, role, port)
        } else {
            Toggle::<libp2p_stream::Behaviour>::from(None)
                .handle_established_outbound_connection(id, peer, address, role, port)
        }
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(e) => {
                let accepted = match self.route {
                    Route::Direct => !e.endpoint.is_relayed(),
                    Route::Relay => e.endpoint.is_relayed(),
                    Route::Lan { allow_loopback } => {
                        lan_endpoint(e.peer_id, e.endpoint, allow_loopback)
                    }
                };
                if !accepted {
                    return;
                }
                self.connections.insert(e.connection_id);
            }
            FromSwarm::ConnectionClosed(e) if !self.connections.remove(&e.connection_id) => return,
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
        if self.connections.contains(&id) {
            self.inner.on_connection_handler_event(peer, id, event);
        }
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        loop {
            match self.inner.poll(cx) {
                // LAN callers explicitly dial a current mDNS address in the actor.
                // Never let an open/close race fall back to a public or relay route.
                Poll::Ready(ToSwarm::Dial { opts }) if matches!(self.route, Route::Lan { .. }) => {
                    let error = libp2p::swarm::DialError::NoAddresses;
                    self.inner.on_swarm_event(FromSwarm::DialFailure(
                        libp2p::swarm::behaviour::DialFailure {
                            peer_id: opts.get_peer_id(),
                            error: &error,
                            connection_id: opts.connection_id(),
                        },
                    ));
                    continue;
                }
                event => return event,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::Future;

    #[tokio::test]
    async fn disconnected_lan_stream_fails_without_dialing_or_stranding_requests() {
        let mut streams = Streams::lan(true);
        for _ in 0..32 {
            let peer = libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id();
            let mut control = streams.control();
            let request = control.open_stream(
                peer,
                libp2p::swarm::StreamProtocol::new("/dump/lan-invite/1"),
            );
            futures::pin_mut!(request);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                futures::future::poll_fn(|cx| {
                    let result = request.as_mut().poll(cx);
                    assert!(matches!(streams.poll(cx), Poll::Pending));
                    result
                }),
            )
            .await
            .expect("disconnected LAN request remained pending");
            assert!(result.is_err());
        }
    }
}
