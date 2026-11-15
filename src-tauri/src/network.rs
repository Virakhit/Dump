use crate::{
    engine::{open_source, refresh_share, stamp, Shared},
    model::*,
    storage::commit_download,
};
use anyhow::{bail, ensure, Context, Result};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
use libp2p::{
    connection_limits, mdns, ping,
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour, Stream, StreamProtocol, SwarmEvent},
    Multiaddr, PeerId, SwarmBuilder,
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
    sync::{mpsc, watch, Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const CONTROL: StreamProtocol = StreamProtocol::new("/dump/control/1");
const FILE: StreamProtocol = StreamProtocol::new("/dump/file/1");
const CHAT: StreamProtocol = StreamProtocol::new("/dump/chat/2");
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(5);
const INACTIVITY: Duration = Duration::from_secs(30);

#[path = "network_chat.rs"]
mod chat_network;
#[path = "network_host.rs"]
mod network_host;
#[path = "network_routes.rs"]
mod network_routes;

#[derive(NetworkBehaviour)]
struct Behaviour {
    streams: libp2p_stream::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    ping: ping::Behaviour,
    limits: connection_limits::Behaviour,
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
    chat_send: Arc<Semaphore>,
    send: Arc<Semaphore>,
    receive: Arc<Semaphore>,
    peers: Mutex<BTreeMap<PeerId, PeerSlots>>,
}
impl Limits {
    fn new() -> Self {
        Self {
            chat_send: Arc::new(Semaphore::new(4)),
            send: Arc::new(Semaphore::new(2)),
            receive: Arc::new(Semaphore::new(2)),
            peers: Mutex::new(BTreeMap::new()),
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

#[derive(Clone)]
pub struct Node {
    host: Arc<network_host::Hub>,
    own_peer: PeerId,
    discovery: bool,
    pub shared: Shared,
    pub control: Control,
    dial: mpsc::Sender<(PeerId, Multiaddr)>,
    pub addresses: watch::Receiver<Vec<Multiaddr>>,
    limits: Arc<Limits>,
    pub shutdown: CancellationToken,
}
impl Node {
    pub async fn start(shared: Shared, discovery: bool) -> Result<Self> {
        let key = shared.lock().await.key.clone();
        let peer = key.public().to_peer_id();
        let mdns = if discovery {
            Some(mdns::tokio::Behaviour::new(mdns::Config::default(), peer)?)
        } else {
            None
        };
        let mut swarm = SwarmBuilder::with_existing_identity(key)
            .with_tokio()
            .with_quic()
            .with_behaviour(|_| Behaviour {
                streams: libp2p_stream::Behaviour::new(),
                mdns: mdns.into(),
                ping: ping::Behaviour::new(
                    ping::Config::new()
                        .with_interval(Duration::from_secs(5))
                        .with_timeout(Duration::from_secs(5)),
                ),
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
        let mut control = swarm.behaviour().streams.new_control();
        let mut incoming = control.accept(CONTROL)?;
        let mut files = control.accept(FILE)?;
        let mut chats = control.accept(CHAT)?;
        let mut route_queries = network_routes::accept(&mut control)?;
        let (mut host_calls, mut host_sources) = network_host::accept(&mut control)?;
        let (dial, mut dials) = mpsc::channel(128);
        let (address_tx, addresses) = watch::channel(Vec::new());
        let limits = Arc::new(Limits::new());
        let shutdown = CancellationToken::new();
        let node = Self {
            host: Arc::new(network_host::Hub::new(
                shared.lock().await.relay_active.clone(),
            )),
            own_peer: peer,
            discovery,
            shared: shared.clone(),
            control: control.clone(),
            dial,
            addresses,
            limits: limits.clone(),
            shutdown: shutdown.clone(),
        };
        let poll_node = node.clone();
        let incoming_slots = Arc::new(Semaphore::new(32));
        let incoming_file_slots = Arc::new(Semaphore::new(8));
        let incoming_chat_slots = Arc::new(Semaphore::new(8));
        let incoming_route_slots = Arc::new(Semaphore::new(8));
        let incoming_host_slots = Arc::new(Semaphore::new(8));
        let incoming_source_slots = Arc::new(Semaphore::new(4));
        tokio::spawn(async move {
            let mut discovered: BTreeMap<PeerId, Vec<Multiaddr>> = BTreeMap::new();
            let in_flight = Arc::new(Mutex::new(BTreeSet::new()));
            let mut last_join = 0;
            let mut connections = BTreeMap::new();
            let mut dialing = BTreeMap::new();
            let mut planner = crate::route_selection::DialPlanner::default();
            let mut route_refresh = BTreeMap::<PeerId, Instant>::new();
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut listeners = Vec::new();
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    Some((peer, address)) = dials.recv() => {
                        discovered.entry(peer).or_default().push(address.clone());
                        swarm.add_peer_address(peer, address.clone());
                        let _ = swarm.dial(address.with(libp2p::multiaddr::Protocol::P2p(peer)));
                    },
                    Some((peer, stream)) = incoming.next() => {
                        if let Ok(permit) = incoming_slots.clone().try_acquire_owned() {
                            let state = shared.clone();
                            tokio::spawn(async move { let _permit = permit; let _ = serve_control(state, peer, stream).await; });
                        }
                    },
                    Some((peer, stream)) = files.next() => {
                        if let Ok(permit) = incoming_file_slots.clone().try_acquire_owned() {
                            let state = shared.clone(); let limits = limits.clone();
                            tokio::spawn(async move { let _permit = permit; let _ = serve_file(state, limits, peer, stream).await; });
                        }
                    },
                    Some((peer, stream)) = chats.next() => {
                        if let Ok(permit) = incoming_chat_slots.clone().try_acquire_owned() {
                            let state = shared.clone(); let stop = shutdown.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                tokio::select! { _ = stop.cancelled() => {}, _ = chat_network::serve(state, peer, stream) => {} }
                            });
                        }
                    },
                    Some((peer, stream)) = route_queries.next() => {
                        if let Ok(permit) = incoming_route_slots.clone().try_acquire_owned() {
                            let state = shared.clone(); let stop = shutdown.clone();
                            tokio::spawn(async move { let _permit = permit; tokio::select! { _ = stop.cancelled() => {}, _ = network_routes::serve(state, peer, stream) => {} } });
                        }
                    },
                    Some((peer, stream)) = host_calls.next() => {
                        if let Ok(permit) = incoming_host_slots.clone().try_acquire_owned() {
                            let node = poll_node.clone(); let stop = shutdown.clone();
                            tokio::spawn(async move { let _permit = permit; tokio::select! { _ = stop.cancelled() => {}, _ = network_host::serve(node, peer, stream) => {} } });
                        }
                    },
                    Some((peer, stream)) = host_sources.next() => {
                        if let Ok(permit) = incoming_source_slots.clone().try_acquire_owned() {
                            let node = poll_node.clone(); let stop = shutdown.clone();
                            tokio::spawn(async move { let _permit = permit; tokio::select! { _ = stop.cancelled() => {}, _ = network_host::serve_source(node, peer, stream) => {} } });
                        }
                    },
                    event = swarm.select_next_some() => match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            if !listeners.contains(&address) { listeners.push(address); } let _ = address_tx.send(listeners.clone());
                            let mut e = shared.lock().await; e.network_status = "LAN discovery is running".into(); e.emit();
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => for (peer, address) in peers {
                            if discovered.len() < 256 || discovered.contains_key(&peer) {
                                let addresses = discovered.entry(peer).or_default();
                                if addresses.len() < 8 && !addresses.contains(&address) { addresses.push(address.clone()); swarm.add_peer_address(peer, address); }
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(peers))) => for (peer, address) in peers { if let Some(routes) = discovered.get_mut(&peer) { routes.retain(|route| route != &address); } },
                        SwarmEvent::ExpiredListenAddr { address, .. } => { listeners.retain(|route| route != &address); let _ = address_tx.send(listeners.clone()); },
                        SwarmEvent::ListenerClosed { addresses, .. } => { listeners.retain(|route| !addresses.contains(route)); let _ = address_tx.send(listeners.clone()); },
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => { dialing.remove(&connection_id); connections.insert(connection_id, peer_id); },
                        SwarmEvent::OutgoingConnectionError { connection_id, .. } => { dialing.remove(&connection_id); },
                        SwarmEvent::ConnectionClosed { peer_id, connection_id, num_established, .. } => {
                            connections.remove(&connection_id);
                            if num_established != 0 { continue; }
                            let mut e = shared.lock().await; e.online.remove(&peer_id.to_string()); e.remote.retain(|_,m| m.owner_peer_id != peer_id.to_string());
                            if e.active_snapshot().is_ok_and(|snapshot| snapshot.owner_peer_id == peer_id.to_string()) { e.relay_host_generation = None; }
                            // Each transfer owns stream liveness/idle timeout. A direct connection
                            // closing must not cancel setup or an authenticated indirect stream.
                            e.emit();
                        },
                        SwarmEvent::ListenerError { .. } => shared.lock().await.error("LAN listener failed; restart Dump and check Windows firewall"),
                        _ => {},
                    },
                    _ = tick.tick() => {
                        let mut e = shared.lock().await;
                        e.online.retain(|_,seen| now().saturating_sub(*seen) < 15);
                        let active_peers: BTreeSet<_> = e.online.keys().cloned().collect();
                        e.remote.retain(|_,m| active_peers.contains(&m.owner_peer_id));
                        e.pending.retain(|_,p| now().saturating_sub(p.requested_at) < 86400);
                        let online = e.online.keys().cloned().collect::<BTreeSet<_>>(); e.relay_peers.retain(|peer| online.contains(peer));
                        if e.persisted.joining_routes.as_ref().is_some_and(|r| r.expires_at <= now()) { let _ = e.cancel_join(); e.error("Invitation expired. Ask the owner for a fresh link."); }
                        let joining = e.persisted.joining.clone();
                        let targets = e.active_snapshot().ok().filter(|s| s.contains(&e.peer())).map(|s| s.members.into_iter().filter_map(|m| m.peer_id.parse::<PeerId>().ok()).filter(|p| *p != peer).collect::<Vec<_>>()).unwrap_or_default();
                        let mut desired = targets.clone();
                        if let Some(invite) = &joining { if let Ok(owner) = invite.owner_peer_id.parse::<PeerId>() { desired.push(owner); } }
                        desired.sort(); desired.dedup(); planner.retain(&desired); route_refresh.retain(|peer, _| desired.contains(peer));
                        let mut budget = 4usize.saturating_sub(dialing.len().min(4));
                        let priority = joining.as_ref().and_then(|invite| invite.owner_peer_id.parse::<PeerId>().ok());
                        for target in &planner.order(&desired, priority, budget) {
                            let connected = connections.values().any(|peer| peer == target);
                            let mut candidates: Vec<_> = discovered.get(target).into_iter().flatten().take(8).filter_map(|address| network_routes::direct_candidate(*target, address.clone())).collect();
                            let signed = e.persisted.joining_routes.as_ref().filter(|r| r.peer_id == target.to_string()).or_else(|| e.persisted.active.and_then(|room| e.persisted.routes.get(&format!("{room}:{target}"))));
                            if let Some(record) = signed.filter(|r| r.verify_at(now()).is_ok()) { for route in &record.addresses { if let Ok(address) = crate::invite_routes::overlay_route(*target, route) { if !candidates.contains(&address) { candidates.push(address); } } } }
                            let pending = dialing.values().filter(|peer| *peer == target).count();
                            for address in planner.plan(*target, &candidates, connected, budget.min(2usize.saturating_sub(pending)), Instant::now()) {
                                let options = libp2p::swarm::dial_opts::DialOpts::peer_id(*target).condition(libp2p::swarm::dial_opts::PeerCondition::Always).addresses(vec![address.with(libp2p::multiaddr::Protocol::P2p(*target))]).build();
                                let id = options.connection_id();
                                if swarm.dial(options).is_ok() { dialing.insert(id, *target); budget = budget.saturating_sub(1); }
                                if joining.as_ref().is_some_and(|invite| invite.owner_peer_id == target.to_string()) { e.join_contacting = true; }
                            }
                        }
                        let host = e.active_snapshot().ok().and_then(|snapshot| snapshot.owner_peer_id.parse::<PeerId>().ok());
                        let relay_ready = e.relay_host_generation.is_some() && host.is_some_and(|owner| connections.values().any(|peer| *peer == owner));
                        e.emit(); drop(e);
                        for target in targets.into_iter().filter(|p| (connections.values().any(|peer| peer == p) || relay_ready) && joining.as_ref().is_none_or(|invite| invite.owner_peer_id != p.to_string())) {
                            let mut ongoing = in_flight.lock().await;
                            if ongoing.insert(target) {
                                let node = poll_node.clone(); let ongoing = in_flight.clone();
                                let refresh_routes = route_refresh.get(&target).is_none_or(|at| at.elapsed() >= Duration::from_secs(30));
                                if refresh_routes { route_refresh.insert(target, Instant::now()); }
                                tokio::spawn(async move { if refresh_routes { let _ = tokio::time::timeout(Duration::from_secs(10), node.refresh_routes(target)).await; if host == Some(target) && node.refresh_host_status(target).await.is_err() { node.shared.lock().await.relay_host_generation = None; } } let _ = node.refresh_peer(target).await; ongoing.lock().await.remove(&target); });
                            }
                        }
                        if let Some(invite) = joining {
                            if now() >= last_join + 4 {
                                last_join = now();
                                if let Ok(target) = invite.owner_peer_id.parse::<PeerId>() {
                                    if connections.values().any(|peer| *peer == target) {
                                        let mut ongoing = in_flight.lock().await;
                                        if ongoing.insert(target) { let node = poll_node.clone(); let ongoing = in_flight.clone(); tokio::spawn(async move { let _ = node.poll_join(invite).await; ongoing.lock().await.remove(&target); }); }
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
            e.network_status = "Offline".into();
            e.emit();
        });
        let refresh_shared = node.shared.clone();
        let refresh_shutdown = node.shutdown.clone();
        tokio::spawn(async move {
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
        let route_node = node.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! { _ = route_node.shutdown.cancelled() => break, _ = tick.tick() => {} }
                tokio::select! { _ = route_node.shutdown.cancelled() => break, _ = route_node.refresh_local_routes() => {} }
            }
        });
        Ok(node)
    }
    pub async fn connect(&self, peer: PeerId, address: Multiaddr) -> Result<()> {
        self.dial.send((peer, address)).await?;
        Ok(())
    }
    pub async fn request(&self, peer: PeerId, request: &Request) -> Result<Response> {
        let mut stream = if let Request::Catalog { snapshot, offset } = request {
            self.open_data_stream(
                peer,
                snapshot.workspace_id,
                crate::host_relay::Purpose::Catalog { offset: *offset },
                CONTROL,
            )
            .await?
        } else {
            let mut control = self.control.clone();
            network_host::DataStream::direct(
                tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, CONTROL)).await??,
            )
        };
        write_frame(&mut stream, request).await?;
        read_frame(&mut stream).await
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
                        .is_some_and(|i| i.token == invitation.token
                            && i.workspace_id == invitation.workspace_id
                            && i.owner_peer_id == invitation.owner_peer_id),
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
                if let Some(record) = &next.joining_routes {
                    crate::invite_routes::merge_record(
                        &mut next.routes,
                        record.clone(),
                        &next.workspaces[&invitation.workspace_id].snapshot,
                    )?;
                }
                next.joining_routes = None;
                e.persist(next)?;
                e.reset_runtime();
                e.emit();
            }
            Response::Denied => {
                let mut e = self.shared.lock().await;
                if e.persisted.joining.as_ref().is_some_and(|i| {
                    i.token == invitation.token
                        && i.workspace_id == invitation.workspace_id
                        && i.owner_peer_id == invitation.owner_peer_id
                }) {
                    e.cancel_join()?;
                    e.error("Invitation was rejected, expired, or already used. Ask the owner for a new invite.");
                }
            }
            Response::Waiting => {
                let mut e = self.shared.lock().await;
                if e.persisted.joining.as_ref().is_some_and(|current| {
                    current.token == invitation.token
                        && current.workspace_id == invitation.workspace_id
                        && current.owner_peer_id == invitation.owner_peer_id
                }) {
                    e.join_acknowledged = Some(digest(invitation.token.as_bytes()));
                    e.join_contacting = true;
                    e.emit();
                }
            }
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
        Ok(())
    }
    pub async fn receive(&self, file_id: Uuid, directory: PathBuf) -> Result<Uuid> {
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
        tokio::spawn(async move {
            // Disk operations cannot be cancelled safely: finish them before cleanup.
            let result = node.receive_inner(&manifest, &directory, id, &cancel).await;
            let part = directory.join(format!(".dump-{id}.part"));
            let mut e = node.shared.lock().await;
            let bytes = e.transfers.get(&id).map_or(0, |t| t.bytes);
            let cleaned = if result.is_err() {
                match tokio::fs::remove_file(&part).await {
                    Ok(()) => true,
                    Err(err) => err.kind() == std::io::ErrorKind::NotFound,
                }
            } else {
                true
            };
            let mut next = e.persisted.clone();
            if cleaned {
                next.partials.retain(|p| p != &part);
            }
            if let Err(err) = e.persist(next) {
                e.error(format!("Cannot save download state: {err}"));
            }
            match result {
                Ok(()) => e.mark_transfer(id, "Completed", manifest.size, None),
                Err(err) => e.mark_transfer(
                    id,
                    if cancel.is_cancelled() {
                        "Cancelled"
                    } else {
                        "Failed"
                    },
                    bytes,
                    Some(err.to_string()),
                ),
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
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => bail!("Transfer cancelled"),
            stream = self.open_data_stream(peer, manifest.workspace_id, crate::host_relay::Purpose::File { manifest: manifest.clone() }, FILE) => stream?,
        };
        if let Some(transfer) = self.shared.lock().await.transfers.get_mut(&id) {
            transfer.via_host = stream.via_host;
        }
        write_frame(
            &mut stream,
            &Request::File {
                snapshot,
                manifest: manifest.clone(),
            },
        )
        .await?;
        match read_frame(&mut stream).await? {
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
        {
            let mut e = self.shared.lock().await;
            let mut next = e.persisted.clone();
            next.partials.push(part.clone());
            e.persist(next)?;
            e.mark_transfer(id, "Receiving", 0, None);
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .await
            .context("Cannot create temporary download")?;
        let mut remaining = manifest.size;
        let mut total = 0;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; BLOCK];
        let mut last = Instant::now();
        while remaining > 0 {
            let wanted = remaining.min(BLOCK as u64) as usize;
            tokio::select! {
                _ = cancel.cancelled() => bail!("Transfer cancelled"),
                read = tokio::time::timeout(INACTIVITY, stream.read_exact(&mut buffer[..wanted])) => read??,
            };
            DiskWrite::write_all(&mut file, &buffer[..wanted]).await?;
            hash.update(&buffer[..wanted]);
            remaining -= wanted as u64;
            total += wanted as u64;
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
                    response = read_frame::<_, Response>(&mut stream) => response?,
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
                .cloned()
                .or_else(|| {
                    e.persisted
                        .shared_invites
                        .get(&hash)
                        .filter(|record| {
                            record.workspace_id == invitation.workspace_id
                                && record.expires_at > now()
                                && !record.rejected.contains(&peer.to_string())
                        })
                        .map(|record| InviteRecord {
                            workspace_id: record.workspace_id,
                            token_hash: hash.clone(),
                            expires_at: record.expires_at,
                            approved_peer: record
                                .approved
                                .contains(&peer.to_string())
                                .then(|| peer.to_string()),
                        })
                })
                .context("Access denied")?;
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
) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request: Request = read_frame(&mut stream).await?;
    serve_file_request(
        shared,
        limits,
        peer,
        stream,
        request,
        false,
        CancellationToken::new(),
    )
    .await
}

async fn serve_file_request<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    shared: Shared,
    limits: Arc<Limits>,
    peer: PeerId,
    mut stream: S,
    request: Request,
    via_host: bool,
    transport_cancel: CancellationToken,
) -> Result<()> {
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
                if let Some(transfer) = e.transfers.get_mut(&id) {
                    transfer.via_host = via_host;
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
        _ = transport_cancel.cancelled() => Err(anyhow::anyhow!("Relay workspace or membership changed")),
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
            write_frame(&mut stream,&Response::File {manifest:manifest.clone()}).await?;
            shared.lock().await.mark_transfer(id,"Sharing",0,None);
            let mut hash = Sha256::new(); let mut total = 0; let mut buffer = vec![0;BLOCK]; let mut last = Instant::now();
            while total < manifest.size {
                let wanted = (manifest.size-total).min(BLOCK as u64) as usize;
                tokio::time::timeout(INACTIVITY,DiskRead::read_exact(&mut file,&mut buffer[..wanted])).await??;
                tokio::time::timeout(INACTIVITY,stream.write_all(&buffer[..wanted])).await??;
                hash.update(&buffer[..wanted]); total += wanted as u64;
                if last.elapsed() >= Duration::from_millis(200) {shared.lock().await.mark_transfer(id,"Sharing",total,None); last = Instant::now();}
            }
            let mut extra = [0]; ensure!(DiskRead::read(&mut file,&mut extra).await? == 0 && hex::encode(hash.finalize()) == manifest.sha256,"Source file integrity changed");
            write_frame(&mut stream,&Response::Done).await?; stream.close().await?;
            Ok::<(),anyhow::Error>(())
        } => result,
    };
    let mut e = shared.lock().await;
    let bytes = e.transfers.get(&id).map_or(0, |t| t.bytes);
    match result {
        Ok(()) => e.mark_transfer(id, "Completed", manifest.size, None),
        Err(err) => {
            let message = err.to_string();
            e.mark_transfer(
                id,
                if cancel.is_cancelled() || transport_cancel.is_cancelled() {
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
