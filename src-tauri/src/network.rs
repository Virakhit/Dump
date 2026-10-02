use crate::{
    connectivity::{
        diagnostic, direct_address, public_address, AddressBook, ConnectionState, Diagnostics,
        DialFailure, Reachability, ReachabilityStatus, MAX_ADDRESSES,
    },
    engine::{open_source, refresh_share, stamp, Shared},
    model::*,
    reachability::{Client as NatClient, PublicAddresses},
    storage::commit_download,
};
use anyhow::{bail, ensure, Context, Result};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
use libp2p::{
    connection_limits, identify, mdns, ping,
    swarm::{
        behaviour::toggle::Toggle, dial_opts::DialOpts, DialError, FromSwarm, NetworkBehaviour,
        NewExternalAddrCandidate, Stream, StreamProtocol, SwarmEvent,
    },
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
    sync::{mpsc, oneshot, watch, Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const CONTROL: StreamProtocol = StreamProtocol::new("/dump/control/1");
const FILE: StreamProtocol = StreamProtocol::new("/dump/file/1");
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(5);
const INACTIVITY: Duration = Duration::from_secs(30);

#[derive(NetworkBehaviour)]
struct Behaviour {
    streams: libp2p_stream::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    autonat: NatClient,
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
    send: Arc<Semaphore>,
    receive: Arc<Semaphore>,
    peers: Mutex<BTreeMap<PeerId, PeerSlots>>,
}
impl Limits {
    fn new() -> Self {
        Self {
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

struct Dial {
    peer: PeerId,
    address: Multiaddr,
    reply: oneshot::Sender<Result<()>>,
}

#[derive(Clone)]
pub struct Node {
    pub shared: Shared,
    pub control: Control,
    dial: mpsc::Sender<Dial>,
    pub addresses: watch::Receiver<Vec<Multiaddr>>,
    pub diagnostics: watch::Receiver<Diagnostics>,
    pub reachability: watch::Receiver<Reachability>,
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
            .with_behaviour(|key| Behaviour {
                streams: libp2p_stream::Behaviour::new(),
                mdns: mdns.into(),
                ping: ping::Behaviour::new(
                    ping::Config::new()
                        .with_interval(Duration::from_secs(5))
                        .with_timeout(Duration::from_secs(5)),
                ),
                identify: identify::Behaviour::new(
                    identify::Config::new("/dump/1".into(), key.public())
                        .with_agent_version(format!("dump/{}", env!("CARGO_PKG_VERSION")))
                        // Never leak interface addresses to Internet peers or cache their
                        // unchecked routing hints. mDNS remains independent for LAN peers.
                        .with_hide_listen_addrs(true)
                        .with_cache_size(0),
                ),
                autonat: NatClient::new(peer),
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
        let (dial, mut dials) = mpsc::channel(128);
        let (address_tx, addresses) = watch::channel(Vec::new());
        let (diagnostic_tx, diagnostics) = watch::channel(Diagnostics::new());
        let (reachability_tx, reachability) = watch::channel(Reachability::default());
        let limits = Arc::new(Limits::new());
        let shutdown = CancellationToken::new();
        let node = Self {
            shared: shared.clone(),
            control: control.clone(),
            dial,
            addresses,
            diagnostics,
            reachability,
            limits: limits.clone(),
            shutdown: shutdown.clone(),
        };
        let poll_node = node.clone();
        let incoming_slots = Arc::new(Semaphore::new(32));
        let incoming_file_slots = Arc::new(Semaphore::new(8));
        tokio::spawn(async move {
            let mut discovered = AddressBook::default();
            let in_flight = Arc::new(Mutex::new(BTreeSet::new()));
            let mut last_join = 0;
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut listeners = Vec::new();
            let mut public = PublicAddresses::default();
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    Some(Dial { peer, address, reply }) = dials.recv() => {
                        let result = (|| {
                            discovered.remember(peer, address.clone())?;
                            swarm.add_peer_address(peer, address.clone());
                            if !swarm.is_connected(&peer) {
                                swarm.dial(DialOpts::peer_id(peer).addresses(vec![address]).build())?;
                                diagnostic_tx.send_modify(|d| {
                                    if let Some(d) = diagnostic(d, peer) {
                                        d.state = ConnectionState::Connecting;
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
                            tokio::spawn(async move { let _permit = permit; let _ = serve_control(state, peer, stream).await; });
                        }
                    },
                    Some((peer, stream)) = files.next() => {
                        if let Ok(permit) = incoming_file_slots.clone().try_acquire_owned() {
                            let state = shared.clone(); let limits = limits.clone();
                            tokio::spawn(async move { let _permit = permit; let _ = serve_file(state, limits, peer, stream).await; });
                        }
                    },
                    event = swarm.select_next_some() => match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            if public_address(&address) {
                                swarm.behaviour_mut().autonat.on_swarm_event(FromSwarm::NewExternalAddrCandidate(NewExternalAddrCandidate { addr: &address }));
                            }
                            listeners.push(address); let _ = address_tx.send(listeners.clone());
                            let mut e = shared.lock().await; e.network_status = if discovery { "LAN discovery is running" } else { "Listening for direct connections" }.into(); e.emit();
                        },
                        SwarmEvent::ExpiredListenAddr { address, .. } => {
                            listeners.retain(|a| a != &address); let _ = address_tx.send(listeners.clone());
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
                            // The library requires a matching nonce on an inbound dial-back,
                            // not just a server's claim that an address is reachable.
                            if let Ok(address) = direct_address(peer, event.tested_addr) {
                                public.record(event.server, address.clone(), event.result.is_ok(), Instant::now());
                                if event.result.is_err() { swarm.remove_external_address(&address); }
                                let addresses = public.addresses();
                                reachability_tx.send_replace(Reachability {
                                    status: if !addresses.is_empty() { ReachabilityStatus::Public } else if event.result.is_err() { ReachabilityStatus::Unreachable } else { ReachabilityStatus::Unknown },
                                    public_addresses: addresses,
                                });
                            }
                        },
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            diagnostic_tx.send_modify(|d| {
                                if let Some(d) = diagnostic(d, peer_id) {
                                    d.state = ConnectionState::Direct;
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
                        SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
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
                                }
                            });
                            let mut e = shared.lock().await; e.online.remove(&peer_id.to_string()); e.remote.retain(|_,m| m.owner_peer_id != peer_id.to_string());
                            for (id,t) in &e.transfers { if t.peer_id == peer_id.to_string() { if let Some(c) = e.cancellations.get(id) { c.cancel(); } } }
                            e.emit();
                        },
                        SwarmEvent::ListenerError { .. } => shared.lock().await.error("LAN listener failed; restart Dump and check Windows firewall"),
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
                        e.online.retain(|_,seen| now().saturating_sub(*seen) < 15);
                        let active_peers: BTreeSet<_> = e.online.keys().cloned().collect();
                        e.remote.retain(|_,m| active_peers.contains(&m.owner_peer_id));
                        e.pending.retain(|_,p| now().saturating_sub(p.requested_at) < 86400);
                        let joining = e.persisted.joining.clone();
                        let targets = e.active_snapshot().ok().filter(|s| s.contains(&e.peer())).map(|s| s.members.into_iter().filter_map(|m| m.peer_id.parse::<PeerId>().ok()).filter(|p| *p != peer).collect::<Vec<_>>()).unwrap_or_default();
                        e.network_status = if !e.online.is_empty() { "Connected directly" } else if joining.is_some() { "Connecting…" } else if listeners.is_empty() { "Offline" } else if discovery { "LAN discovery is running" } else { "Listening for direct connections" }.into();
                        e.emit(); drop(e);
                        for target in targets.into_iter().filter(|p| discovered.contains(p)) {
                            let mut ongoing = in_flight.lock().await;
                            if ongoing.insert(target) {
                                let node = poll_node.clone(); let ongoing = in_flight.clone();
                                tokio::spawn(async move { let _ = node.refresh_peer(target).await; ongoing.lock().await.remove(&target); });
                            }
                        }
                        if let Some(invite) = joining {
                            if now() >= last_join + 4 {
                                last_join = now();
                                if let Ok(target) = invite.owner_peer_id.parse::<PeerId>() {
                                    if discovered.contains(&target) {
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
            diagnostic_tx.send_modify(|d| d.clear());
            reachability_tx.send_replace(Reachability::default());
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
        Ok(node)
    }
    pub async fn connect(&self, peer: PeerId, address: Multiaddr) -> Result<()> {
        let address = direct_address(peer, address)?;
        let (reply, registered) = oneshot::channel();
        self.dial
            .send(Dial {
                peer,
                address,
                reply,
            })
            .await?;
        // Acknowledges registration, not connection success; consult diagnostics for that.
        tokio::time::timeout(PREAUTH_TIMEOUT, registered).await??
    }
    pub async fn request(&self, peer: PeerId, request: &Request) -> Result<Response> {
        let mut control = self.control.clone();
        let mut stream =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, CONTROL)).await??;
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
        let mut control = self.control.clone();
        let mut stream = tokio::select! {
            _ = cancel.cancelled() => bail!("Transfer cancelled"),
            stream = tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, FILE)) => stream??,
        };
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
