use crate::connectivity::public_address;
use libp2p::{
    core::{transport::PortUse, Endpoint},
    identify,
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use std::task::{Context, Poll};

/// Relay clients can confirm LAN-only circuit addresses. Keep them out of Identify
/// even when the swarm legitimately uses them in local tests/private networks.
pub(crate) struct Identify(identify::Behaviour);

impl Identify {
    pub fn new(config: identify::Config) -> Self {
        Self(identify::Behaviour::new(config))
    }
}

impl NetworkBehaviour for Identify {
    type ConnectionHandler = <identify::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = identify::Event;

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        peer: PeerId,
        local: &Multiaddr,
        remote: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.0
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
        self.0
            .handle_established_outbound_connection(id, peer, address, role, port)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        if let FromSwarm::ExternalAddrConfirmed(e) = event {
            if !public_address(e.addr) {
                return;
            }
        }
        self.0.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer: PeerId,
        id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.0.on_connection_handler_event(peer, id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        self.0.poll(cx)
    }
}
