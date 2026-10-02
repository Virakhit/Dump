use crate::{
    connectivity::{
        diagnostic, direct_address, is_relayed, peer_address, public_address, AddressBook,
        ConnectionState, Diagnostics, DialFailure, ProtocolOutcome, Reachability,
        ReachabilityStatus, MAX_ADDRESSES,
    },
    engine::{open_source, refresh_share, stamp, Shared, PARTIAL_CLEANUP_FAILED},
    holepunch::HolePunch,
    identify::Identify,
    model::*,
    reachability::{Client as NatClient, PublicAddresses},
    storage::commit_download,
    streams::Streams,
};
use anyhow::{bail, ensure, Context, Result};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
use libp2p::{
    connection_limits, identify, mdns, noise, ping, relay,
    swarm::{
        behaviour::toggle::Toggle,
        dial_opts::{DialOpts, PeerCondition},
        ConnectionId, DialError, FromSwarm, NetworkBehaviour, NewExternalAddrCandidate, Stream,
        StreamProtocol, SwarmEvent,
    },
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use libp2p_stream::Control;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt as DiskRead, AsyncWriteExt as DiskWrite},
    sync::{mpsc, oneshot, watch, Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

const CONTROL: StreamProtocol = StreamProtocol::new("/dump/control/1");
const FILE: StreamProtocol = StreamProtocol::new("/dump/file/1");
const CONTACT: StreamProtocol = StreamProtocol::new("/dump/contact/1");
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(5);
const INACTIVITY: Duration = Duration::from_secs(30);
const RELAY_INTERRUPTED: &str = "Relay connection closed or stalled. Its transfer limit may have been reached. Try again manually using a direct connection or ask the relay operator about limits.";

fn relay_error(error: anyhow::Error, relayed: bool) -> anyhow::Error {
    if relayed
        && error
            .chain()
            .any(|cause| cause.is::<std::io::Error>() || cause.is::<tokio::time::error::Elapsed>())
    {
        error.context(RELAY_INTERRUPTED)
    } else {
        error
    }
}

async fn remove_owned_partial(part: &Path, created: bool) -> bool {
    if !created {
        return true;
    }
    match tokio::fs::remove_file(part).await {
        Ok(()) => true,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

async fn create_owned_partial(
    shared: &Shared,
    part: &Path,
    id: Uuid,
    created: &mut bool,
) -> Result<tokio::fs::File> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(part)
        .await
        .context("Cannot create temporary download")?;
    *created = true;
    // Record only a path we exclusively created, never a colliding user's file.
    let mut e = shared.lock().await;
    let mut next = e.persisted.clone();
    next.partials.push(part.to_path_buf());
    e.persist(next)?;
    e.mark_transfer(id, "Receiving", 0, None);
    Ok(file)
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    // Check connection admission before other handlers allocate per-connection state.
    limits: connection_limits::Behaviour,
    streams: Streams,
    relay_streams: Streams,
    relay: relay::client::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    ping: ping::Behaviour,
    identify: Identify,
    autonat: NatClient,
    holepunch: HolePunch,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Request {
    Join {
        invitation: Invitation,
        name: String,
    },
    Catalog {
        snapshot: Snapshot,
        offset: u32,
    },
    File {
        snapshot: Snapshot,
        manifest: Manifest,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Response {
    Waiting,
    Approved {
        workspace: Workspace,
    },
    Catalog {
        snapshot: Snapshot,
        files: Vec<Manifest>,
        total: u32,
    },
    File {
        manifest: Manifest,
    },
    Done,
    Denied,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactRequest {
    snapshot: Snapshot,
    contact: Option<crate::contact::Contact>,
    offset: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactResponse {
    contacts: Vec<crate::contact::Contact>,
    total: u32,
}

pub async fn write_frame<S: AsyncWrite + Unpin, T: Serialize>(
    stream: &mut S,
    value: &T,
) -> Result<()> {
    let mut data = Vec::new();
    ciborium::into_writer(value, &mut data)?;
    ensure!(data.len() <= MAX_CONTROL, "Control frame exceeds limit");
    tokio::time::timeout(PREAUTH_TIMEOUT, async {
        stream.write_all(&(data.len() as u32).to_be_bytes()).await?;
        stream.write_all(&data).await?;
        stream.flush().await?;
        Ok::<_, std::io::Error>(())
    })
    .await??;
    Ok(())
}
pub async fn read_frame<S: AsyncRead + Unpin, T: DeserializeOwned>(stream: &mut S) -> Result<T> {
    tokio::time::timeout(PREAUTH_TIMEOUT, async {
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await?;
        let len = u32::from_be_bytes(prefix) as usize;
        ensure!(
            len > 0 && len <= MAX_CONTROL,
            "Invalid control frame length"
        );
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await?;
        let mut cursor = std::io::Cursor::new(bytes);
        let value = ciborium::from_reader(&mut cursor)?;
        ensure!(cursor.position() as usize == len, "Trailing control data");
        Ok::<T, anyhow::Error>(value)
    })
    .await?
}

type PeerSlots = (Arc<Semaphore>, Arc<Semaphore>);
struct Limits {
    send: Arc<Semaphore>,
    receive: Arc<Semaphore>,
    peers: Mutex<BTreeMap<PeerId, PeerSlots>>,
    #[cfg(test)]
    receive_pause: Mutex<Option<ReceivePause>>,
}

#[cfg(test)]
struct ReceivePause {
    started: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}
impl Limits {
    fn new() -> Self {
        Self {
            send: Arc::new(Semaphore::new(2)),
            receive: Arc::new(Semaphore::new(2)),
            peers: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            receive_pause: Mutex::new(None),
        }
    }
    async fn peer(&self, id: PeerId, send: bool) -> Result<Arc<Semaphore>> {
        let mut peers = self.peers.lock().await;
        if !peers.contains_key(&id) {
            ensure!(peers.len() < 256, "Peer transfer limit reached");
            peers.insert(
                id,
                (Arc::new(Semaphore::new(1)), Arc::new(Semaphore::new(1))),
            );
        }
        let (out, input) = peers.get(&id).unwrap();
        Ok(if send { out.clone() } else { input.clone() })
    }
}

struct Dial {
    peer: PeerId,
    address: Multiaddr,
    reply: oneshot::Sender<Result<()>>,
    reserve: bool,
}

struct ProtocolReport {
    peer: PeerId,
    protocol: StreamProtocol,
    outcome: ProtocolOutcome,
    #[cfg(test)]
    probe: Option<TestProbe>,
}

#[cfg(test)]
struct TestProbe {
    address: Option<Multiaddr>,
    revalidate: bool,
    reply: oneshot::Sender<BTreeSet<ConnectionId>>,
}

fn connection_state(
    connections: &BTreeMap<ConnectionId, (PeerId, bool)>,
    peer: PeerId,
) -> ConnectionState {
    if connections
        .values()
        .any(|(id, relayed)| *id == peer && !relayed)
    {
        ConnectionState::Direct
    } else if connections
        .values()
        .any(|(id, relayed)| *id == peer && *relayed)
    {
        ConnectionState::Relay
    } else {
        ConnectionState::Offline
    }
}

fn preferred_routes(peer: PeerId, addresses: Vec<Multiaddr>) -> Vec<Multiaddr> {
    let mut routes: Vec<_> = addresses
        .into_iter()
        .filter_map(|a| peer_address(peer, a).ok())
        .collect();
    routes.sort_by_key(|a| {
        (
            is_relayed(a),
            public_address(a),
            !a.iter().any(|p| p == libp2p::multiaddr::Protocol::QuicV1),
            a.to_string(),
        )
    });
    routes.dedup();
    routes.truncate(MAX_ADDRESSES);
    routes
}

#[derive(Clone)]
pub struct Node {
    pub shared: Shared,
    pub control: Control,
    relay_control: Control,
    dial: mpsc::Sender<Dial>,
    protocol_reports: mpsc::Sender<ProtocolReport>,
    tasks: TaskTracker,
    pub addresses: watch::Receiver<Vec<Multiaddr>>,
    pub diagnostics: watch::Receiver<Diagnostics>,
    pub reachability: watch::Receiver<Reachability>,
    pub relay_addresses: watch::Receiver<Vec<Multiaddr>>,
    limits: Arc<Limits>,
    pub shutdown: CancellationToken,
    #[cfg(test)]
    nat_confirmations: watch::Receiver<usize>,
}
impl Node {
    pub async fn start(shared: Shared, discovery: bool) -> Result<Self> {
        Self::start_inner(shared, discovery, false).await
    }
    async fn start_inner(shared: Shared, discovery: bool, allow_loopback: bool) -> Result<Self> {
        let allow_loopback = allow_loopback && cfg!(test);
        let key = shared.lock().await.key.clone();
        let peer = key.public().to_peer_id();
        let mdns = if discovery {
            Some(mdns::tokio::Behaviour::new(mdns::Config::default(), peer)?)
        } else {
            None
        };
        let mut swarm = SwarmBuilder::with_existing_identity(key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay| Behaviour {
                streams: Streams::new(false),
                relay_streams: Streams::new(true),
                relay,
                mdns: mdns.into(),
                ping: ping::Behaviour::new(
                    ping::Config::new()
                        .with_interval(Duration::from_secs(5))
                        .with_timeout(Duration::from_secs(5)),
                ),
                identify: Identify::new(
                    identify::Config::new("/dump/1".into(), key.public())
                        .with_agent_version(format!("dump/{}", env!("CARGO_PKG_VERSION")))
                        // Never leak interface addresses to Internet peers or cache their
                        // unchecked routing hints. mDNS remains independent for LAN peers.
                        .with_hide_listen_addrs(true)
                        .with_cache_size(0),
                ),
                autonat: NatClient::new(peer),
                holepunch: HolePunch::new(peer),
                limits: connection_limits::Behaviour::new(
                    connection_limits::ConnectionLimits::default()
                        .with_max_pending_incoming(Some(16))
                        .with_max_pending_outgoing(Some(16))
                        .with_max_established(Some(128))
                        .with_max_established_per_peer(Some(2)),
                ),
            })?
            .with_swarm_config(|config| {
                config.with_idle_connection_timeout(Duration::from_secs(60))
            })
            .build();
        swarm.listen_on(
            if discovery {
                "/ip4/0.0.0.0/udp/0/quic-v1"
            } else {
                "/ip4/127.0.0.1/udp/0/quic-v1"
            }
            .parse()?,
        )?;
        swarm.listen_on(
            if discovery {
                "/ip4/0.0.0.0/tcp/0"
            } else {
                "/ip4/127.0.0.1/tcp/0"
            }
            .parse()?,
        )?;
        let mut control = swarm.behaviour().streams.control();
        let mut relay_control = swarm.behaviour().relay_streams.control();
        let mut incoming =
            futures::stream::select(control.accept(CONTROL)?, relay_control.accept(CONTROL)?);
        let mut files = futures::stream::select(
            control
                .accept(FILE)?
                .map(|(peer, stream)| (peer, stream, false)),
            relay_control
                .accept(FILE)?
                .map(|(peer, stream)| (peer, stream, true)),
        );
        let mut contacts =
            futures::stream::select(control.accept(CONTACT)?, relay_control.accept(CONTACT)?);
        let (dial, mut dials) = mpsc::channel(128);
        let (protocol_reports, mut reports) = mpsc::channel::<ProtocolReport>(128);
        let tasks = TaskTracker::new();
        let (address_tx, addresses) = watch::channel(Vec::new());
        let (diagnostic_tx, diagnostics) = watch::channel(Diagnostics::new());
        let (reachability_tx, reachability) = watch::channel(Reachability::default());
        #[cfg(test)]
        let (nat_confirmations_tx, nat_confirmations) = watch::channel(0);
        let (relay_tx, relay_addresses) = watch::channel(Vec::new());
        let limits = Arc::new(Limits::new());
        let shutdown = CancellationToken::new();
        let (host_error, mut host_errors) = mpsc::channel(1);
        {
            let mut e = shared.lock().await;
            if let Err(err) = crate::relay_host::start(
                e.relay_key.clone(),
                &e.persisted.network,
                shutdown.clone(),
                host_error,
                &tasks,
            ) {
                e.error(format!("Network assistance could not start: {err}"));
            }
        }
        let node = Self {
            shared: shared.clone(),
            control: control.clone(),
            relay_control,
            dial,
            protocol_reports,
            tasks: tasks.clone(),
            addresses,
            diagnostics,
            reachability,
            relay_addresses,
            limits: limits.clone(),
            shutdown: shutdown.clone(),
            #[cfg(test)]
            nat_confirmations,
        };
        let poll_node = node.clone();
        let incoming_slots = Arc::new(Semaphore::new(32));
        let incoming_file_slots = Arc::new(Semaphore::new(8));
        let actor_tasks = tasks.clone();
        tasks.spawn(async move {
            let mut discovered = AddressBook::default();
            let in_flight = Arc::new(Mutex::new(BTreeSet::new()));
            let mut last_join = 0;
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut listeners = Vec::new();
            let mut public = PublicAddresses::default();
            let mut reservations = BTreeMap::new();
            let mut connections = BTreeMap::new();
            let mut attempts: BTreeMap<PeerId, (Instant, usize)> = BTreeMap::new();
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    Some(message) = host_errors.recv() => shared.lock().await.error(message),
                    Some(report) = reports.recv() => {
                        #[cfg(test)]
                        if let Some(probe) = report.probe {
                            swarm.behaviour_mut().autonat.test_probe(probe.address.as_ref(), probe.revalidate);
                            let _ = probe.reply.send(connections.iter().filter(|(_, (peer, _))| *peer == report.peer).map(|(id, _)| *id).collect());
                            continue;
                        }
                        diagnostic_tx.send_modify(|d| { if let Some(d) = diagnostic(d, report.peer) {
                            d.last_protocol = Some(report.protocol.to_string());
                            d.protocol_outcome = Some(report.outcome);
                        } });
                    },
                    Some(Dial { peer, address, reply, reserve }) = dials.recv() => {
                        let result = (|| {
                            discovered.remember(peer, address.clone())?;
                            swarm.add_peer_address(peer, address.clone());
                            if reserve {
                                if !reservations.contains_key(&peer) {
                                    ensure!(reservations.len() < 3, "Relay reservation limit reached");
                                    let id = swarm.listen_on(address.with(libp2p::multiaddr::Protocol::P2p(peer)).with(libp2p::multiaddr::Protocol::P2pCircuit))?;
                                    reservations.insert(peer, id);
                                }
                            } else if connection_state(&connections, peer) == ConnectionState::Offline || (!is_relayed(&address) && connection_state(&connections, peer) == ConnectionState::Relay) {
                                let condition = if connection_state(&connections, peer) == ConnectionState::Relay { PeerCondition::Always } else { PeerCondition::DisconnectedAndNotDialing };
                                swarm.dial(DialOpts::peer_id(peer).condition(condition).addresses(vec![address.with(libp2p::multiaddr::Protocol::P2p(peer))]).build())?;
                                diagnostic_tx.send_modify(|d| {
                                    if let Some(d) = diagnostic(d, peer) {
                                        if !swarm.is_connected(&peer) { d.state = ConnectionState::Connecting; }
                                        d.last_failure = None;
                                    }
                                });
                            }
                            Ok(())
                        })();
                        let _ = reply.send(result);
                    },
                    Some((peer, stream)) = incoming.next() => {
                        if let Ok(permit) = incoming_slots.clone().try_acquire_owned() {
                            let state = shared.clone();
                            actor_tasks.spawn(async move { let _permit = permit; let _ = serve_control(state, peer, stream).await; });
                        }
                    },
                    Some((peer, stream, relayed)) = files.next() => {
                        if let Ok(permit) = incoming_file_slots.clone().try_acquire_owned() {
                            let state = shared.clone(); let limits = limits.clone();
                            actor_tasks.spawn(async move { let _permit = permit; let _ = serve_file(state, limits, peer, stream, relayed).await; });
                        }
                    },
                    Some((peer, stream)) = contacts.next() => {
                        if let Ok(permit) = incoming_slots.clone().try_acquire_owned() {
                            let state = shared.clone();
                            actor_tasks.spawn(async move { let _permit = permit; let _ = serve_contacts(state, peer, stream).await; });
                        }
                    },
                    event = swarm.select_next_some() => match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            if is_relayed(&address) {
                                relay_tx.send_modify(|a| { if a.len() < 3 && !a.contains(&address) { a.push(address.clone()); } });
                            } else if public_address(&address) {
                                swarm.behaviour_mut().autonat.on_swarm_event(FromSwarm::NewExternalAddrCandidate(NewExternalAddrCandidate { addr: &address }));
                            }
                            listeners.push(address); let _ = address_tx.send(listeners.clone());
                            let mut e = shared.lock().await; e.network_status = if discovery { "LAN discovery is running" } else { "Listening for direct connections" }.into(); e.emit();
                        },
                        SwarmEvent::ExpiredListenAddr { address, .. } => {
                            listeners.retain(|a| a != &address); let _ = address_tx.send(listeners.clone());
                            relay_tx.send_modify(|a| a.retain(|a| a != &address));
                            if public.remove(&address) {
                                swarm.remove_external_address(&address);
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability { status: if addresses.is_empty() { ReachabilityStatus::Unknown } else { ReachabilityStatus::Public }, public_addresses: addresses });
                            }
                        },
                        SwarmEvent::ExternalAddrExpired { address } => {
                            if public.remove(&address) {
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability { status: if addresses.is_empty() { ReachabilityStatus::Unknown } else { ReachabilityStatus::Public }, public_addresses: addresses });
                            }
                        },
                        SwarmEvent::ListenerClosed { listener_id, addresses, .. } => {
                            reservations.retain(|_, id| *id != listener_id);
                            listeners.retain(|a| !addresses.contains(a)); let _ = address_tx.send(listeners.clone());
                            relay_tx.send_modify(|a| a.retain(|a| !addresses.contains(a)));
                            let mut withdrawn = false;
                            for address in &addresses { if public.remove(address) { swarm.remove_external_address(address); withdrawn = true; } }
                            if withdrawn {
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability { status: if addresses.is_empty() { ReachabilityStatus::Unknown } else { ReachabilityStatus::Public }, public_addresses: addresses });
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => for (peer, address) in peers {
                            if discovered.remember(peer, address.clone()).is_ok_and(|added| added) {
                                swarm.add_peer_address(peer, address);
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                            if info.public_key.to_peer_id() == peer_id {
                                let observed = direct_address(peer, info.observed_addr).ok().filter(public_address);
                                diagnostic_tx.send_modify(|d| {
                                    if let Some(d) = diagnostic(d, peer_id) {
                                        d.identified = true;
                                        d.supports_contacts = info.protocols.contains(&CONTACT);
                                        d.observed_address = observed;
                                        d.advertised_addresses = info.listen_addrs.iter().take(MAX_ADDRESSES).cloned().collect();
                                    }
                                });
                                // An authenticated remote may still be malicious. Learn public
                                // hints only for already known peers; every request still authorizes.
                                if discovered.contains(&peer_id) {
                                    for address in info.listen_addrs.into_iter().take(MAX_ADDRESSES) {
                                        if let Ok(address) = direct_address(peer_id, address) {
                                            if public_address(&address) && discovered.remember(peer_id, address.clone()).is_ok_and(|added| added) {
                                                swarm.add_peer_address(peer_id, address);
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Autonat(event)) => {
                            #[cfg(test)]
                            if event.result.is_ok() { nat_confirmations_tx.send_modify(|count| *count += 1); }
                            // The library requires a matching nonce on an inbound dial-back,
                            // not just a server's claim that an address is reachable.
                            if let Ok(address) = direct_address(peer, event.tested_addr) {
                                public.record(event.server, address.clone(), event.result.is_ok(), Instant::now());
                                if event.result.is_err() { swarm.remove_external_address(&address); }
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability {
                                    status: if !addresses.is_empty() { ReachabilityStatus::Public } else if event.result.as_ref().is_err_and(|error| !error.is_inconclusive()) { ReachabilityStatus::Unreachable } else { ReachabilityStatus::Unknown },
                                    public_addresses: addresses,
                                });
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Holepunch(event)) => {
                            diagnostic_tx.send_modify(|d| { if let Some(d) = diagnostic(d, event.remote_peer_id) { d.hole_punch_succeeded = Some(event.result.is_ok()); } });
                            // Failure leaves the authenticated circuit and current streams intact.
                        },
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                            connections.insert(connection_id, (peer_id, endpoint.is_relayed()));
                            diagnostic_tx.send_modify(|d| {
                                if let Some(d) = diagnostic(d, peer_id) {
                                    d.state = connection_state(&connections, peer_id);
                                    d.last_failure = None;
                                }
                            });
                        },
                        SwarmEvent::OutgoingConnectionError { peer_id: Some(peer_id), error, .. } => {
                            diagnostic_tx.send_modify(|d| {
                                if let Some(d) = diagnostic(d, peer_id) {
                                    if !swarm.is_connected(&peer_id) { d.state = ConnectionState::Offline; }
                                    d.last_failure = Some(if matches!(error, DialError::WrongPeerId { .. }) { DialFailure::WrongPeer } else { DialFailure::Unreachable });
                                }
                            });
                        },
                        SwarmEvent::ConnectionClosed { peer_id, connection_id, num_established, .. } => {
                            connections.remove(&connection_id);
                            if num_established != 0 {
                                diagnostic_tx.send_modify(|d| { if let Some(d) = diagnostic(d, peer_id) { d.state = connection_state(&connections, peer_id); } });
                                continue;
                            }
                            let expired = public.expire(Some(peer_id), Instant::now());
                            for address in &expired { swarm.remove_external_address(address); }
                            if !expired.is_empty() {
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability { status: if addresses.is_empty() { ReachabilityStatus::Unknown } else { ReachabilityStatus::Public }, public_addresses: addresses });
                            }
                            diagnostic_tx.send_modify(|d| {
                                if let Some(d) = diagnostic(d, peer_id) {
                                    d.state = ConnectionState::Offline;
                                    d.identified = false;
                                    d.observed_address = None;
                                    d.advertised_addresses.clear();
                                    d.supports_contacts = false;
                                }
                            });
                            let mut e = shared.lock().await; e.online.remove(&peer_id.to_string()); e.remote.retain(|_,m| m.owner_peer_id != peer_id.to_string());
                            let cancelled: Vec<_> = e.transfers.iter().filter(|(_, t)| t.peer_id == peer_id.to_string()).map(|(id, _)| *id).collect();
                            for id in cancelled {
                                if let Some(c) = e.cancellations.get(&id) { c.cancel();
                                    if let Some(t) = e.transfers.get_mut(&id) { if t.relayed { t.error = Some(RELAY_INTERRUPTED.into()); } }
                                }
                            }
                            e.emit();
                        },
                        SwarmEvent::ListenerError { listener_id, .. } => {
                            let configured_relay = reservations.values().any(|id| *id == listener_id);
                            if configured_relay { swarm.remove_listener(listener_id); reservations.retain(|_, id| *id != listener_id); }
                            let message = if configured_relay { "Configured relay is unavailable or has no capacity; direct and LAN routes can still work" } else { "Local listener failed; restart Dump and check Windows firewall" };
                            shared.lock().await.error(message);
                        },
                        _ => {},
                    },
                    _ = tick.tick() => {
                        let expired = public.expire(None, Instant::now());
                        for address in &expired { swarm.remove_external_address(address); }
                        if !expired.is_empty() {
                            let addresses = public.addresses();
                            reachability_tx.send_replace(Reachability { status: if addresses.is_empty() { ReachabilityStatus::Unknown } else { ReachabilityStatus::Public }, public_addresses: addresses });
                        }
                        let mut e = shared.lock().await;
                        e.relay_peers = e.online.keys().filter_map(|p| p.parse().ok()).filter(|p| connection_state(&connections, *p) == ConnectionState::Relay).map(|p: PeerId| p.to_string()).collect();
                        let mut own_addresses: Vec<_> = reachability_tx.borrow().public_addresses.iter().chain(listeners.iter()).filter(|a| !is_relayed(a) && public_address(a)).filter_map(|a| peer_address(peer, a.clone()).ok()).map(|a| a.to_string()).collect();
                        own_addresses.sort(); own_addresses.dedup(); own_addresses.truncate(MAX_ADDRESSES - 3);
                        own_addresses.extend(relay_tx.borrow().iter().filter(|a| public_address(a)).filter_map(|a| peer_address(peer, a.clone()).ok()).map(|a| a.to_string()));
                        own_addresses.sort(); own_addresses.dedup(); own_addresses.truncate(MAX_ADDRESSES);
                        if let Err(err) = e.update_contact(own_addresses) { e.error(format!("Cannot save contact routes: {err}")); }
                        e.online.retain(|_,seen| now().saturating_sub(*seen) < 15);
                        let active_peers: BTreeSet<_> = e.online.keys().cloned().collect();
                        e.remote.retain(|_,m| active_peers.contains(&m.owner_peer_id));
                        e.pending.retain(|_,p| now().saturating_sub(p.requested_at) < 86400);
                        let joining = e.persisted.joining.clone();
                        let targets = e.active_snapshot().ok().filter(|s| s.contains(&e.peer())).map(|s| s.members.into_iter().filter_map(|m| m.peer_id.parse::<PeerId>().ok()).filter(|p| *p != peer).collect::<Vec<_>>()).unwrap_or_default();
                        let mut desired = targets.clone();
                        if let Some(invite) = &joining { if let Ok(id) = invite.owner_peer_id.parse() { desired.push(id); } }
                        desired.sort(); desired.dedup();
                        attempts.retain(|p, _| desired.contains(p));
                        for target in &desired {
                            let mut routes = discovered.routes(target);
                            if let Some(contact) = e.persisted.contacts.get(&target.to_string()) { if let Ok(contact_routes) = contact.routes_with_policy(allow_loopback) { routes.extend(contact_routes); } }
                            if let Some(contact) = joining.as_ref().and_then(|i| i.contact.as_ref()).filter(|c| c.peer_id == target.to_string()) { if let Ok(contact_routes) = contact.routes_with_policy(allow_loopback) { routes.extend(contact_routes); } }
                            let routes = preferred_routes(*target, routes);
                            if routes.is_empty() { continue; }
                            let state = connection_state(&connections, *target);
                            let available: Vec<_> = routes.into_iter().filter(|a| state != ConnectionState::Relay || !is_relayed(a)).collect();
                            if state == ConnectionState::Direct || available.is_empty() { continue; }
                            if let Some((at, _)) = attempts.get(target) { if at.elapsed() < Duration::from_secs(if state == ConnectionState::Relay { 30 } else { 10 }) { continue; } }
                            let index = attempts.get(target).map_or(0, |(_, index)| *index) % available.len();
                            let address = available[index].clone();
                            {
                                let condition = if state == ConnectionState::Relay { PeerCondition::Always } else { PeerCondition::DisconnectedAndNotDialing };
                                let _ = swarm.dial(DialOpts::peer_id(*target).condition(condition).addresses(vec![address.with(libp2p::multiaddr::Protocol::P2p(*target))]).build());
                                diagnostic_tx.send_modify(|d| { if let Some(d) = diagnostic(d, *target) { if state == ConnectionState::Offline { d.state = ConnectionState::Connecting; } } });
                            }
                            attempts.insert(*target, (Instant::now(), index + 1));
                        }
                        e.network_status = if !e.online.is_empty() { if e.online.keys().filter_map(|p| p.parse().ok()).any(|p| connection_state(&connections, p) == ConnectionState::Direct) { "Connected directly" } else { "Connected via relay" } } else if joining.is_some() || desired.iter().any(|p| diagnostic_tx.borrow().get(p).is_some_and(|d| d.state == ConnectionState::Connecting)) { "Connecting…" } else if listeners.is_empty() || !desired.is_empty() { "Offline" } else if discovery { "LAN discovery is running" } else { "Offline" }.into();
                        e.emit(); drop(e);
                        for target in targets.into_iter().filter(|p| connection_state(&connections, *p) != ConnectionState::Offline) {
                            let mut ongoing = in_flight.lock().await;
                            if ongoing.insert(target) {
                                let node = poll_node.clone(); let ongoing = in_flight.clone();
                                actor_tasks.spawn(async move { let _ = node.refresh_peer(target).await; ongoing.lock().await.remove(&target); });
                            }
                        }
                        if let Some(invite) = joining {
                            if now() >= last_join + 4 {
                                last_join = now();
                                if let Ok(target) = invite.owner_peer_id.parse::<PeerId>() {
                                    if connection_state(&connections, target) != ConnectionState::Offline {
                                        let mut ongoing = in_flight.lock().await;
                                        if ongoing.insert(target) { let node = poll_node.clone(); let ongoing = in_flight.clone(); actor_tasks.spawn(async move { let _ = node.poll_join(invite).await; ongoing.lock().await.remove(&target); }); }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let mut e = shared.lock().await;
            for token in e.cancellations.values() {
                token.cancel();
            }
            e.online.clear();
            e.remote.clear();
            e.relay_peers.clear();
            e.network_status = "Offline".into();
            e.emit();
            diagnostic_tx.send_modify(|d| d.clear());
            reachability_tx.send_replace(Reachability::default());
            relay_tx.send_replace(Vec::new());
        });
        let relay_node = node.clone();
        let configured = node.shared.lock().await.persisted.network.relays.clone();
        if !configured.is_empty() {
            tasks.spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(15));
                let mut rotation = 0;
                loop {
                    tokio::select! { _ = relay_node.shutdown.cancelled() => break, _ = tick.tick() => {} }
                    for (peer, address) in crate::relay_host::relay_choices(&configured, rotation) {
                        if relay_node.reserve_relay(peer, address).await.is_err() {
                            relay_node.shared.lock().await.error("Configured relay could not be reserved. Check Advanced connectivity; LAN and direct routes remain available.");
                        }
                    }
                    rotation = (rotation + 1) % configured.len();
                }
            });
        }
        let refresh_shared = node.shared.clone();
        let refresh_shutdown = node.shutdown.clone();
        tasks.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! { _ = refresh_shutdown.cancelled() => break, _ = tick.tick() => {} }
                let shares: Vec<_> = {
                    let e = refresh_shared.lock().await;
                    e.persisted
                        .shares
                        .values()
                        .filter(|s| Some(s.manifest.workspace_id) == e.persisted.active)
                        .cloned()
                        .collect()
                };
                for share in shares {
                    let checked = share.clone();
                    let unchanged = tokio::task::spawn_blocking(move || {
                        checked.available
                            && open_source(&checked.path)
                                .and_then(|f| stamp(&f))
                                .is_ok_and(|s| s == checked.stamp)
                    })
                    .await
                    .unwrap_or(false);
                    if !unchanged {
                        let _ = refresh_share(refresh_shared.clone(), share).await;
                    }
                }
            }
        });
        Ok(node)
    }
    pub async fn connect(&self, peer: PeerId, address: Multiaddr) -> Result<()> {
        let address = peer_address(peer, address)?;
        let (reply, registered) = oneshot::channel();
        self.dial
            .send(Dial {
                peer,
                address,
                reply,
                reserve: false,
            })
            .await?;
        // Acknowledges registration, not connection success; consult diagnostics for that.
        tokio::time::timeout(PREAUTH_TIMEOUT, registered).await??
    }

    /// Configurable relay client API. No relay is contacted unless explicitly supplied.
    pub async fn reserve_relay(&self, peer: PeerId, address: Multiaddr) -> Result<()> {
        let address = direct_address(peer, address)?;
        let (reply, registered) = oneshot::channel();
        self.dial
            .send(Dial {
                peer,
                address,
                reply,
                reserve: true,
            })
            .await?;
        tokio::time::timeout(PREAUTH_TIMEOUT, registered).await??
    }

    pub fn stream_control(&self, peer: PeerId) -> Control {
        if self
            .diagnostics
            .borrow()
            .get(&peer)
            .is_some_and(|d| d.state == ConnectionState::Relay)
        {
            self.relay_control.clone()
        } else {
            self.control.clone()
        }
    }
    pub async fn request(&self, peer: PeerId, request: &Request) -> Result<Response> {
        let mut stream = self.open_stream(peer, CONTROL).await?;
        write_frame(&mut stream, request).await?;
        read_frame(&mut stream).await
    }
    async fn open_stream(&self, peer: PeerId, protocol: StreamProtocol) -> Result<Stream> {
        let relayed = self
            .diagnostics
            .borrow()
            .get(&peer)
            .is_some_and(|d| d.state == ConnectionState::Relay);
        self.open_stream_on_route(peer, protocol, relayed).await
    }
    async fn open_stream_on_route(
        &self,
        peer: PeerId,
        protocol: StreamProtocol,
        relayed: bool,
    ) -> Result<Stream> {
        let mut control = if relayed {
            self.relay_control.clone()
        } else {
            self.control.clone()
        };
        let result =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, protocol.clone()))
                .await;
        let outcome = match &result {
            Ok(Ok(_)) => ProtocolOutcome::Negotiated,
            Ok(Err(libp2p_stream::OpenStreamError::UnsupportedProtocol(_))) => {
                ProtocolOutcome::Unsupported
            }
            Ok(Err(_)) => ProtocolOutcome::Io,
            Err(_) => ProtocolOutcome::Timeout,
        };
        // Bounded diagnostics contain protocol/outcome only, never request payloads.
        let _ = self.protocol_reports.try_send(ProtocolReport {
            peer,
            protocol,
            outcome,
            #[cfg(test)]
            probe: None,
        });
        Ok(result??)
    }
    pub async fn shutdown_and_wait(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
    async fn poll_join(&self, invitation: Invitation) -> Result<()> {
        let peer = invitation.owner_peer_id.parse()?;
        let name = self.shared.lock().await.persisted.device_name.clone();
        match self
            .request(
                peer,
                &Request::Join {
                    invitation: invitation.clone(),
                    name,
                },
            )
            .await?
        {
            Response::Approved { workspace } => {
                workspace.snapshot.verify()?;
                let mut e = self.shared.lock().await;
                ensure!(
                    workspace.snapshot.workspace_id == invitation.workspace_id
                        && workspace.snapshot.owner_peer_id == invitation.owner_peer_id
                        && workspace.snapshot.contains(&e.peer()),
                    "Invalid approval"
                );
                ensure!(
                    e.persisted
                        .joining
                        .as_ref()
                        .is_some_and(|i| i.token == invitation.token),
                    "Join was cancelled"
                );
                let mut next = e.persisted.clone();
                next.active = Some(invitation.workspace_id);
                next.joining = None;
                if let Some(old) = next.workspaces.get(&invitation.workspace_id) {
                    ensure!(
                        workspace.snapshot.owner_peer_id == old.snapshot.owner_peer_id
                            && workspace.snapshot.owner_public_key == old.snapshot.owner_public_key,
                        "Workspace owner changed"
                    );
                    ensure!(
                        workspace.snapshot.revision >= old.snapshot.revision,
                        "Stale approval"
                    );
                    if workspace.snapshot.revision == old.snapshot.revision {
                        ensure!(
                            workspace.snapshot == old.snapshot,
                            "Conflicting membership revision"
                        );
                    }
                }
                next.workspaces.insert(invitation.workspace_id, workspace);
                e.persist(next)?;
                e.reset_runtime();
                e.emit();
            }
            Response::Denied => {
                let mut e = self.shared.lock().await;
                if e.persisted
                    .joining
                    .as_ref()
                    .is_some_and(|i| i.token == invitation.token)
                {
                    e.cancel_join()?;
                    e.error("Invitation was rejected, expired, or already used. Ask the owner for a new invite.");
                }
            }
            Response::Waiting => {}
            _ => bail!("Invalid join response"),
        }
        Ok(())
    }
    pub async fn refresh_peer(&self, peer: PeerId) -> Result<()> {
        let mut snapshot = self.shared.lock().await.active_snapshot()?;
        let workspace = snapshot.workspace_id;
        let mut offset = 0;
        let mut received = Vec::new();
        let mut expected_total = None;
        loop {
            match self
                .request(
                    peer,
                    &Request::Catalog {
                        snapshot: snapshot.clone(),
                        offset,
                    },
                )
                .await?
            {
                Response::Catalog {
                    snapshot: latest,
                    files,
                    total,
                } => {
                    ensure!(
                        latest.workspace_id == workspace
                            && files.len() <= PAGE_SIZE
                            && total <= 10000,
                        "Invalid catalog"
                    );
                    if let Some(expected) = expected_total {
                        ensure!(total == expected, "Catalog changed; retry");
                    } else {
                        expected_total = Some(total);
                    }
                    ensure!(
                        offset <= total
                            && files.len() as u32 <= total - offset
                            && (offset == total || !files.is_empty()),
                        "Invalid catalog page"
                    );
                    {
                        let mut e = self.shared.lock().await;
                        ensure!(e.persisted.active == Some(workspace), "Workspace changed");
                        e.accept_snapshot(&latest)?;
                        ensure!(
                            latest.contains(&peer.to_string()) && latest.contains(&e.peer()),
                            "Access denied"
                        );
                    }
                    snapshot = latest;
                    for manifest in &files {
                        manifest.verify()?;
                        ensure!(
                            manifest.owner_peer_id == peer.to_string()
                                && manifest.workspace_id == workspace,
                            "Invalid file owner"
                        );
                    }
                    offset += files.len() as u32;
                    received.extend(files);
                    if offset >= total {
                        break;
                    }
                    ensure!(offset > 0 && received.len() < 10000, "Invalid catalog page");
                    // Stay below the serving peer's request budget, leaving room for transfers.
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                Response::Denied => bail!("Access denied"),
                _ => bail!("Invalid catalog response"),
            }
        }
        let mut e = self.shared.lock().await;
        ensure!(
            e.persisted.active == Some(workspace)
                && e.active_snapshot()?.contains(&peer.to_string())
                && e.active_snapshot()?.contains(&e.peer()),
            "Workspace changed"
        );
        e.remote.retain(|_, m| m.owner_peer_id != peer.to_string());
        for manifest in received {
            e.remote.insert(manifest.file_id, manifest);
        }
        e.online.insert(peer.to_string(), now());
        e.emit();
        drop(e);
        if self
            .diagnostics
            .borrow()
            .get(&peer)
            .is_some_and(|d| d.supports_contacts)
        {
            self.exchange_contacts(peer).await?;
        }
        Ok(())
    }
    async fn exchange_contacts(&self, peer: PeerId) -> Result<()> {
        let (snapshot, contact) = {
            let e = self.shared.lock().await;
            (
                e.active_snapshot()?,
                e.persisted
                    .contacts
                    .get(&e.peer())
                    .filter(|c| c.verify().is_ok())
                    .cloned(),
            )
        };
        let mut offset = 0;
        let mut expected = None;
        loop {
            let mut stream = self.open_stream(peer, CONTACT).await?;
            write_frame(
                &mut stream,
                &ContactRequest {
                    snapshot: snapshot.clone(),
                    contact: if offset == 0 { contact.clone() } else { None },
                    offset,
                },
            )
            .await?;
            let response: ContactResponse = read_frame(&mut stream).await?;
            ensure!(
                response.total <= MAX_MEMBERS as u32
                    && response.contacts.len() <= 8
                    && offset <= response.total
                    && response.contacts.len() as u32 <= response.total - offset
                    && (offset == response.total || !response.contacts.is_empty()),
                "Invalid contact page"
            );
            ensure!(
                expected.is_none_or(|total| total == response.total),
                "Contact list changed; retry"
            );
            expected = Some(response.total);
            offset += response.contacts.len() as u32;
            {
                let mut e = self.shared.lock().await;
                let current = e.authorize(&peer.to_string(), &snapshot)?;
                e.cache_contacts(&current, response.contacts)?;
            }
            if offset == response.total {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(())
    }
    pub async fn receive(&self, file_id: Uuid, directory: PathBuf) -> Result<Uuid> {
        ensure!(!self.shutdown.is_cancelled(), "Node is shutting down");
        ensure!(directory.is_dir(), "Choose an existing destination folder");
        let (manifest, id, cancel) = {
            let mut e = self.shared.lock().await;
            ensure!(e.cancellations.len() < 100, "Transfer queue is full");
            let manifest = e
                .remote
                .get(&file_id)
                .context("File is no longer available")?
                .clone();
            let (id, cancel) = e.register_transfer(&manifest, &manifest.owner_peer_id, "Receiving");
            (manifest, id, cancel)
        };
        let node = self.clone();
        self.tasks.spawn(async move {
            // Disk operations cannot be cancelled safely: finish them before cleanup.
            let mut created = false;
            let result = node
                .receive_inner(&manifest, &directory, id, &cancel, &mut created)
                .await;
            let part = directory.join(format!(".dump-{id}.part"));
            let mut e = node.shared.lock().await;
            let bytes = e.transfers.get(&id).map_or(0, |t| t.bytes);
            let cleaned = if result.is_err() {
                remove_owned_partial(&part, created).await
            } else {
                true
            };
            let mut next = e.persisted.clone();
            if cleaned {
                next.partials.retain(|p| p != &part);
            } else if created && !next.partials.contains(&part) {
                next.partials.push(part.clone());
            }
            if !cleaned {
                e.error(PARTIAL_CLEANUP_FAILED);
            }
            if let Err(err) = e.persist(next) {
                e.error(format!("Cannot save download state: {err}"));
            }
            match result {
                Ok(()) => e.mark_transfer(id, "Completed", manifest.size, None),
                Err(err) => {
                    let reason = e
                        .transfers
                        .get(&id)
                        .and_then(|t| t.error.clone())
                        .filter(|_| cancel.is_cancelled())
                        .unwrap_or_else(|| err.to_string());
                    e.mark_transfer(
                        id,
                        if cancel.is_cancelled() {
                            "Cancelled"
                        } else {
                            "Failed"
                        },
                        bytes,
                        Some(reason),
                    )
                }
            }
        });
        Ok(id)
    }
    async fn receive_inner(
        &self,
        manifest: &Manifest,
        directory: &Path,
        id: Uuid,
        cancel: &CancellationToken,
        created: &mut bool,
    ) -> Result<()> {
        manifest.verify()?;
        let peer: PeerId = manifest.owner_peer_id.parse()?;
        let peer_slots = self.limits.peer(peer, false).await?;
        let _peer = tokio::select! {
            _ = cancel.cancelled() => bail!("Transfer cancelled"),
            permit = peer_slots.acquire_owned() => permit?,
        };
        let _global = tokio::select! {
            _ = cancel.cancelled() => bail!("Transfer cancelled"),
            permit = self.limits.receive.clone().acquire_owned() => permit?,
        };
        let snapshot = {
            let e = self.shared.lock().await;
            ensure!(
                e.persisted.active == Some(manifest.workspace_id),
                "Workspace changed"
            );
            let snapshot = e.active_snapshot()?;
            ensure!(
                snapshot.contains(&manifest.owner_peer_id) && snapshot.contains(&e.peer()),
                "Access denied"
            );
            snapshot
        };
        let relayed = self
            .diagnostics
            .borrow()
            .get(&peer)
            .is_some_and(|d| d.state == ConnectionState::Relay);
        if let Some(t) = self.shared.lock().await.transfers.get_mut(&id) {
            t.relayed = relayed;
        }
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => bail!("Transfer cancelled"),
            stream = self.open_stream_on_route(peer, FILE, relayed) => stream.map_err(|e| relay_error(e, relayed))?,
        };
        write_frame(
            &mut stream,
            &Request::File {
                snapshot,
                manifest: manifest.clone(),
            },
        )
        .await
        .map_err(|e| relay_error(e, relayed))?;
        match read_frame(&mut stream)
            .await
            .map_err(|e| relay_error(e, relayed))?
        {
            Response::File { manifest: actual } => {
                ensure!(
                    actual == *manifest,
                    "File version changed; refresh the file list"
                );
                actual.verify()?;
            }
            _ => bail!("File unavailable or access denied"),
        }
        let destination = directory.join(&manifest.name);
        ensure!(
            !destination.exists(),
            "A file with that name already exists; choose a different folder"
        );
        let part = directory.join(format!(".dump-{id}.part"));
        ensure!(!cancel.is_cancelled(), "Transfer cancelled");
        let mut file = create_owned_partial(&self.shared, &part, id, created).await?;
        let mut remaining = manifest.size;
        let mut total = 0;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; BLOCK];
        let mut last = Instant::now();
        while remaining > 0 {
            let wanted = remaining.min(BLOCK as u64) as usize;
            tokio::select! {
                _ = cancel.cancelled() => bail!("Transfer cancelled"),
                read = tokio::time::timeout(INACTIVITY, stream.read_exact(&mut buffer[..wanted])) => read.map_err(anyhow::Error::from).and_then(|r| r.map_err(Into::into)).map_err(|e| relay_error(e, relayed))?,
            };
            DiskWrite::write_all(&mut file, &buffer[..wanted]).await?;
            hash.update(&buffer[..wanted]);
            remaining -= wanted as u64;
            total += wanted as u64;
            #[cfg(test)]
            if let Some(pause) = self.limits.receive_pause.lock().await.take() {
                self.shared
                    .lock()
                    .await
                    .mark_transfer(id, "Receiving", total, None);
                let _ = pause.started.send(());
                tokio::select! {
                    _ = cancel.cancelled() => bail!("Transfer cancelled"),
                    resumed = pause.resume => resumed?,
                }
            }
            if last.elapsed() >= Duration::from_millis(200) {
                self.shared
                    .lock()
                    .await
                    .mark_transfer(id, "Receiving", total, None);
                last = Instant::now();
            }
        }
        self.shared
            .lock()
            .await
            .mark_transfer(id, "Verifying", total, None);
        ensure!(
            matches!(
                tokio::select! {
                    _ = cancel.cancelled() => bail!("Transfer cancelled"),
                    response = read_frame::<_, Response>(&mut stream) => response.map_err(|e| relay_error(e, relayed))?,
                },
                Response::Done
            ),
            "Sender could not verify source file"
        );
        ensure!(
            hex::encode(hash.finalize()) == manifest.sha256,
            "File integrity verification failed"
        );
        file.sync_all().await?;
        drop(file);
        // Serialize completion with membership/unshare cancellation so a known removal cannot race commit.
        let e = self.shared.lock().await;
        ensure!(
            !cancel.is_cancelled()
                && e.persisted.active == Some(manifest.workspace_id)
                && e.active_snapshot()?.contains(&manifest.owner_peer_id)
                && e.active_snapshot()?.contains(&e.peer()),
            "Transfer no longer authorized"
        );
        commit_download(&part, &destination)?;
        Ok(())
    }
}

pub async fn serve_control(shared: Shared, peer: PeerId, mut stream: Stream) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request = read_frame::<_, Request>(&mut stream).await?;
    let response = handle_control(shared, peer, request)
        .await
        .unwrap_or(Response::Denied);
    write_frame(&mut stream, &response).await?;
    stream.close().await?;
    Ok(())
}
async fn serve_contacts(shared: Shared, peer: PeerId, mut stream: Stream) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request: ContactRequest = read_frame(&mut stream).await?;
    let response = {
        let mut e = shared.lock().await;
        let snapshot = e.authorize(&peer.to_string(), &request.snapshot)?;
        ensure!(
            request.offset <= MAX_MEMBERS as u32,
            "Invalid contact offset"
        );
        if let Some(contact) = request.contact {
            ensure!(
                contact.peer_id == peer.to_string(),
                "Contact is not the authenticated sender"
            );
            e.cache_contacts(&snapshot, vec![contact])?;
        }
        let all: Vec<_> = e
            .persisted
            .contacts
            .values()
            .filter(|c| snapshot.contains(&c.peer_id) && c.verify().is_ok())
            .cloned()
            .collect();
        ContactResponse {
            total: all.len() as u32,
            contacts: all
                .into_iter()
                .skip(request.offset as usize)
                .take(8)
                .collect(),
        }
    };
    write_frame(&mut stream, &response).await?;
    stream.close().await?;
    Ok(())
}
pub async fn handle_control(shared: Shared, peer: PeerId, request: Request) -> Result<Response> {
    let mut e = shared.lock().await;
    match request {
        Request::Join { invitation, name } => {
            validate_label(&name)?;
            ensure!(
                invitation.version == VERSION
                    && invitation.token.len() <= 64
                    && invitation.owner_peer_id == e.peer()
                    && e.persisted.active == Some(invitation.workspace_id),
                "Access denied"
            );
            let hash = digest(invitation.token.as_bytes());
            let invite = e
                .persisted
                .invites
                .iter()
                .find(|i| {
                    i.workspace_id == invitation.workspace_id
                        && i.token_hash == hash
                        && i.expires_at > now()
                })
                .context("Access denied")?
                .clone();
            if let Some(approved) = invite.approved_peer {
                ensure!(approved == peer.to_string(), "Access denied");
                let workspace = e
                    .persisted
                    .workspaces
                    .get(&invitation.workspace_id)
                    .context("Access denied")?;
                ensure!(workspace.snapshot.contains(&approved), "Access denied");
                return Ok(Response::Approved {
                    workspace: workspace.clone(),
                });
            }
            ensure!(
                e.pending.len() < 32 || e.pending.contains_key(&peer.to_string()),
                "Access denied"
            );
            ensure!(peer.as_ref().code() == 0, "Unsupported peer identity");
            let public = libp2p::identity::PublicKey::try_decode_protobuf(peer.as_ref().digest())?;
            ensure!(public.to_peer_id() == peer, "Access denied");
            let pending = crate::engine::Pending {
                peer_id: peer.to_string(),
                name,
                workspace_id: invitation.workspace_id,
                token_hash: hash,
                fingerprint: digest(&public.encode_protobuf()),
                requested_at: now(),
            };
            e.pending.insert(peer.to_string(), pending);
            e.emit();
            Ok(Response::Waiting)
        }
        Request::Catalog { snapshot, offset } => {
            let current = e.authorize(&peer.to_string(), &snapshot)?;
            ensure!(offset <= 10000, "Invalid catalog offset");
            let all: Vec<_> = e
                .persisted
                .shares
                .values()
                .filter(|s| s.available && s.manifest.workspace_id == current.workspace_id)
                .map(|s| s.manifest.clone())
                .collect();
            ensure!(all.len() <= 10000, "Too many shared files");
            let total = all.len() as u32;
            let files = all
                .into_iter()
                .skip(offset as usize)
                .take(PAGE_SIZE)
                .collect();
            e.online.insert(peer.to_string(), now());
            Ok(Response::Catalog {
                snapshot: current,
                files,
                total,
            })
        }
        Request::File { .. } => bail!("Wrong protocol"),
    }
}

async fn serve_file(
    shared: Shared,
    limits: Arc<Limits>,
    peer: PeerId,
    mut stream: Stream,
    relayed: bool,
) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request: Request = read_frame(&mut stream).await?;
    let (manifest, share, id, cancel) = {
        let mut e = shared.lock().await;
        let result: Result<_> = (|| {
            let Request::File { snapshot, manifest } = request else {
                bail!("Wrong protocol")
            };
            e.authorize(&peer.to_string(), &snapshot)?;
            manifest.verify()?;
            ensure!(
                manifest.owner_peer_id == e.peer()
                    && manifest.workspace_id == snapshot.workspace_id,
                "Access denied"
            );
            let share = e
                .persisted
                .shares
                .get(&manifest.file_id)
                .context("Access denied")?
                .clone();
            ensure!(
                share.available && share.manifest == manifest,
                "File version changed"
            );
            ensure!(e.cancellations.len() < 100, "Transfer queue full");
            Ok((manifest, share))
        })();
        match result {
            Ok((manifest, share)) => {
                let (id, cancel) = e.register_transfer(&manifest, &peer.to_string(), "Sharing");
                if let Some(t) = e.transfers.get_mut(&id) {
                    t.relayed = relayed;
                }
                (manifest, share, id, cancel)
            }
            Err(_) => {
                drop(e);
                write_frame(&mut stream, &Response::Denied).await?;
                return Ok(());
            }
        }
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("Transfer cancelled")),
        result = async {
            let _peer = limits.peer(peer,true).await?.try_acquire_owned().context("Sender busy; retry shortly")?;
            let _global = limits.send.clone().try_acquire_owned().context("Sender busy; retry shortly")?;
            {
                let e = shared.lock().await;
                ensure!(e.persisted.active == Some(manifest.workspace_id) && e.active_snapshot()?.contains(&peer.to_string()) && e.persisted.shares.contains_key(&manifest.file_id),"Access denied");
            }
            let std_file = open_source(&share.path)?;
            ensure!(stamp(&std_file)? == share.stamp,"File changed; retry after refreshing");
            let mut file = tokio::fs::File::from_std(std_file);
            write_frame(&mut stream,&Response::File {manifest:manifest.clone()}).await.map_err(|e| relay_error(e, relayed))?;
            shared.lock().await.mark_transfer(id,"Sharing",0,None);
            let mut hash = Sha256::new(); let mut total = 0; let mut buffer = vec![0;BLOCK]; let mut last = Instant::now();
            while total < manifest.size {
                let wanted = (manifest.size-total).min(BLOCK as u64) as usize;
                tokio::time::timeout(INACTIVITY,DiskRead::read_exact(&mut file,&mut buffer[..wanted])).await??;
                tokio::time::timeout(INACTIVITY,stream.write_all(&buffer[..wanted])).await.map_err(anyhow::Error::from).and_then(|r| r.map_err(Into::into)).map_err(|e| relay_error(e, relayed))?;
                hash.update(&buffer[..wanted]); total += wanted as u64;
                if last.elapsed() >= Duration::from_millis(200) {shared.lock().await.mark_transfer(id,"Sharing",total,None); last = Instant::now();}
            }
            let mut extra = [0]; ensure!(DiskRead::read(&mut file,&mut extra).await? == 0 && hex::encode(hash.finalize()) == manifest.sha256,"Source file integrity changed");
            write_frame(&mut stream,&Response::Done).await.map_err(|e| relay_error(e, relayed))?; stream.close().await.map_err(|e| relay_error(e.into(), relayed))?;
            Ok::<(),anyhow::Error>(())
        } => result,
    };
    let mut e = shared.lock().await;
    let bytes = e.transfers.get(&id).map_or(0, |t| t.bytes);
    match result {
        Ok(()) => e.mark_transfer(id, "Completed", manifest.size, None),
        Err(err) => {
            let message = e
                .transfers
                .get(&id)
                .and_then(|t| t.error.clone())
                .filter(|_| cancel.is_cancelled())
                .unwrap_or_else(|| err.to_string());
            e.mark_transfer(
                id,
                if cancel.is_cancelled() {
                    "Cancelled"
                } else {
                    "Failed"
                },
                bytes,
                Some(message.clone()),
            );
            if message.contains("changed") {
                drop(e);
                let _ = refresh_share(shared, share).await;
            }
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
#[path = "network_identity_tests.rs"]
mod identity_tests;

#[cfg(all(test, windows))]
#[path = "network_reachability_tests.rs"]
mod reachability_tests;

#[cfg(all(test, windows))]
mod internet_tests {
    use super::*;
    use crate::{
        contact::Contact,
        engine::{share_paths, Engine},
    };
    use libp2p::swarm::Swarm;
    async fn wait(shared: &Shared, predicate: impl Fn(&crate::engine::View) -> bool) -> Result<()> {
        wait_stage("state transition", shared, None, predicate).await
    }
    async fn wait_stage(
        stage: &str,
        shared: &Shared,
        node: Option<&Node>,
        predicate: impl Fn(&crate::engine::View) -> bool,
    ) -> Result<()> {
        let (tx, mut changed) = watch::channel(());
        let previous = {
            let mut e = shared.lock().await;
            let previous = e.notify.clone();
            let notify = previous.clone();
            e.notify = Arc::new(move |view| {
                notify(view);
                tx.send_replace(());
            });
            previous
        };
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            loop {
                let view = shared.lock().await.view();
                if predicate(&view) {
                    break Ok(());
                }
                if let Some(t) = view
                    .transfers
                    .iter()
                    .find(|t| matches!(t.status.as_str(), "Failed" | "Cancelled"))
                {
                    bail!(
                        "stage={stage}: transfer {} at {}/{} bytes: {}",
                        t.status,
                        t.bytes,
                        t.total,
                        t.error.as_deref().unwrap_or("no reason")
                    );
                }
                changed.changed().await?;
            }
        })
        .await;
        shared.lock().await.notify = previous;
        if !matches!(result, Ok(Ok(()))) {
            let e = shared.lock().await;
            let view = e.view();
            eprintln!("stage={stage} elapsed={:?} joining={} pending={} files={} online={} last_error={:?}", started.elapsed(), view.joining, view.pending.len(), view.files.len(), view.online.len(), view.last_error);
            for t in &view.transfers {
                eprintln!(
                    "transfer status={} bytes={}/{} error={:?}",
                    t.status, t.bytes, t.total, t.error
                );
            }
            if let Some(node) = node {
                for d in node.diagnostics.borrow().values() {
                    eprintln!(
                        "route={:?} identify={} protocol={:?} outcome={:?} dial={:?}",
                        d.state, d.identified, d.last_protocol, d.protocol_outcome, d.last_failure
                    );
                }
                eprintln!(
                    "reservation locators={}",
                    node.relay_addresses.borrow().len()
                );
            }
        }
        eprintln!("stage={stage} elapsed={:?}", started.elapsed());
        result.with_context(|| format!("stage={stage}: state transition timed out"))?
    }
    async fn address<B: NetworkBehaviour>(swarm: &mut Swarm<B>) -> Result<Multiaddr> {
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                    break address;
                }
            }
        })
        .await
        .context("stage=relay listen");
        eprintln!("stage=relay listen elapsed={:?}", started.elapsed());
        result
    }

    #[derive(Default)]
    struct Cleanup {
        nodes: Vec<Node>,
        relay_stop: CancellationToken,
        host: Option<tokio::task::JoinHandle<()>>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for node in &self.nodes {
                node.shutdown.cancel();
            }
            self.relay_stop.cancel();
            if let Some(host) = &self.host {
                host.abort();
            }
        }
    }
    impl Cleanup {
        async fn finish(&mut self) -> Result<()> {
            for node in &self.nodes {
                node.shutdown.cancel();
            }
            self.relay_stop.cancel();
            for node in &self.nodes {
                tokio::time::timeout(Duration::from_secs(10), node.shutdown_and_wait())
                    .await
                    .context("stage=node cleanup")?;
            }
            if let Some(host) = self.host.take() {
                host.await?;
            }
            Ok(())
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn invite_locator_automatically_joins_and_transfers_and_contacts_stay_private(
    ) -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut cleanup = Cleanup::default();
        let result: Result<()> = async {
        let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
        let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
        owner.lock().await.create_workspace("Internet".into())?;
        let mut invite = Invitation::parse(&owner.lock().await.create_invite()?)?;
        let relay_key = libp2p::identity::Keypair::generate_ed25519();
        let relay_peer = relay_key.public().to_peer_id();
        let mut relay = crate::relay_host::swarm(relay_key, crate::relay_host::config())?;
        relay.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        let relay_address = address(&mut relay).await?;
        relay.add_external_address(relay_address.clone());
        let relay_stop = cleanup.relay_stop.clone();
        let stop = relay_stop.clone();
        cleanup.host = Some(tokio::spawn(async move {
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = relay.select_next_some() => {} }
            }
        }));
        let a = Node::start(owner.clone(), false).await?;
        cleanup.nodes.push(a.clone());
        let started = Instant::now();
        a.reserve_relay(relay_peer, relay_address).await?;
        let mut addresses = a.relay_addresses.clone();
        let circuit = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(address) = addresses.borrow().first().cloned() {
                    break Ok::<_, anyhow::Error>(address);
                }
                addresses.changed().await?;
            }
        })
        .await.context("stage=owner reservation/circuit locator")??;
        eprintln!("stage=owner reservation/circuit locator elapsed={:?} locators={}", started.elapsed(), a.relay_addresses.borrow().len());
        // Simulated relay address is injected only into this private unit harness.
        // Public invitation parsing and shipping Node::start always reject loopback contacts.
        invite.contact = Some(Contact::issue_routes(
            &owner.lock().await.key,
            1,
            vec![circuit.to_string()],
            true,
        )?);
        {
            let mut e = member.lock().await;
            let mut next = e.persisted.clone();
            next.joining = Some(invite);
            e.persist(next)?;
        }
        let b = Node::start_inner(member.clone(), false, true).await?;
        cleanup.nodes.push(b.clone());
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        let started = Instant::now();
        let mut diagnostics = b.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if diagnostics.borrow().get(&owner_peer).is_some_and(|d| d.state == ConnectionState::Relay) { break Ok::<_, anyhow::Error>(()); }
                diagnostics.changed().await?;
            }
        }).await.context("stage=member owner connection")??;
        eprintln!("stage=member owner connection elapsed={:?} route=Relay", started.elapsed());
        let member_peer = member.lock().await.peer();
        wait_stage("owner join request", &owner, Some(&b), |v| {
            v.pending.iter().any(|p| p.peer_id == member_peer)
        })
        .await?;
        owner.lock().await.approve(&member_peer)?;
        wait_stage("member approval", &member, Some(&b), |v| v.active.is_some() && !v.joining).await?;
        let payload = vec![23; 2 * BLOCK + 17];
        let source = root.path().join("via-invite.bin");
        tokio::fs::write(&source, &payload).await?;
        share_paths(owner.clone(), vec![source]).await?;
        wait_stage("member catalog/manifest", &member, Some(&b), |v| v.files.iter().any(|f| !f.mine)).await?;
        let file = member.lock().await.remote.values().next().unwrap().file_id;
        let output = root.path().join("output");
        tokio::fs::create_dir(&output).await?;
        let id = b.receive(file, output.clone()).await?;
        wait_stage("file transfer", &member, Some(&b), |v| {
            v.transfers
                .iter()
                .any(|t| t.id == id && t.status == "Completed")
        })
        .await?;
        ensure!(tokio::fs::read(output.join("via-invite.bin")).await? == payload, "stage=file verification: received bytes differ");
        let outsider = Engine::open(&root.path().join("outsider"), Arc::new(|_| {}))?;
        let c = Node::start(outsider, false).await?;
        cleanup.nodes.push(c.clone());
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        let route = a.relay_addresses.borrow()[0].clone();
        c.connect(owner_peer, route).await?;
        let mut diagnostics = c.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if diagnostics.borrow().get(&owner_peer).is_some_and(|d| d.state == ConnectionState::Relay) { break Ok::<_, anyhow::Error>(()); }
                diagnostics.changed().await?;
            }
        })
        .await.context("stage=outsider owner connection")??;
        let mut stream = c.open_stream(owner_peer, CONTACT).await.context("stage=outsider contact negotiation")?;
        let started = Instant::now();
        write_frame(
            &mut stream,
            &ContactRequest {
                snapshot: owner.lock().await.active_snapshot()?,
                contact: None,
                offset: 0,
            },
        )
        .await?;
        assert!(
            read_frame::<_, ContactResponse>(&mut stream).await.is_err(),
            "Nonmember must never receive contacts"
        );
        eprintln!("stage=outsider contact denied elapsed={:?}", started.elapsed());
        Ok(())
        }.await;
        let cleaned = cleanup.finish().await;
        result?;
        cleaned
    }

    #[derive(NetworkBehaviour)]
    struct BadProvider {
        streams: libp2p_stream::Behaviour,
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn authorized_provider_wrong_bytes_fail_final_hash_and_leave_no_partial() -> Result<()> {
        let root = tempfile::tempdir()?;
        let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
        let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
        owner.lock().await.create_workspace("Integrity".into())?;
        let invitation = Invitation::parse(&owner.lock().await.create_invite()?)?;
        let member_peer = member.lock().await.key.public().to_peer_id();
        assert!(matches!(
            handle_control(
                owner.clone(),
                member_peer,
                Request::Join {
                    invitation,
                    name: "Member".into()
                }
            )
            .await?,
            Response::Waiting
        ));
        owner.lock().await.approve(&member_peer.to_string())?;
        let workspace = {
            let e = owner.lock().await;
            e.persisted.workspaces[&e.persisted.active.unwrap()].clone()
        };
        {
            let mut e = member.lock().await;
            let mut next = e.persisted.clone();
            next.active = Some(workspace.snapshot.workspace_id);
            next.workspaces
                .insert(workspace.snapshot.workspace_id, workspace);
            e.persist(next)?;
        }
        let source = root.path().join("tampered.bin");
        tokio::fs::write(&source, b"expected bytes").await?;
        share_paths(owner.clone(), vec![source]).await?;
        let manifest = owner
            .lock()
            .await
            .persisted
            .shares
            .values()
            .next()
            .unwrap()
            .manifest
            .clone();
        let key = owner.lock().await.key.clone();
        let owner_peer = key.public().to_peer_id();
        let mut provider = SwarmBuilder::with_existing_identity(key)
            .with_tokio()
            .with_quic()
            .with_behaviour(|_| BadProvider {
                streams: libp2p_stream::Behaviour::new(),
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();
        provider.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        let provider_address = address(&mut provider).await?;
        let mut files = provider.behaviour().streams.new_control().accept(FILE)?;
        let served_manifest = manifest.clone();
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let bad = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_stop.cancelled() => break,
                    _ = provider.select_next_some() => {},
                    Some((_, mut stream)) = files.next() => {
                        let request = read_frame::<_, Request>(&mut stream).await?;
                        ensure!(matches!(request, Request::File { .. }), "Expected a file request");
                        write_frame(&mut stream, &Response::File { manifest: served_manifest.clone() }).await?;
                        stream.write_all(&vec![0; served_manifest.size as usize]).await?;
                        write_frame(&mut stream, &Response::Done).await?;
                        stream.close().await?;
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        });
        let b = Node::start(member.clone(), false).await?;
        b.connect(owner_peer, provider_address).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while b
                .diagnostics
                .borrow()
                .get(&owner_peer)
                .is_none_or(|d| d.state != ConnectionState::Direct)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        member
            .lock()
            .await
            .remote
            .insert(manifest.file_id, manifest.clone());
        let output = root.path().join("output");
        tokio::fs::create_dir(&output).await?;
        let id = b.receive(manifest.file_id, output.clone()).await?;
        wait(&member, |v| {
            v.transfers
                .iter()
                .any(|t| t.id == id && t.status == "Failed")
        })
        .await?;
        let e = member.lock().await;
        assert!(e.transfers[&id]
            .error
            .as_deref()
            .is_some_and(|s| s.contains("integrity verification failed")));
        assert!(e.persisted.partials.is_empty());
        drop(e);
        assert!(tokio::fs::read_dir(output)
            .await?
            .next_entry()
            .await?
            .is_none());
        b.shutdown.cancel();
        stop.cancel();
        bad.await??;
        Ok(())
    }
    #[test]
    fn relay_errors_preserve_direct_and_validation_failures() {
        let reset =
            || anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert!(relay_error(reset(), true)
            .to_string()
            .contains("may have been reached"));
        assert!(!relay_error(reset(), false).to_string().contains("Relay"));
        assert_eq!(
            relay_error(anyhow::anyhow!("File integrity verification failed"), true).to_string(),
            "File integrity verification failed"
        );
        assert_eq!(
            relay_error(anyhow::anyhow!("Access denied"), true).to_string(),
            "Access denied"
        );
    }
    #[tokio::test]
    async fn partial_creation_collision_never_deletes_an_unowned_file() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let shared = Engine::open(&state, Arc::new(|_| {}))?;
        let id = Uuid::new_v4();
        let part = directory.path().join(format!(".dump-{id}.part"));
        tokio::fs::write(&part, b"existing unrelated bytes").await?;
        let mut created = false;
        ensure!(
            create_owned_partial(&shared, &part, id, &mut created)
                .await
                .is_err(),
            "collision unexpectedly succeeded"
        );
        ensure!(
            !created && shared.lock().await.persisted.partials.is_empty(),
            "unowned partial was recorded"
        );
        assert!(remove_owned_partial(&part, created).await);
        let reopened = Engine::open(&state, Arc::new(|_| {}))?;
        ensure!(
            reopened.lock().await.persisted.partials.is_empty(),
            "unowned partial persisted"
        );
        ensure!(
            tokio::fs::read(&part).await? == b"existing unrelated bytes",
            "unowned file changed after restart"
        );
        let owned = directory
            .path()
            .join(format!(".dump-{}.part", Uuid::new_v4()));
        tokio::fs::write(&owned, b"Dump partial").await?;
        assert!(remove_owned_partial(&owned, true).await);
        assert!(!owned.exists());
        Ok(())
    }
    #[test]
    fn routes_prefer_lan_then_public_direct_then_circuit() -> Result<()> {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let relay = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let circuit: Multiaddr = format!("/ip4/8.8.8.8/tcp/42/p2p/{relay}/p2p-circuit").parse()?;
        let public: Multiaddr = "/ip4/1.1.1.1/udp/42/quic-v1".parse()?;
        let lan: Multiaddr = "/ip4/192.168.1.2/udp/42/quic-v1".parse()?;
        assert_eq!(
            preferred_routes(peer, vec![circuit.clone(), public.clone(), lan.clone()]),
            vec![lan, public, circuit]
        );
        Ok(())
    }
}
