#![cfg(windows)]
use anyhow::{Context, Result};
use dump_core::{
    host_relay::{Grant, Purpose, MAX_FILE_BYTES},
    model::{digest, public_key, Manifest, Member, Snapshot, VERSION},
};
use libp2p::identity::Keypair;
use uuid::Uuid;
fn roster(owner: &Keypair, source: &Keypair, receiver: &Keypair) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        version: VERSION,
        workspace_id: Uuid::new_v4(),
        name: "Office".into(),
        owner_peer_id: owner.public().to_peer_id().to_string(),
        owner_public_key: public_key(owner),
        revision: 1,
        members: [owner, source, receiver]
            .into_iter()
            .map(|key| Member {
                peer_id: key.public().to_peer_id().to_string(),
                name: "Device".into(),
            })
            .collect(),
        signature: String::new(),
    };
    snapshot.sign(owner)?;
    Ok(snapshot)
}
#[test]
fn grants_bind_both_devices_room_epoch_generation_and_exact_file_identity() -> Result<()> {
    let owner = Keypair::generate_ed25519();
    let source = Keypair::generate_ed25519();
    let receiver = Keypair::generate_ed25519();
    let snapshot = roster(&owner, &source, &receiver)?;
    let generation = Uuid::new_v4();
    let mut manifest = Manifest {
        version: VERSION,
        file_id: Uuid::new_v4(),
        workspace_id: snapshot.workspace_id,
        name: "file.bin".into(),
        size: 3,
        sha256: digest(b"abc"),
        owner_peer_id: source.public().to_peer_id().to_string(),
        owner_public_key: public_key(&source),
        signature: String::new(),
    };
    manifest.sign(&source)?;
    let grant = Grant::new(
        &owner,
        snapshot.clone(),
        receiver.public().to_peer_id().to_string(),
        source.public().to_peer_id().to_string(),
        Purpose::File {
            manifest: manifest.clone(),
        },
        generation,
    )?;
    grant.authorize(
        &snapshot,
        &receiver.public().to_peer_id().to_string(),
        generation,
    )?;
    assert!(grant
        .authorize(
            &snapshot,
            &source.public().to_peer_id().to_string(),
            generation
        )
        .is_err());
    assert!(grant
        .authorize(
            &snapshot,
            &receiver.public().to_peer_id().to_string(),
            Uuid::new_v4()
        )
        .is_err());
    let mut changed = snapshot.clone();
    changed.workspace_id = Uuid::new_v4();
    changed.sign(&owner)?;
    assert!(grant
        .authorize(
            &changed,
            &receiver.public().to_peer_id().to_string(),
            generation
        )
        .is_err());
    let mut changed_grant = grant.clone();
    if let Purpose::File { manifest } = &mut changed_grant.purpose {
        manifest.sha256 = digest(b"bad");
    }
    assert!(changed_grant.verify_at(dump_core::model::now()).is_err());
    assert!(grant.verify_at(grant.expires_at).is_err());
    let mut oversized = manifest;
    oversized.size = MAX_FILE_BYTES + 1;
    oversized.sign(&source)?;
    assert!(Grant::new(
        &owner,
        snapshot,
        receiver.public().to_peer_id().to_string(),
        source.public().to_peer_id().to_string(),
        Purpose::File {
            manifest: oversized
        },
        generation
    )
    .is_err());
    Ok(())
}
#[test]
fn outsider_host_self_loops_and_stale_rosters_cannot_authorize_a_bridge() -> Result<()> {
    let owner = Keypair::generate_ed25519();
    let source = Keypair::generate_ed25519();
    let receiver = Keypair::generate_ed25519();
    let snapshot = roster(&owner, &source, &receiver)?;
    let generation = Uuid::new_v4();
    let from = receiver.public().to_peer_id().to_string();
    let to = source.public().to_peer_id().to_string();
    let grant = Grant::new(
        &owner,
        snapshot.clone(),
        from.clone(),
        to.clone(),
        Purpose::Catalog { offset: 0 },
        generation,
    )?;
    let outsider = Keypair::generate_ed25519()
        .public()
        .to_peer_id()
        .to_string();
    assert!(Grant::new(
        &owner,
        snapshot.clone(),
        outsider,
        to.clone(),
        Purpose::Catalog { offset: 0 },
        generation
    )
    .is_err());
    assert!(Grant::new(
        &owner,
        snapshot.clone(),
        from.clone(),
        from.clone(),
        Purpose::Catalog { offset: 0 },
        generation
    )
    .is_err());
    assert!(Grant::new(
        &owner,
        snapshot.clone(),
        from.clone(),
        snapshot.owner_peer_id.clone(),
        Purpose::Catalog { offset: 0 },
        generation
    )
    .is_err());
    let mut newer = snapshot;
    newer.revision += 1;
    newer.sign(&owner)?;
    assert!(grant.authorize(&newer, &from, generation).is_err());
    Ok(())
}

struct CountingReader(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl futures::AsyncRead for CountingReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        out: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        out.fill(7);
        std::task::Poll::Ready(Ok(out.len()))
    }
}
struct BlockedWriter;
impl futures::AsyncWrite for BlockedWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Pending
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Pending
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Pending
    }
}
#[tokio::test]
async fn stalled_downstream_backpressures_upstream_and_cancellation_releases_it() -> Result<()> {
    use dump_core::host_relay::{pump, Bandwidth, BUFFER_BYTES};
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    };
    let reads = Arc::new(AtomicUsize::new(0));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(pump(
        CountingReader(reads.clone()),
        BlockedWriter,
        Arc::new(AtomicU64::new(10 * BUFFER_BYTES as u64)),
        Arc::new(Bandwidth::default()),
        cancel.clone(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while reads.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    cancel.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await??
            .is_err()
    );
    Ok(())
}
#[tokio::test]
async fn opaque_byte_budget_cannot_underflow_or_forward_over_budget() -> Result<()> {
    use dump_core::host_relay::{pump, Bandwidth};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    let remaining = Arc::new(AtomicU64::new(3));
    let mut output = futures::io::Cursor::new(Vec::new());
    assert!(pump(
        futures::io::Cursor::new(b"four".to_vec()),
        &mut output,
        remaining.clone(),
        Arc::new(Bandwidth::default()),
        tokio_util::sync::CancellationToken::new()
    )
    .await
    .is_err());
    assert_eq!(remaining.load(Ordering::SeqCst), 3);
    assert!(output.into_inner().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approved_peers_without_direct_route_fetch_via_online_owner() -> Result<()> {
    use dump_core::{
        engine::{share_paths, Engine, Shared, View},
        network::Node,
    };
    use std::{sync::Arc, time::Duration};
    async fn wait(shared: &Shared, condition: impl Fn(&View) -> bool) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if condition(&shared.lock().await.view()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .with_context(|| "waiting for isolated relay state")?;
        Ok(())
    }
    async fn join(host: &Node, member: &Node) -> Result<()> {
        let invite = host.shared.lock().await.create_invite()?;
        member.shared.lock().await.join(&invite)?;
        let mut addresses = host.addresses.clone();
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(a) = addresses.borrow().first() {
                    break Ok::<_, anyhow::Error>(a.clone());
                }
                addresses.changed().await?;
            }
        })
        .await??;
        member
            .connect(host.shared.lock().await.peer().parse()?, address)
            .await?;
        let id = member.shared.lock().await.peer();
        let host_id = host.shared.lock().await.peer().parse()?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let probe = member
            .request(
                host_id,
                &dump_core::network::Request::Join {
                    invitation: dump_core::model::Invitation::parse(&invite)?,
                    name: "Isolated device".into(),
                },
            )
            .await;
        if let Err(error) = &probe {
            eprintln!("isolated authenticated join probe failed: {error:#}");
        }
        wait(&host.shared, |v| v.pending.iter().any(|p| p.peer_id == id))
            .await
            .context("owner did not receive isolated join request")?;
        host.shared.lock().await.approve(&id)?;
        wait(&member.shared, |v| v.active.is_some() && !v.joining)
            .await
            .context("member did not receive isolated approval")
    }
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("host"), Arc::new(|_| {}))?;
    let source = Engine::open(&root.path().join("source"), Arc::new(|_| {}))?;
    let receiver = Engine::open(&root.path().join("receiver"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Relay test".into())?;
    let host = Node::start(owner.clone(), false).await?;
    let sender = Node::start(source.clone(), false).await?;
    let client = Node::start(receiver.clone(), false).await?;
    join(&host, &sender).await?;
    join(&host, &client).await?;
    let host_id = owner.lock().await.peer().parse()?;
    sender.refresh_peer(host_id).await?;
    owner.lock().await.set_host_relay(true)?;
    client.refresh_host_status(host_id).await?;
    let file = root.path().join("payload.bin");
    let data = vec![42u8; 192 * 1024 + 7];
    std::fs::write(&file, &data)?;
    share_paths(source.clone(), vec![file]).await?;
    let source_id = source.lock().await.peer().parse()?;
    // Discovery is disabled; only explicit member-to-host links were supplied.
    // No source address is disclosed to the receiver in this test.
    client.refresh_peer(source_id).await?;
    let manifest = receiver
        .lock()
        .await
        .remote
        .values()
        .next()
        .unwrap()
        .clone();
    let dest = root.path().join("download");
    std::fs::create_dir(&dest)?;
    let id = client.receive(manifest.file_id, dest.clone()).await?;
    wait(&receiver, |v| {
        v.transfers.iter().any(|t| {
            t.id == id && ["Completed", "Failed", "Cancelled"].contains(&t.status.as_str())
        })
    })
    .await?;
    let view = receiver.lock().await.view();
    let transfer = view.transfers.iter().find(|t| t.id == id).unwrap();
    assert_eq!(transfer.status, "Completed", "{:?}", transfer);
    assert!(transfer.via_host);
    assert!(source
        .lock()
        .await
        .view()
        .transfers
        .iter()
        .any(|t| t.direction == "Sharing" && t.via_host));
    assert_eq!(std::fs::read(dest.join(&manifest.name))?, data);
    assert!(receiver.lock().await.persisted.partials.is_empty());
    assert!(owner.lock().await.persisted.partials.is_empty());
    // Turning the host service off must not imply an offline file cache.
    owner.lock().await.set_host_relay(false)?;
    assert!(client.refresh_peer(source_id).await.is_err());
    host.shutdown.cancel();
    sender.shutdown.cancel();
    client.shutdown.cancel();
    Ok(())
}
