//! Workspace-owner broker. Host forwards opaque Noise bytes, never source files.
use super::*;
use crate::host_relay::{self, Grant, Purpose};
use libp2p::core::upgrade::{InboundConnectionUpgrade, OutboundConnectionUpgrade, UpgradeInfo};
use std::{
    pin::Pin,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    task::{Context as TaskContext, Poll},
};
use tokio::sync::{oneshot, OwnedSemaphorePermit};

const HOST: StreamProtocol = StreamProtocol::new("/dump/host-relay/1");
const SOURCE: StreamProtocol = StreamProtocol::new("/dump/host-source/1");
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Call {
    Status {
        snapshot: Snapshot,
    },
    Fetch {
        snapshot: Snapshot,
        source: String,
        purpose: Purpose,
    },
    Prepare {
        grant: Grant,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Reply {
    Status {
        snapshot: Snapshot,
        generation: Option<Uuid>,
    },
    Ready {
        grant: Box<Grant>,
    },
    Prepared,
    Accepted,
    Denied,
    Unavailable,
    Busy,
}

pub(super) trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}
pub(super) struct DataStream {
    inner: Box<dyn Duplex>,
    _permit: Option<OwnedSemaphorePermit>,
    pub via_host: bool,
}
impl DataStream {
    pub(super) fn direct(stream: Stream) -> Self {
        Self {
            inner: Box::new(stream),
            _permit: None,
            via_host: false,
        }
    }
}
impl AsyncRead for DataStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for DataStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.inner).poll_close(cx)
    }
}
#[derive(Clone)]
struct Scope {
    snapshot: Snapshot,
    generation: Uuid,
    cancel: CancellationToken,
}
struct Pending {
    grant: Grant,
    sender: oneshot::Sender<Stream>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct Inner {
    scope: Option<Scope>,
    pending: BTreeMap<Uuid, Pending>,
    peers: BTreeMap<String, Arc<Semaphore>>,
}
pub(super) struct Hub {
    inner: std::sync::Mutex<Inner>,
    slots: Arc<Semaphore>,
    source_slots: Arc<Semaphore>,
    client_slots: Arc<Semaphore>,
    bandwidth: Arc<host_relay::Bandwidth>,
    active: Arc<AtomicUsize>,
}
impl Hub {
    pub(super) fn new(active: Arc<AtomicUsize>) -> Self {
        Self {
            inner: std::sync::Mutex::new(Inner::default()),
            slots: Arc::new(Semaphore::new(4)),
            source_slots: Arc::new(Semaphore::new(2)),
            client_slots: Arc::new(Semaphore::new(2)),
            bandwidth: Arc::new(host_relay::Bandwidth::default()),
            active,
        }
    }
    fn scope(&self, snapshot: Snapshot, token: CancellationToken) -> Result<Scope> {
        ensure!(!token.is_cancelled(), "Relay scope stopped");
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(scope) = &inner.scope {
            if scope.snapshot == snapshot && !scope.cancel.is_cancelled() {
                return Ok(scope.clone());
            }
            scope.cancel.cancel();
        }
        inner.pending.clear();
        inner.peers.clear();
        let scope = Scope {
            snapshot,
            generation: Uuid::new_v4(),
            cancel: token.child_token(),
        };
        inner.scope = Some(scope.clone());
        Ok(scope)
    }
    fn allocate(
        self: &Arc<Self>,
        grant: Grant,
        scope: &Scope,
    ) -> Result<(Lease, oneshot::Receiver<Stream>)> {
        grant.authorize(&scope.snapshot, &grant.receiver, scope.generation)?;
        ensure!(!scope.cancel.is_cancelled(), "Relay scope changed");
        let global = self
            .slots
            .clone()
            .try_acquire_owned()
            .context("Host relay is busy")?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            inner
                .scope
                .as_ref()
                .is_some_and(|current| current.generation == scope.generation
                    && !current.cancel.is_cancelled()),
            "Relay generation changed"
        );
        let a = inner
            .peers
            .entry(grant.receiver.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(2)))
            .clone()
            .try_acquire_owned()
            .context("Receiving device relay limit reached")?;
        let b = inner
            .peers
            .entry(grant.source.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(2)))
            .clone()
            .try_acquire_owned()
            .context("Source device relay limit reached")?;
        ensure!(
            inner.pending.len() < 4 && !inner.pending.contains_key(&grant.nonce),
            "Relay pending limit reached"
        );
        let (sender, receiver) = oneshot::channel();
        let cancel = scope.cancel.child_token();
        let nonce = grant.nonce;
        inner.pending.insert(
            nonce,
            Pending {
                grant,
                sender,
                cancel: cancel.clone(),
            },
        );
        self.active.fetch_add(1, Ordering::AcqRel);
        Ok((
            Lease {
                hub: self.clone(),
                nonce,
                cancel,
                _permits: [global, a, b],
            },
            receiver,
        ))
    }
    fn claim(&self, grant: &Grant, source: &str) -> Result<oneshot::Sender<Stream>> {
        grant.verify_at(now())?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            inner
                .scope
                .as_ref()
                .is_some_and(|scope| scope.snapshot == grant.snapshot
                    && scope.generation == grant.generation
                    && !scope.cancel.is_cancelled()),
            "Relay generation stopped"
        );
        let pending = inner
            .pending
            .get(&grant.nonce)
            .context("Relay grant was consumed or cancelled")?;
        ensure!(
            pending.grant == *grant && grant.source == source && !pending.cancel.is_cancelled(),
            "Wrong relay source"
        );
        Ok(inner.pending.remove(&grant.nonce).unwrap().sender)
    }
}
struct Lease {
    hub: Arc<Hub>,
    nonce: Uuid,
    cancel: CancellationToken,
    _permits: [OwnedSemaphorePermit; 3],
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.hub
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(&self.nonce);
        self.hub.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn accept(
    control: &mut Control,
) -> Result<(
    libp2p_stream::IncomingStreams,
    libp2p_stream::IncomingStreams,
)> {
    Ok((control.accept(HOST)?, control.accept(SOURCE)?))
}
impl Node {
    async fn owner_scope(&self, peer: PeerId, incoming: &Snapshot) -> Result<Scope> {
        let (current, cancel) = {
            let mut e = self.shared.lock().await;
            let current = e.authorize(&peer.to_string(), incoming)?;
            ensure!(
                current == *incoming
                    && current.owner_peer_id == e.peer()
                    && e.persisted.host_relay.contains(&current.workspace_id),
                "Host relay is disabled or workspace changed"
            );
            (current, e.relay_cancel.clone())
        };
        self.host.scope(current, cancel)
    }
    pub async fn refresh_host_status(&self, owner: PeerId) -> Result<()> {
        let snapshot = self.shared.lock().await.active_snapshot()?;
        ensure!(
            snapshot.owner_peer_id == owner.to_string()
                && snapshot.contains(&self.own_peer.to_string()),
            "Wrong workspace host"
        );
        let mut control = self.control.clone();
        let mut stream =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(owner, HOST)).await??;
        write_frame(
            &mut stream,
            &Call::Status {
                snapshot: snapshot.clone(),
            },
        )
        .await?;
        let reply: Reply = read_frame(&mut stream).await?;
        let mut e = self.shared.lock().await;
        ensure!(
            e.persisted.active == Some(snapshot.workspace_id),
            "Workspace changed"
        );
        match reply {
            Reply::Status {
                snapshot: latest,
                generation,
            } => {
                ensure!(
                    latest.workspace_id == snapshot.workspace_id,
                    "Wrong host room"
                );
                let current = e.authorize(&owner.to_string(), &latest)?;
                ensure!(current == latest, "Host relay status is stale");
                e.relay_host_generation = generation;
                e.emit();
                Ok(())
            }
            _ => {
                e.relay_host_generation = None;
                anyhow::bail!("Host relay status unavailable")
            }
        }
    }
    pub(super) async fn open_data_stream(
        &self,
        peer: PeerId,
        workspace: Uuid,
        purpose: Purpose,
        protocol: StreamProtocol,
    ) -> Result<DataStream> {
        let mut control = self.control.clone();
        if let Ok(Ok(stream)) =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, protocol)).await
        {
            self.shared
                .lock()
                .await
                .relay_peers
                .remove(&peer.to_string());
            return Ok(DataStream::direct(stream));
        }
        let permit = self
            .host
            .client_slots
            .clone()
            .try_acquire_owned()
            .context("Relay client is busy; retry shortly")?;
        let snapshot = self.shared.lock().await.active_snapshot()?;
        ensure!(
            snapshot.workspace_id == workspace
                && snapshot.contains(&peer.to_string())
                && snapshot.contains(&self.own_peer.to_string()),
            "Relay membership changed"
        );
        let owner: PeerId = snapshot.owner_peer_id.parse()?;
        ensure!(
            owner != self.own_peer && owner != peer,
            "Direct source/host unavailable; source and host must stay online"
        );
        purpose.budget()?;
        let mut control = self.control.clone();
        let mut raw =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(owner, HOST)).await??;
        write_frame(
            &mut raw,
            &Call::Fetch {
                snapshot: snapshot.clone(),
                source: peer.to_string(),
                purpose: purpose.clone(),
            },
        )
        .await?;
        let reply: Reply = read_host_reply(&mut raw).await?;
        let grant = match reply { Reply::Ready { grant } => grant, Reply::Busy => anyhow::bail!("Host relay is busy; retry shortly"), _ => anyhow::bail!("Host/source could not be reached or did not authorize this request. Both must stay online.") };
        grant.authorize(&snapshot, &self.own_peer.to_string(), grant.generation)?;
        ensure!(
            grant.source == peer.to_string() && grant.purpose == purpose,
            "Host returned the wrong grant"
        );
        let (key, scope) = {
            let e = self.shared.lock().await;
            ensure!(
                e.active_snapshot()? == snapshot,
                "Workspace or membership changed"
            );
            (e.key.clone(), e.relay_cancel.clone())
        };
        let noise = libp2p::noise::Config::new(&key)?;
        let protocol = noise.protocol_info().next().context("No Noise protocol")?;
        let (actual, encrypted) = tokio::select! { biased; _ = self.shutdown.cancelled() => anyhow::bail!("Networking stopped"), _ = scope.cancelled() => anyhow::bail!("Workspace changed"), result = tokio::time::timeout(Duration::from_secs(15), noise.upgrade_outbound(raw, protocol)) => result?? };
        ensure!(
            actual == peer,
            "Relay endpoint identity does not match the signed source"
        );
        grant.verify_at(now())?;
        {
            let mut e = self.shared.lock().await;
            ensure!(
                e.active_snapshot()? == snapshot && !scope.is_cancelled(),
                "Workspace changed after relay setup"
            );
            e.relay_peers.insert(peer.to_string());
        }
        Ok(DataStream {
            inner: Box::new(encrypted),
            _permit: Some(permit),
            via_host: true,
        })
    }
    async fn prepare_source(&self, host: PeerId, grant: Grant) -> Result<()> {
        grant.verify_at(now())?;
        let (key, scope) = {
            let mut e = self.shared.lock().await;
            ensure!(
                host.to_string() == grant.snapshot.owner_peer_id && grant.source == e.peer(),
                "Wrong relay host or source"
            );
            let current = e.authorize(&host.to_string(), &grant.snapshot)?;
            ensure!(current == grant.snapshot, "Relay grant membership is stale");
            if let Purpose::File { manifest } = &grant.purpose {
                ensure!(
                    e.persisted
                        .shares
                        .get(&manifest.file_id)
                        .is_some_and(|share| share.available && share.manifest == *manifest),
                    "Source file unavailable"
                );
            }
            (e.key.clone(), e.relay_cancel.clone())
        };
        let permit = self
            .host
            .source_slots
            .clone()
            .try_acquire_owned()
            .context("Source relay queue is full")?;
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! { biased; _ = node.shutdown.cancelled() => {}, _ = node.upload_source(host, grant, key, scope.clone()) => {} }
        });
        Ok(())
    }
    async fn upload_source(
        &self,
        host: PeerId,
        grant: Grant,
        key: libp2p::identity::Keypair,
        scope: CancellationToken,
    ) -> Result<()> {
        let mut control = self.control.clone();
        let mut raw =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(host, SOURCE)).await??;
        write_frame(&mut raw, &grant).await?;
        ensure!(
            matches!(read_frame::<_, Reply>(&mut raw).await?, Reply::Accepted),
            "Host did not accept this source grant"
        );
        let noise = libp2p::noise::Config::new(&key)?;
        let protocol = noise.protocol_info().next().context("No Noise protocol")?;
        let (peer, mut encrypted) = tokio::time::timeout(
            Duration::from_secs(15),
            noise.upgrade_inbound(raw, protocol),
        )
        .await??;
        ensure!(
            peer.to_string() == grant.receiver,
            "Relay receiver identity mismatch"
        );
        grant.verify_at(now())?;
        let request: Request = read_frame(&mut encrypted).await?;
        grant.authorize_request(&request)?;
        {
            let mut e = self.shared.lock().await;
            let current = e.authorize(&peer.to_string(), &grant.snapshot)?;
            ensure!(
                current == grant.snapshot && !scope.is_cancelled(),
                "Source workspace or membership changed"
            );
            e.admit_request(&peer.to_string())?;
        }
        match grant.purpose {
            Purpose::Catalog { .. } => {
                let response = handle_control(self.shared.clone(), peer, request)
                    .await
                    .unwrap_or(Response::Denied);
                write_frame(&mut encrypted, &response).await?;
                encrypted.close().await?;
                Ok(())
            }
            Purpose::File { .. } => {
                serve_file_request(
                    self.shared.clone(),
                    self.limits.clone(),
                    peer,
                    encrypted,
                    request,
                    true,
                    scope.clone(),
                )
                .await
            }
        }
    }
}
async fn read_host_reply<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Reply> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut length = [0; 4];
        stream.read_exact(&mut length).await?;
        let len = u32::from_be_bytes(length) as usize;
        ensure!(len > 0 && len <= MAX_CONTROL, "Invalid host reply size");
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await?;
        let mut reader = std::io::Cursor::new(bytes);
        let reply = ciborium::from_reader(&mut reader)?;
        ensure!(reader.position() == len as u64, "Trailing host reply data");
        Ok(reply)
    })
    .await?
}
pub(super) async fn serve_source(node: Node, peer: PeerId, mut stream: Stream) -> Result<()> {
    node.shared.lock().await.admit_request(&peer.to_string())?;
    let grant: Grant = read_frame(&mut stream).await?;
    let scope = node.owner_scope(peer, &grant.snapshot).await?;
    grant.authorize(&scope.snapshot, &grant.receiver, scope.generation)?;
    let sender = node.host.claim(&grant, &peer.to_string())?;
    write_frame(&mut stream, &Reply::Accepted).await?;
    sender
        .send(stream)
        .map_err(|_| anyhow::anyhow!("Receiver cancelled"))?;
    Ok(())
}
pub(super) async fn serve(node: Node, peer: PeerId, mut stream: Stream) -> Result<()> {
    node.shared.lock().await.admit_request(&peer.to_string())?;
    let call: Call = read_frame(&mut stream).await?;
    match call {
        Call::Status { snapshot } => {
            let (current, enabled) = {
                let mut e = node.shared.lock().await;
                let current = e.authorize(&peer.to_string(), &snapshot)?;
                ensure!(current.owner_peer_id == e.peer(), "Not the host");
                let enabled = e.persisted.host_relay.contains(&current.workspace_id);
                (current, enabled)
            };
            let generation = if enabled {
                Some(node.owner_scope(peer, &current).await?.generation)
            } else {
                None
            };
            write_frame(
                &mut stream,
                &Reply::Status {
                    snapshot: current,
                    generation,
                },
            )
            .await?;
        }
        Call::Prepare { grant } => {
            let reply = if node.prepare_source(peer, grant).await.is_ok() {
                Reply::Prepared
            } else {
                Reply::Denied
            };
            write_frame(&mut stream, &reply).await?;
        }
        Call::Fetch {
            snapshot,
            source,
            purpose,
        } => {
            let scope = match node.owner_scope(peer, &snapshot).await {
                Ok(scope) => scope,
                Err(_) => {
                    write_frame(&mut stream, &Reply::Unavailable).await?;
                    return Ok(());
                }
            };
            let key = node.shared.lock().await.key.clone();
            let grant = Grant::new(
                &key,
                scope.snapshot.clone(),
                peer.to_string(),
                source.clone(),
                purpose,
                scope.generation,
            )?;
            let (lease, incoming) = match node.host.allocate(grant.clone(), &scope) {
                Ok(result) => result,
                Err(_) => {
                    write_frame(&mut stream, &Reply::Busy).await?;
                    return Ok(());
                }
            };
            let source_peer: PeerId = source.parse()?;
            let setup = async {
                let mut control = node.control.clone();
                let mut upstream =
                    tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(source_peer, HOST))
                        .await??;
                write_frame(
                    &mut upstream,
                    &Call::Prepare {
                        grant: grant.clone(),
                    },
                )
                .await?;
                ensure!(
                    matches!(
                        read_frame::<_, Reply>(&mut upstream).await?,
                        Reply::Prepared
                    ),
                    "Source unavailable or access denied"
                );
                Ok::<_, anyhow::Error>(incoming.await?)
            };
            let upstream = tokio::select! { biased; _ = lease.cancel.cancelled() => anyhow::bail!("Host relay scope changed"), result = tokio::time::timeout(Duration::from_secs(15), setup) => result };
            let upstream = match upstream {
                Ok(Ok(stream)) => stream,
                _ => {
                    write_frame(&mut stream, &Reply::Unavailable).await?;
                    return Ok(());
                }
            };
            grant.authorize(&scope.snapshot, &peer.to_string(), scope.generation)?;
            write_frame(
                &mut stream,
                &Reply::Ready {
                    grant: Box::new(grant.clone()),
                },
            )
            .await?;
            let remaining = Arc::new(AtomicU64::new(grant.purpose.budget()?));
            let (client_read, client_write) = stream.split();
            let (source_read, source_write) = upstream.split();
            tokio::try_join!(
                host_relay::pump(
                    client_read,
                    source_write,
                    remaining.clone(),
                    node.host.bandwidth.clone(),
                    lease.cancel.clone()
                ),
                host_relay::pump(
                    source_read,
                    client_write,
                    remaining,
                    node.host.bandwidth.clone(),
                    lease.cancel.clone()
                ),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_callback_is_single_use_and_cancelled_epochs_release_capacity() -> Result<()> {
        let owner = libp2p::identity::Keypair::generate_ed25519();
        let source = libp2p::identity::Keypair::generate_ed25519();
        let receiver = libp2p::identity::Keypair::generate_ed25519();
        let mut snapshot = Snapshot {
            version: crate::model::VERSION,
            workspace_id: Uuid::new_v4(),
            name: "Room".into(),
            owner_peer_id: owner.public().to_peer_id().to_string(),
            owner_public_key: crate::model::public_key(&owner),
            revision: 1,
            members: [&owner, &source, &receiver]
                .into_iter()
                .map(|key| crate::model::Member {
                    peer_id: key.public().to_peer_id().to_string(),
                    name: "Device".into(),
                })
                .collect(),
            signature: String::new(),
        };
        snapshot.sign(&owner)?;
        let hub = Arc::new(Hub::new(Arc::new(AtomicUsize::new(0))));
        let token = CancellationToken::new();
        let scope = hub.scope(snapshot.clone(), token.clone())?;
        let make = |generation| {
            Grant::new(
                &owner,
                snapshot.clone(),
                receiver.public().to_peer_id().to_string(),
                source.public().to_peer_id().to_string(),
                Purpose::Catalog { offset: 0 },
                generation,
            )
        };
        let grant = make(scope.generation)?;
        // Box is an allocation detail; older unboxed peers receive identical CBOR.
        #[derive(Serialize)]
        enum UnboxedReply {
            Ready { grant: Grant },
        }
        let mut previous = Vec::new();
        ciborium::into_writer(
            &UnboxedReply::Ready {
                grant: grant.clone(),
            },
            &mut previous,
        )?;
        let mut current = Vec::new();
        ciborium::into_writer(
            &Reply::Ready {
                grant: Box::new(grant.clone()),
            },
            &mut current,
        )?;
        assert_eq!(current, previous);
        let (lease, _incoming) = hub.allocate(grant.clone(), &scope)?;
        assert!(hub
            .claim(&grant, &receiver.public().to_peer_id().to_string())
            .is_err());
        let _sender = hub.claim(&grant, &source.public().to_peer_id().to_string())?;
        assert!(hub
            .claim(&grant, &source.public().to_peer_id().to_string())
            .is_err());
        let (second, _) = hub.allocate(make(scope.generation)?, &scope)?;
        assert!(hub.allocate(make(scope.generation)?, &scope).is_err());
        token.cancel();
        assert!(lease.cancel.is_cancelled());
        assert!(hub.allocate(make(scope.generation)?, &scope).is_err());
        drop(lease);
        drop(second);
        assert_eq!(hub.active.load(Ordering::Acquire), 0);
        assert_eq!(hub.slots.available_permits(), 4);
        let fresh = hub.scope(snapshot.clone(), CancellationToken::new())?;
        assert_ne!(fresh.generation, scope.generation);
        assert!(hub
            .claim(&grant, &source.public().to_peer_id().to_string())
            .is_err());
        Ok(())
    }
}

#[cfg(test)]
#[path = "network_host_tests.rs"]
mod transport_tests;
