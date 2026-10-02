use std::{
    collections::{HashMap, VecDeque},
    fmt::{Debug, Display, Formatter},
    task::{Context, Poll},
    time::Duration,
};

use either::Either;
use futures::FutureExt;
use futures_timer::Delay;
use libp2p_core::{Endpoint, Multiaddr, transport::PortUse};
use libp2p_identity::PeerId;
use libp2p_swarm::{
    ConnectionClosed, ConnectionDenied, ConnectionHandler, ConnectionId, FromSwarm,
    NetworkBehaviour, NewExternalAddrCandidate, NotifyHandler, ToSwarm,
    behaviour::ConnectionEstablished,
};
use rand::prelude::*;

use super::handler::{
    dial_back::{self, IncomingNonce},
    dial_request,
};
use crate::v2::{Nonce, protocol::DialRequest};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// How many candidates we will test at most.
    pub(crate) max_candidates: usize,

    /// The interval at which we will attempt to confirm candidates as external addresses.
    pub(crate) probe_interval: Duration,
}

impl Config {
    pub fn with_max_candidates(self, max_candidates: usize) -> Self {
        Self {
            max_candidates,
            ..self
        }
    }

    pub fn with_probe_interval(self, probe_interval: Duration) -> Self {
        Self {
            probe_interval,
            ..self
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_candidates: 10,
            probe_interval: Duration::from_secs(5),
        }
    }
}

pub struct Behaviour<R = rand::rngs::StdRng>
where
    R: rand::Rng + 'static,
{
    rng: R,
    config: Config,
    pending_events: VecDeque<
        ToSwarm<
            <Self as NetworkBehaviour>::ToSwarm,
            <<Self as NetworkBehaviour>::ConnectionHandler as ConnectionHandler>::FromBehaviour,
        >,
    >,
    address_candidates: HashMap<Multiaddr, AddressInfo>,
    next_tick: Delay,
    peer_info: HashMap<ConnectionId, ConnectionInfo>,
}

impl<R> NetworkBehaviour for Behaviour<R>
where
    R: rand::Rng + 'static,
{
    type ConnectionHandler = Either<dial_request::Handler, dial_back::Handler>;

    type ToSwarm = Event;

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<<Self as NetworkBehaviour>::ConnectionHandler, ConnectionDenied> {
        Ok(Either::Right(dial_back::Handler::new()))
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: Endpoint,
        _: PortUse,
    ) -> Result<<Self as NetworkBehaviour>::ConnectionHandler, ConnectionDenied> {
        Ok(Either::Left(dial_request::Handler::new()))
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::NewExternalAddrCandidate(NewExternalAddrCandidate { addr }) => {
                let info = self.address_candidates.entry(addr.clone()).or_default();
                info.score = info.score.saturating_add(1);
            }
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                ..
            }) => {
                self.peer_info.insert(
                    connection_id,
                    ConnectionInfo {
                        peer_id,
                        supports_autonat: false,
                    },
                );
            }
            FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id,
                connection_id,
                ..
            }) => {
                let info = self
                    .peer_info
                    .remove(&connection_id)
                    .expect("inconsistent state");

                if info.supports_autonat {
                    tracing::debug!(%peer_id, "Disconnected from AutoNAT server");
                }
                for (address, candidate) in &mut self.address_candidates {
                    if candidate.connection == Some(connection_id) {
                        candidate.status = TestStatus::Failed;
                        candidate.connection = None;
                        self.pending_events
                            .push_back(ToSwarm::ExternalAddrExpired(address.clone()));
                        self.pending_events.push_back(ToSwarm::GenerateEvent(Event {
                            tested_addr: address.clone(),
                            bytes_sent: 0,
                            server: peer_id,
                            result: Err(Error { inner: None }),
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: <Self::ConnectionHandler as ConnectionHandler>::ToBehaviour,
    ) {
        let (nonce, outcome) = match event {
            Either::Right(IncomingNonce { nonce, sender }) => {
                let Some((_, info)) = self.address_candidates.iter_mut().find(|(_, info)| {
                    info.is_pending_with_nonce(nonce)
                        && info
                            .connection
                            .and_then(|id| self.peer_info.get(&id))
                            .is_some_and(|connection| connection.peer_id == peer_id)
                }) else {
                    let _ = sender.send(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Received unexpected nonce: {nonce} from {peer_id}"),
                    )));
                    return;
                };

                info.status = TestStatus::Received(nonce);
                tracing::debug!(%peer_id, %nonce, "Successful dial-back");

                let _ = sender.send(Ok(()));

                return;
            }
            Either::Left(dial_request::ToBehaviour::PeerHasServerSupport) => {
                self.peer_info
                    .get_mut(&connection_id)
                    .expect("inconsistent state")
                    .supports_autonat = true;
                return;
            }
            Either::Left(dial_request::ToBehaviour::TestOutcome { nonce, outcome }) => {
                (nonce, outcome)
            }
        };

        let Some((expected_addr, _)) = self.address_candidates.iter().find(|(_, info)| {
            info.connection == Some(connection_id)
                && (info.is_pending_with_nonce(nonce) || info.is_received_with_nonce(nonce))
        }) else {
            return;
        };
        let expected_addr = expected_addr.clone();

        let ((tested_addr, bytes_sent), result) = match outcome {
            Ok(address) => {
                let received_dial_back =
                    self.address_candidates.get(&address.0).is_some_and(|info| {
                        info.is_received_with_nonce(nonce) && info.connection == Some(connection_id)
                    });

                if !received_dial_back {
                    tracing::warn!(
                        %peer_id,
                        %nonce,
                        "Server reported reachbility but we never received a dial-back"
                    );
                    self.reset_status_to(nonce, TestStatus::Failed);
                    ((expected_addr, 0), Err(Error { inner: None }))
                } else {
                    self.reset_status_to(nonce, TestStatus::Confirmed);
                    self.pending_events
                        .push_back(ToSwarm::ExternalAddrConfirmed(address.0.clone()));
                    (address, Ok(()))
                }
            }
            Err(dial_request::Error::UnsupportedProtocol) => {
                self.peer_info
                    .get_mut(&connection_id)
                    .expect("inconsistent state")
                    .supports_autonat = false;

                self.reset_status_to(nonce, TestStatus::Failed); // Retry only when the caller's backoff allows it.

                ((expected_addr, 0), Err(Error { inner: None }))
            }
            Err(dial_request::Error::Io(e)) => {
                tracing::debug!(
                    %peer_id,
                    %nonce,
                    "Failed to complete AutoNAT probe: {e}"
                );

                self.reset_status_to(nonce, TestStatus::Failed); // Retry only when the caller's backoff allows it.

                ((expected_addr, 0), Err(Error { inner: None }))
            }
            Err(dial_request::Error::AddressNotReachable {
                address,
                bytes_sent,
                error,
            }) => {
                self.reset_status_to(nonce, TestStatus::Failed);
                if address != expected_addr {
                    ((expected_addr, 0), Err(Error { inner: None }))
                } else {
                    ((address, bytes_sent), Err(Error { inner: Some(error) }))
                }
            }
        };

        if result.is_err() {
            self.pending_events
                .push_back(ToSwarm::ExternalAddrExpired(tested_addr.clone()));
        }

        self.pending_events.push_back(ToSwarm::GenerateEvent(Event {
            tested_addr,
            bytes_sent,
            server: peer_id,
            result,
        }));
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, <Self::ConnectionHandler as ConnectionHandler>::FromBehaviour>>
    {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Poll::Ready(event);
            }

            if self.next_tick.poll_unpin(cx).is_ready() {
                self.next_tick.reset(self.config.probe_interval);

                self.issue_dial_requests_for_untested_candidates();
                continue;
            }

            return Poll::Pending;
        }
    }
}

impl<R> Behaviour<R>
where
    R: rand::Rng + 'static,
{
    pub fn new(rng: R, config: Config) -> Self {
        Self {
            rng,
            next_tick: Delay::new(config.probe_interval),
            config,
            pending_events: VecDeque::new(),
            address_candidates: HashMap::new(),
            peer_info: HashMap::new(),
        }
    }

    /// Issues dial requests to random AutoNAT servers for the most frequently reported, untested
    /// candidates.
    ///
    /// In the current implementation, we only send a single address to each AutoNAT server.
    /// This spreads our candidates out across all servers we are connected to which should give us
    /// pretty fast feedback on all of them.
    fn issue_dial_requests_for_untested_candidates(&mut self) {
        for addr in self.untested_candidates() {
            let Some((conn_id, peer_id)) = self.random_autonat_server() else {
                tracing::debug!("Not connected to any AutoNAT servers");
                return;
            };

            let nonce = self.rng.random();
            let info = self
                .address_candidates
                .get_mut(&addr)
                .expect("only emit candidates");
            info.status = TestStatus::Pending(nonce);
            info.connection = Some(conn_id);

            self.pending_events.push_back(ToSwarm::NotifyHandler {
                peer_id,
                handler: NotifyHandler::One(conn_id),
                event: Either::Left(DialRequest {
                    nonce,
                    addrs: vec![addr],
                }),
            });
        }
    }

    /// Returns all untested candidates, sorted by the frequency they were reported at.
    ///
    /// More frequently reported candidates are considered to more likely be external addresses and
    /// thus tested first.
    fn untested_candidates(&self) -> impl Iterator<Item = Multiaddr> + use<R> {
        let mut entries = self
            .address_candidates
            .iter()
            .filter(|(_, info)| info.status == TestStatus::Untested)
            .map(|(addr, count)| (addr.clone(), *count))
            .collect::<Vec<_>>();

        entries.sort_unstable_by_key(|(_, info)| info.score);

        if entries.is_empty() {
            tracing::debug!("No untested address candidates");
        }

        entries
            .into_iter()
            .rev() // `sort_unstable` is ascending
            .take(self.config.max_candidates)
            .map(|(addr, _)| addr)
    }

    /// Chooses an active connection to one of our peers that reported support for the
    /// [`DIAL_REQUEST_PROTOCOL`](crate::v2::DIAL_REQUEST_PROTOCOL) protocol.
    fn random_autonat_server(&mut self) -> Option<(ConnectionId, PeerId)> {
        let (conn_id, info) = self
            .peer_info
            .iter()
            .filter(|(_, info)| info.supports_autonat)
            .choose(&mut self.rng)?;

        Some((*conn_id, info.peer_id))
    }

    fn reset_status_to(&mut self, nonce: Nonce, new_status: TestStatus) {
        let Some((_, info)) = self
            .address_candidates
            .iter_mut()
            .find(|(_, i)| i.is_pending_with_nonce(nonce) || i.is_received_with_nonce(nonce))
        else {
            return;
        };

        info.status = new_status;
        info.connection = None;
    }

    /// Requeue a completed candidate without replacing connection handlers or active nonces.
    /// The caller is responsible for its revalidation interval and candidate cache bound.
    pub fn retry_candidate(&mut self, addr: &Multiaddr) -> bool {
        let Some(info) = self.address_candidates.get_mut(addr) else {
            return false;
        };
        if matches!(
            info.status,
            TestStatus::Pending(_) | TestStatus::Received(_)
        ) {
            return false;
        }
        info.status = TestStatus::Untested;
        true
    }

    /// Remove an idle candidate. In-flight dial-backs must finish before it can be removed.
    pub fn remove_candidate(&mut self, addr: &Multiaddr) -> bool {
        if self.address_candidates.get(addr).is_some_and(|info| {
            matches!(
                info.status,
                TestStatus::Pending(_) | TestStatus::Received(_)
            )
        }) {
            return false;
        }
        self.address_candidates.remove(addr);
        self.pending_events.retain(|event| match event {
            ToSwarm::ExternalAddrConfirmed(address) | ToSwarm::ExternalAddrExpired(address) => {
                address != addr
            }
            ToSwarm::GenerateEvent(event) => &event.tested_addr != addr,
            _ => true,
        });
        true
    }

    // FIXME: We don't want test-only APIs in our public API.
    #[doc(hidden)]
    pub fn validate_addr(&mut self, addr: &Multiaddr) {
        if let Some(info) = self.address_candidates.get_mut(addr) {
            info.status = TestStatus::Received(self.rng.next_u64());
        }
    }
}

impl Default for Behaviour<rand::rngs::StdRng> {
    fn default() -> Self {
        Self::new(rand::make_rng::<rand::rngs::StdRng>(), Config::default())
    }
}

pub struct Error {
    pub(crate) inner: Option<dial_request::DialBackError>,
}

impl Error {
    /// No reliable reachable/unreachable outcome was obtained from this probe.
    pub fn is_inconclusive(&self) -> bool {
        self.inner.is_none()
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(error) => Display::fmt(error, f),
            None => f.write_str("AutoNAT probe was inconclusive"),
        }
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

#[derive(Debug)]
pub struct Event {
    /// The address that was selected for testing.
    pub tested_addr: Multiaddr,
    /// The amount of data that was sent to the server.
    /// Is 0 if it wasn't necessary to send any data.
    /// Otherwise it's a number between 30.000 and 100.000.
    pub bytes_sent: usize,
    /// The peer id of the server that was selected for testing.
    pub server: PeerId,
    /// The result of the test. If the test was successful, this is `Ok(())`.
    /// Otherwise it's an error.
    pub result: Result<(), Error>,
}

struct ConnectionInfo {
    peer_id: PeerId,
    supports_autonat: bool,
}

#[derive(Copy, Clone, Default)]
struct AddressInfo {
    score: usize,
    status: TestStatus,
    connection: Option<ConnectionId>,
}

impl AddressInfo {
    fn is_pending_with_nonce(&self, nonce: Nonce) -> bool {
        match self.status {
            TestStatus::Pending(c) => c == nonce,
            _ => false,
        }
    }

    fn is_received_with_nonce(&self, nonce: Nonce) -> bool {
        match self.status {
            TestStatus::Received(c) => c == nonce,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum TestStatus {
    #[default]
    Untested,
    Pending(Nonce),
    Failed,
    Confirmed,
    Received(Nonce),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use libp2p_core::ConnectedPoint;

    fn pending() -> (Behaviour, Multiaddr, PeerId, ConnectionId, Nonce) {
        let mut client = Behaviour::default();
        let address: Multiaddr = "/ip4/8.8.8.8/udp/9000/quic-v1".parse().unwrap();
        let server = PeerId::random();
        let id = ConnectionId::new_unchecked(1);
        client.peer_info.insert(
            id,
            ConnectionInfo {
                peer_id: server,
                supports_autonat: true,
            },
        );
        client.on_swarm_event(FromSwarm::NewExternalAddrCandidate(
            NewExternalAddrCandidate { addr: &address },
        ));
        client.issue_dial_requests_for_untested_candidates();
        let TestStatus::Pending(nonce) = client.address_candidates[&address].status else {
            panic!("probe was not queued");
        };
        client.pending_events.clear();
        (client, address, server, id, nonce)
    }

    fn outcome(
        client: &mut Behaviour,
        address: Multiaddr,
        server: PeerId,
        id: ConnectionId,
        nonce: Nonce,
    ) {
        client.on_connection_handler_event(
            server,
            id,
            Either::Left(dial_request::ToBehaviour::TestOutcome {
                nonce,
                outcome: Ok((address, 0)),
            }),
        );
    }

    fn dialback(client: &mut Behaviour, server: PeerId, nonce: Nonce) {
        let (sender, _) = oneshot::channel();
        client.on_connection_handler_event(
            server,
            ConnectionId::new_unchecked(2),
            Either::Right(IncomingNonce { nonce, sender }),
        );
    }

    fn inconclusive(client: &Behaviour, address: &Multiaddr) {
        assert!(
            !client
                .pending_events
                .iter()
                .any(|event| matches!(event, ToSwarm::ExternalAddrConfirmed(_)))
        );
        assert!(client.pending_events.iter().any(
            |event| matches!(event, ToSwarm::ExternalAddrExpired(expired) if expired == address)
        ));
        assert!(client.pending_events.iter().any(|event| matches!(event, ToSwarm::GenerateEvent(event) if &event.tested_addr == address && event.result.as_ref().is_err_and(Error::is_inconclusive))));
    }

    #[test]
    fn completed_candidate_requeues_without_replacing_connected_handlers() {
        let (mut client, address, server, id, nonce) = pending();
        assert!(!client.retry_candidate(&address));
        assert!(!client.remove_candidate(&address));
        dialback(&mut client, server, nonce);
        assert!(!client.retry_candidate(&address));
        assert!(!client.remove_candidate(&address));
        outcome(&mut client, address.clone(), server, id, nonce);
        assert!(matches!(
            client.address_candidates[&address].status,
            TestStatus::Confirmed
        ));
        assert!(client.retry_candidate(&address));
        assert!(client.peer_info[&id].supports_autonat);
        client.issue_dial_requests_for_untested_candidates();
        let TestStatus::Pending(new_nonce) = client.address_candidates[&address].status else {
            panic!("re-probe was not queued");
        };
        assert_ne!(new_nonce, nonce);
        assert_eq!(client.address_candidates[&address].connection, Some(id));
    }

    #[test]
    fn remote_success_claim_needs_exact_address_nonce_and_probe_server() {
        let (mut client, address, server, id, nonce) = pending();
        outcome(&mut client, address.clone(), server, id, nonce);
        inconclusive(&client, &address);
        assert!(matches!(
            client.address_candidates[&address].status,
            TestStatus::Failed
        ));

        let (mut client, address, server, id, nonce) = pending();
        dialback(&mut client, PeerId::random(), nonce);
        outcome(&mut client, address.clone(), server, id, nonce);
        inconclusive(&client, &address);

        let (mut client, address, server, id, nonce) = pending();
        dialback(&mut client, server, nonce);
        outcome(
            &mut client,
            "/ip4/1.1.1.1/udp/9000/quic-v1".parse().unwrap(),
            server,
            id,
            nonce,
        );
        inconclusive(&client, &address);
        assert!(matches!(
            client.address_candidates[&address].status,
            TestStatus::Failed
        ));

        let (mut client, address, server, id, nonce) = pending();
        dialback(&mut client, server, nonce);
        outcome(
            &mut client,
            address.clone(),
            server,
            ConnectionId::new_unchecked(9),
            nonce,
        );
        assert!(client.pending_events.is_empty());
        assert!(!client.retry_candidate(&address));
        outcome(&mut client, address, server, id, nonce);
        assert!(
            client
                .pending_events
                .iter()
                .any(|event| matches!(event, ToSwarm::ExternalAddrConfirmed(_)))
        );
    }

    #[test]
    fn closed_probe_connection_and_io_failure_release_nonce_for_bounded_retry() {
        let (mut client, address, server, id, _) = pending();
        let endpoint = ConnectedPoint::Dialer {
            address: address.clone(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        };
        client.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id: server,
            connection_id: id,
            endpoint: &endpoint,
            cause: None,
            remaining_established: 0,
        }));
        inconclusive(&client, &address);
        assert!(client.retry_candidate(&address));
        assert!(client.remove_candidate(&address));

        let (mut client, address, server, id, nonce) = pending();
        client.on_connection_handler_event(
            server,
            id,
            Either::Left(dial_request::ToBehaviour::TestOutcome {
                nonce,
                outcome: Err(dial_request::Error::Io(std::io::ErrorKind::TimedOut.into())),
            }),
        );
        assert!(matches!(
            client.address_candidates[&address].status,
            TestStatus::Failed
        ));
        inconclusive(&client, &address);
        client.pending_events.clear();
        client.issue_dial_requests_for_untested_candidates();
        assert!(client.pending_events.is_empty());
        assert!(client.retry_candidate(&address));
    }

    #[test]
    fn inconclusive_reprobe_revokes_old_confirmation_and_failed_nonce_can_retry() {
        let (mut client, address, server, id, nonce) = pending();
        dialback(&mut client, server, nonce);
        outcome(&mut client, address.clone(), server, id, nonce);
        assert!(matches!(
            client.address_candidates[&address].status,
            TestStatus::Confirmed
        ));
        client.pending_events.clear();
        assert!(client.retry_candidate(&address));
        client.issue_dial_requests_for_untested_candidates();
        let TestStatus::Pending(nonce) = client.address_candidates[&address].status else {
            panic!("re-probe was not queued");
        };
        client.pending_events.clear();
        client.on_connection_handler_event(
            server,
            id,
            Either::Left(dial_request::ToBehaviour::TestOutcome {
                nonce,
                outcome: Err(dial_request::Error::Io(std::io::ErrorKind::TimedOut.into())),
            }),
        );
        inconclusive(&client, &address);
        assert!(client.retry_candidate(&address));
    }

    #[test]
    fn retired_candidate_finishes_pending_nonce_without_advertising_the_retired_route() {
        let (mut client, address, server, id, nonce) = pending();
        assert!(!client.remove_candidate(&address));
        dialback(&mut client, server, nonce);
        assert!(!client.remove_candidate(&address));
        outcome(&mut client, address.clone(), server, id, nonce);
        // The wrapper prunes an expired listener before polling queued events.
        assert!(client.remove_candidate(&address));
        assert!(client.pending_events.is_empty());
        assert!(!client.address_candidates.contains_key(&address));
        assert!(client.peer_info[&id].supports_autonat);
    }
}
