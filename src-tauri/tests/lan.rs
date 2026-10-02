#![cfg(windows)]
use anyhow::{ensure, Result};
use dump_core::{
    engine::{refresh_share, share_paths, Engine, Shared, View},
    model::*,
    network::{handle_control, Node, Request, Response},
};
use libp2p::PeerId;
use std::{sync::Arc, time::Duration};

async fn wait(shared: &Shared, predicate: impl Fn(&View) -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    Ok(())
}
async fn address(node: &Node) -> Result<libp2p::Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(addr) = addresses
                .borrow()
                .iter()
                .find(|a| a.iter().any(|p| p == libp2p::multiaddr::Protocol::QuicV1))
            {
                return Ok(addr.clone());
            }
            addresses.changed().await?;
        }
    })
    .await?
}
async fn join(owner: &Node, member: &Node) -> Result<()> {
    let invitation = owner.shared.lock().await.create_invite()?;
    member.shared.lock().await.join(&invitation)?;
    let owner_id: PeerId = owner.shared.lock().await.peer().parse()?;
    let member_id = member.shared.lock().await.peer();
    member.connect(owner_id, address(owner).await?).await?;
    wait(&owner.shared, |v| {
        v.pending.iter().any(|p| p.peer_id == member_id)
    })
    .await?;
    owner.shared.lock().await.approve(&member_id)?;
    wait(&member.shared, |v| v.active.is_some() && !v.joining).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_quic_file_loop_and_revocation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let a = Engine::open(&root.path().join("a"), Arc::new(|_| {}))?;
    let b = Engine::open(&root.path().join("b"), Arc::new(|_| {}))?;
    let c = Engine::open(&root.path().join("c"), Arc::new(|_| {}))?;
    a.lock().await.set_name("Owner".into())?;
    b.lock().await.set_name("Sender".into())?;
    c.lock().await.set_name("Receiver".into())?;
    a.lock().await.create_workspace("Friends".into())?;
    let na = Node::start(a.clone(), false).await?;
    let nb = Node::start(b.clone(), false).await?;
    let nc = Node::start(c.clone(), false).await?;
    join(&na, &nb).await?;
    let old = b.lock().await.active_snapshot()?;
    join(&na, &nc).await?;
    let b_id: PeerId = b.lock().await.peer().parse()?;
    let c_id = c.lock().await.peer();
    nc.connect(b_id, address(&nb).await?).await?;
    // connect queues a dial; wait for authenticated admission before requesting a stream.
    wait(&b, |v| v.online.contains(&c_id)).await?;
    // The newcomer's signed snapshot authorizes it on a peer that only knows an older roster.
    nc.refresh_peer(b_id).await?;
    assert_eq!(
        b.lock().await.active_snapshot()?.revision,
        c.lock().await.active_snapshot()?.revision
    );
    assert!(b.lock().await.accept_snapshot(&old).is_err());
    let current = a.lock().await.active_snapshot()?;
    let outsider = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    assert!(handle_control(
        a.clone(),
        outsider,
        Request::Catalog {
            snapshot: current.clone(),
            offset: 0
        }
    )
    .await
    .is_err());
    let mut conflict = current.clone();
    conflict.name = "Forged".into();
    conflict.sign(&a.lock().await.key)?;
    assert!(b.lock().await.accept_snapshot(&conflict).is_err());
    na.shutdown.cancel();
    let source = root.path().join("รายงาน.bin");
    let payload: Vec<u8> = (0..3 * BLOCK + 17).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &payload)?;
    share_paths(b.clone(), vec![source.clone()]).await?;
    nc.refresh_peer(b_id).await?;
    let manifest = c.lock().await.remote.values().next().unwrap().clone();
    let destination = root.path().join("received");
    std::fs::create_dir(&destination)?;
    let transfer = nc.receive(manifest.file_id, destination.clone()).await?;
    wait(&c, |v| {
        v.transfers
            .iter()
            .any(|t| t.id == transfer && ["Completed", "Failed"].contains(&t.status.as_str()))
    })
    .await?;
    let state = c.lock().await.view();
    let completed = state.transfers.iter().find(|t| t.id == transfer).unwrap();
    ensure!(
        completed.status == "Completed",
        "transfer failed: {:?}",
        completed
    );
    assert_eq!(std::fs::read(destination.join(&manifest.name))?, payload);
    assert!(c.lock().await.persisted.partials.is_empty());
    // Existing files are preserved rather than overwritten.
    let duplicate = nc.receive(manifest.file_id, destination.clone()).await?;
    wait(&c, |v| {
        v.transfers
            .iter()
            .any(|t| t.id == duplicate && t.status == "Failed")
    })
    .await?;
    assert_eq!(std::fs::read(destination.join(&manifest.name))?, payload);
    let cancelled_dir = root.path().join("cancelled");
    std::fs::create_dir(&cancelled_dir)?;
    let cancelled = nc.receive(manifest.file_id, cancelled_dir.clone()).await?;
    c.lock().await.cancel(cancelled)?;
    wait(&c, |v| {
        v.transfers
            .iter()
            .any(|t| t.id == cancelled && t.status == "Cancelled")
    })
    .await?;
    assert!(!cancelled_dir.join(&manifest.name).exists());
    // Source edits cannot be accepted under an old manifest, including edits of equal size.
    std::fs::write(&source, vec![7u8; payload.len()])?;
    let changed_dir = root.path().join("changed");
    std::fs::create_dir(&changed_dir)?;
    let changed = nc.receive(manifest.file_id, changed_dir.clone()).await?;
    wait(&c, |v| {
        v.transfers
            .iter()
            .any(|t| t.id == changed && t.status == "Failed")
    })
    .await?;
    assert!(!changed_dir.join(&manifest.name).exists());
    // Isolated peers keep old membership until an authentic new revision arrives.
    a.lock().await.remove_member(&c_id)?;
    let removed = a.lock().await.active_snapshot()?;
    assert!(b.lock().await.active_snapshot()?.contains(&c_id));
    b.lock().await.accept_snapshot(&removed)?;
    assert!(handle_control(
        b.clone(),
        c_id.parse()?,
        Request::Catalog {
            snapshot: current,
            offset: 0
        }
    )
    .await
    .is_err());
    c.lock().await.accept_snapshot(&removed)?;
    assert!(!c.lock().await.active_snapshot()?.contains(&c_id));
    nb.shutdown.cancel();
    nc.shutdown.cancel();
    Ok(())
}

#[tokio::test]
async fn invitations_frames_and_empty_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    let shared = Engine::open(root.path(), Arc::new(|_| {}))?;
    let mut e = shared.lock().await;
    e.create_workspace("Checks".into())?;
    let url = e.create_invite()?;
    let invitation = Invitation::parse(&url)?;
    assert!(Invitation::parse("dump://join/garbage").is_err());
    let peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    drop(e);
    assert!(matches!(
        handle_control(
            shared.clone(),
            peer,
            Request::Join {
                invitation: invitation.clone(),
                name: "Friend".into()
            }
        )
        .await?,
        Response::Waiting
    ));
    shared.lock().await.approve(&peer.to_string())?;
    let other = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    assert!(handle_control(
        shared.clone(),
        other,
        Request::Join {
            invitation: invitation.clone(),
            name: "Intruder".into()
        }
    )
    .await
    .is_err());
    shared.lock().await.persisted.invites[0].expires_at = 0;
    assert!(handle_control(
        shared.clone(),
        peer,
        Request::Join {
            invitation,
            name: "Friend".into()
        }
    )
    .await
    .is_err());
    let mut oversized = futures::io::Cursor::new(((MAX_CONTROL + 1) as u32).to_be_bytes().to_vec());
    assert!(
        dump_core::network::read_frame::<_, Response>(&mut oversized)
            .await
            .is_err()
    );
    let empty = root.path().join("empty.txt");
    std::fs::write(&empty, [])?;
    share_paths(shared.clone(), vec![empty]).await?;
    let e = shared.lock().await;
    let manifest = &e.persisted.shares.values().next().unwrap().manifest;
    assert_eq!(manifest.size, 0);
    assert_eq!(manifest.sha256, digest(b""));
    manifest.verify()?;
    let loaded = e.store.load()?;
    assert_eq!(loaded.peer_id()?, e.peer());
    Ok(())
}

#[tokio::test]
async fn stale_refresh_cannot_cross_workspaces_or_undo_unshare() -> Result<()> {
    let root = tempfile::tempdir()?;
    let shared = Engine::open(root.path(), Arc::new(|_| {}))?;
    shared.lock().await.create_workspace("Private".into())?;
    let private_id = shared.lock().await.active_snapshot()?.workspace_id;
    let file = root.path().join("private.txt");
    std::fs::write(&file, b"private")?;
    share_paths(shared.clone(), vec![file.clone()]).await?;
    let original = shared
        .lock()
        .await
        .persisted
        .shares
        .values()
        .next()
        .unwrap()
        .clone();
    let attacker = libp2p::identity::Keypair::generate_ed25519();
    let forged = Invitation {
        version: VERSION,
        workspace_id: private_id,
        owner_peer_id: attacker.public().to_peer_id().to_string(),
        token: random_secret(),
        contact: None,
    };
    assert!(shared.lock().await.join(&forged.url()?).is_err());
    shared.lock().await.create_workspace("Public".into())?;
    std::fs::write(&file, b"changed")?;
    refresh_share(shared.clone(), original.clone()).await?;
    assert!(shared.lock().await.view().files.is_empty());
    shared.lock().await.activate(private_id)?;
    let peer = shared.lock().await.peer();
    let (_, cancel) = shared
        .lock()
        .await
        .register_transfer(&original.manifest, &peer, "Sharing");
    shared.lock().await.unshare(original.manifest.file_id)?;
    assert!(cancel.is_cancelled());
    refresh_share(shared.clone(), original).await?;
    assert!(shared.lock().await.persisted.shares.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paginated_catalog_and_cancelled_disk_creation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let a = Engine::open(&root.path().join("a"), Arc::new(|_| {}))?;
    let b = Engine::open(&root.path().join("b"), Arc::new(|_| {}))?;
    a.lock().await.create_workspace("Paging".into())?;
    let na = Node::start(a.clone(), false).await?;
    let nb = Node::start(b.clone(), false).await?;
    join(&na, &nb).await?;
    let source = root.path().join("cancel.bin");
    std::fs::File::create(&source)?.set_len(64 * BLOCK as u64)?;
    share_paths(a.clone(), vec![source]).await?;
    let original = a
        .lock()
        .await
        .persisted
        .shares
        .values()
        .next()
        .unwrap()
        .clone();
    {
        let mut e = a.lock().await;
        let mut next = e.persisted.clone();
        for i in 0..383 {
            let mut share = original.clone();
            share.manifest.file_id = uuid::Uuid::new_v4();
            share.manifest.name = format!("catalog-{i}.bin");
            share.manifest.sign(&e.key)?;
            next.shares.insert(share.manifest.file_id, share);
        }
        e.persist(next)?;
    }
    // Background refresh must complete all 12 pages without hitting the request budget.
    wait(&b, |v| v.files.len() == 384).await?;
    for i in 0..8 {
        let directory = root.path().join(format!("cancel-{i}"));
        std::fs::create_dir(&directory)?;
        let id = nb
            .receive(original.manifest.file_id, directory.clone())
            .await?;
        let part = directory.join(format!(".dump-{id}.part"));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut e = b.lock().await;
                if e.persisted.partials.contains(&part) {
                    e.cancel(id)?;
                    return Ok::<_, anyhow::Error>(());
                }
                drop(e);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await??;
        wait(&b, |v| {
            v.transfers
                .iter()
                .any(|t| t.id == id && t.status == "Cancelled")
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!part.exists(), "cancelled disk creation left a partial");
        assert!(!directory.join(&original.manifest.name).exists());
        assert!(!b.lock().await.persisted.partials.contains(&part));
    }
    // A busy recorded partial stays recorded until a subsequent startup can clean it.
    let part = root
        .path()
        .join(format!(".dump-{}.part", uuid::Uuid::new_v4()));
    std::fs::write(&part, b"unfinished")?;
    let held = dump_core::engine::open_source(&part)?;
    {
        let mut e = b.lock().await;
        let mut next = e.persisted.clone();
        next.partials.push(part.clone());
        e.persist(next)?;
    }
    let reopened = Engine::open(&root.path().join("b"), Arc::new(|_| {}))?;
    assert!(reopened.lock().await.persisted.partials.contains(&part));
    drop(held);
    let reopened = Engine::open(&root.path().join("b"), Arc::new(|_| {}))?;
    assert!(!part.exists());
    assert!(!reopened.lock().await.persisted.partials.contains(&part));
    na.shutdown.cancel();
    nb.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Writes and transfers 8 GiB; requires at least 18 GiB of free temporary disk space"]
async fn eight_gib_streaming_transfer() -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let root = tempfile::tempdir()?;
    let source = root.path().join("eight-gib.bin");
    let block: Vec<u8> = (0..BLOCK).map(|i| (i % 251) as u8).collect();
    let started = std::time::Instant::now();
    let mut file = std::fs::File::create(&source)?;
    for _ in 0..8192 {
        file.write_all(&block)?;
    }
    file.sync_all()?;
    drop(file);
    eprintln!("8 GiB source written in {:?}", started.elapsed());
    let a = Engine::open(&root.path().join("a"), Arc::new(|_| {}))?;
    let b = Engine::open(&root.path().join("b"), Arc::new(|_| {}))?;
    a.lock().await.create_workspace("Large file check".into())?;
    let na = Node::start(a.clone(), false).await?;
    let nb = Node::start(b.clone(), false).await?;
    join(&na, &nb).await?;
    share_paths(a.clone(), vec![source.clone()]).await?;
    wait(&b, |v| !v.files.is_empty()).await?;
    let manifest = b.lock().await.remote.values().next().unwrap().clone();
    assert_eq!(manifest.size, 8 * 1024 * 1024 * 1024);
    let destination = root.path().join("received");
    std::fs::create_dir(&destination)?;
    let transfer_started = std::time::Instant::now();
    let id = nb.receive(manifest.file_id, destination.clone()).await?;
    tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            let e = b.lock().await;
            let transfer = e.transfers.get(&id).unwrap();
            if transfer.status == "Completed" {
                return Ok::<_, anyhow::Error>(());
            }
            ensure!(
                transfer.status != "Failed",
                "8 GiB transfer failed: {:?}",
                transfer
            );
            drop(e);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    eprintln!(
        "8 GiB QUIC transfer completed in {:?}",
        transfer_started.elapsed()
    );
    let received = destination.join(&manifest.name);
    assert_eq!(std::fs::metadata(&received)?.len(), manifest.size);
    let mut file = std::fs::File::open(received)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; BLOCK];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    assert_eq!(hex::encode(hash.finalize()), manifest.sha256);
    assert!(b.lock().await.persisted.partials.is_empty());
    drop(file);
    na.shutdown.cancel();
    nb.shutdown.cancel();
    eprintln!(
        "8 GiB size/hash/no-partial checks passed; total {:?}",
        started.elapsed()
    );
    Ok(())
}
