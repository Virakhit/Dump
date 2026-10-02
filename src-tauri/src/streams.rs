use libp2p::{
    core::{transport::PortUse, Endpoint},
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
/// a circuit that still carries an active transfer.
pub(crate) struct Streams {
    inner: Toggle<libp2p_stream::Behaviour>,
    relayed: bool,
    connections: BTreeSet<ConnectionId>,
}

impl Streams {
    pub fn new(relayed: bool) -> Self {
        Self {
            inner: Some(libp2p_stream::Behaviour::new()).into(),
            relayed,
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

    fn accepts(&self, address: &Multiaddr) -> bool {
        address
            .iter()
            .any(|p| p == libp2p::multiaddr::Protocol::P2pCircuit)
            == self.relayed
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
        if self.accepts(local) {
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
        if self.accepts(address) {
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
                if e.endpoint.is_relayed() != self.relayed {
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
        self.inner.poll(cx)
    }
}
