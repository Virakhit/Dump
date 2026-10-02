#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use dump_core::{
    connectivity::{ConnectionState, PeerDiagnostics},
    engine::{share_paths, Engine, Shared, View},
    model::Invitation,
    network::{read_frame, write_frame, Node, Request, Response},
};
use libp2p::{swarm::StreamProtocol, Multiaddr, PeerId};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Default)]
struct Nodes(Vec<Node>);
impl Drop for Nodes {
    fn drop(&mut self) {
        for node in &self.0 {
            node.shutdown.cancel();
        }
    }
}
impl Nodes {
    async fn close(&self) -> Result<()> {
        for node in &self.0 {
            node.shutdown.cancel();
        }
        for node in &self.0 {
            tokio::time::timeout(Duration::from_secs(10), node.shutdown_and_wait())
                .await
                .context("LAN test node cleanup stage")?;
        }
        Ok(())
    }
}

async fn lan_listener(node: &Node) -> Result<()> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if addresses.borrow().iter().any(|address| {
                matches!(address.iter().next(), Some(libp2p::multiaddr::Protocol::Ip4(ip)) if !ip.is_loopback())
                    && address.iter().any(|p| p == libp2p::multiaddr::Protocol::QuicV1)
            }) { return Ok::<_, anyhow::Error>(()); }
            addresses.changed().await?;
        }
    }).await.context("LAN non-loopback QUIC listener readiness stage")??;
    Ok(())
}

async fn lan_state(
    stage: &str,
    deadline: Duration,
    shared: &Shared,
    changes: &mut watch::Receiver<()>,
    observer: &Node,
    remote: PeerId,
    predicate: impl Fn(&View) -> bool,
) -> Result<()> {
    let started = Instant::now();
    let mut diagnostics = observer.diagnostics.clone();
    let result = tokio::time::timeout(deadline, async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::select! {
                changed = changes.changed() => changed?,
                changed = diagnostics.changed() => changed?,
            }
        }
    })
    .await;
    if result.is_err() {
        let view = shared.lock().await.view();
        let peer = diagnostics
            .borrow()
            .get(&remote)
            .cloned()
            .unwrap_or_default();
        anyhow::bail!("stage={stage} timeout after {:?}: listeners={} network={} joining={} pending={} route={:?} identified={} protocol={:?} outcome={:?} dial={:?}", started.elapsed(), observer.addresses.borrow().len(), view.network_status, view.joining, view.pending.len(), peer.state, peer.identified, peer.last_protocol, peer.protocol_outcome, peer.last_failure);
    }
    result??;
    eprintln!("stage={stage} elapsed={:?}", started.elapsed());
    Ok(())
}

async fn address(node: &Node) -> Result<Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(address) = addresses
                .borrow()
                .iter()
                .find(|a| a.iter().any(|p| p == libp2p::multiaddr::Protocol::QuicV1))
            {
                return Ok(address.clone());
            }
            addresses.changed().await?;
        }
    })
    .await?
}

async fn diagnostic(
    node: &Node,
    peer: PeerId,
    predicate: impl Fn(&PeerDiagnostics) -> bool,
) -> Result<PeerDiagnostics> {
    let mut diagnostics = node.diagnostics.clone();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(value) = diagnostics.borrow().get(&peer).filter(|d| predicate(d)) {
                return Ok(value.clone());
            }
            diagnostics.changed().await?;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_direct_addresses_identify_and_nonmember_denials() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let outsider = Engine::open(&root.path().join("outsider"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Private".into())?;
    let file = root.path().join("private.txt");
    std::fs::write(&file, b"private")?;
    share_paths(owner.clone(), vec![file]).await?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(outsider.clone(), false).await?;
    let owner_id: PeerId = owner.lock().await.peer().parse()?;
    let outsider_id: PeerId = outsider.lock().await.peer().parse()?;
    // Same path as a known Internet QUIC address, simulated with loopback; no public service.
    b.connect(owner_id, address(&a).await?).await?;
    let info = diagnostic(&b, owner_id, |d| d.identified).await?;
    assert_eq!(info.state, ConnectionState::Direct);
    assert!(
        info.advertised_addresses.is_empty(),
        "Identify leaked interface addresses"
    );
    assert!(
        info.observed_address.is_none(),
        "loopback is not a public candidate"
    );
    let info = diagnostic(&a, outsider_id, |d| d.identified).await?;
    assert_eq!(info.state, ConnectionState::Direct);
    let snapshot = owner.lock().await.active_snapshot()?;
    assert!(matches!(
        b.request(
            owner_id,
            &Request::Catalog {
                snapshot: snapshot.clone(),
                offset: 0
            }
        )
        .await?,
        Response::Denied
    ));
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
    let mut control = b.control.clone();
    let mut stream = control
        .open_stream(owner_id, StreamProtocol::new("/dump/file/1"))
        .await?;
    write_frame(&mut stream, &Request::File { snapshot, manifest }).await?;
    assert!(matches!(
        read_frame::<_, Response>(&mut stream).await?,
        Response::Denied
    ));
    assert!(!owner
        .lock()
        .await
        .online
        .contains_key(&outsider_id.to_string()));
    assert!(owner.lock().await.transfers.is_empty());
    assert!(outsider.lock().await.persisted.workspaces.is_empty());
    assert!(!owner.lock().await.network_status.contains("Connected"));
    a.shutdown.cancel();
    diagnostic(&b, owner_id, |d| d.state == ConnectionState::Offline).await?;
    b.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mismatched_transport_identity_never_connects() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let client = Engine::open(&root.path().join("client"), Arc::new(|_| {}))?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(client, false).await?;
    let wrong = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    let target = address(&a).await?;
    // Reject a conflicting address suffix before queueing any network work.
    let owner_id = owner.lock().await.key.public().to_peer_id();
    assert!(b
        .connect(
            wrong,
            target
                .clone()
                .with(libp2p::multiaddr::Protocol::P2p(owner_id))
        )
        .await
        .is_err());
    // Also authenticate the expected ID when there is no suffix.
    b.connect(wrong, target).await?;
    let info = diagnostic(&b, wrong, |d| d.last_failure.is_some()).await?;
    assert_eq!(info.state, ConnectionState::Offline);
    assert!(!info.identified);
    assert!(b.diagnostics.borrow().get(&owner_id).is_none());
    assert!(b
        .request(
            wrong,
            &Request::Join {
                invitation: Invitation {
                    version: 1,
                    workspace_id: uuid::Uuid::new_v4(),
                    owner_peer_id: wrong.to_string(),
                    token: "invalid".into(),
                    contact: None,
                },
                name: "Client".into(),
            }
        )
        .await
        .is_err());
    a.shutdown.cancel();
    b.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_mdns_still_finds_invited_owner_without_explicit_addresses() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner_changes, mut owner_updates) = watch::channel(());
    let (member_changes, mut member_updates) = watch::channel(());
    let owner = Engine::open(
        &root.path().join("owner"),
        Arc::new(move |_| {
            owner_changes.send_replace(());
        }),
    )?;
    let member = Engine::open(
        &root.path().join("member"),
        Arc::new(move |_| {
            member_changes.send_replace(());
        }),
    )?;
    let mut nodes = Nodes::default();
    let result = async {
        owner.lock().await.create_workspace("LAN".into())?;
        member
            .lock()
            .await
            .join(&owner.lock().await.create_invite()?)?;
        let a = Node::start(owner.clone(), true).await?;
        nodes.0.push(a.clone());
        // mDNS advertises only transport addresses matching its non-loopback interface.
        // Make the owner's listener real before the joining node starts discovering.
        lan_listener(&a).await?;
        let b = Node::start(member.clone(), true).await?;
        nodes.0.push(b.clone());
        lan_listener(&b).await?;
        let member_id = member.lock().await.peer();
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        lan_state(
            "mDNS owner discovery / application join request",
            Duration::from_secs(30),
            &owner,
            &mut owner_updates,
            &b,
            owner_peer,
            |view| {
                view.pending
                    .iter()
                    .any(|pending| pending.peer_id == member_id)
            },
        )
        .await?;
        owner.lock().await.approve(&member_id)?;
        lan_state(
            "LAN signed approval",
            Duration::from_secs(15),
            &member,
            &mut member_updates,
            &b,
            owner_peer,
            |view| view.active.is_some() && !view.joining,
        )
        .await?;
        ensure!(
            member.lock().await.active_snapshot()?.contains(&member_id),
            "LAN approval missing"
        );
        Ok(())
    }
    .await;
    let closed = nodes.close().await;
    match (result, closed) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("LAN cleanup also failed: {cleanup:#}")))
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_tcp_route_supports_the_same_workspace_protocol() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("TCP fallback".into())?;
    member
        .lock()
        .await
        .join(&owner.lock().await.create_invite()?)?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(member.clone(), false).await?;
    let owner_id: PeerId = owner.lock().await.peer().parse()?;
    let mut addresses = a.addresses.clone();
    let tcp = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(address) = addresses.borrow().iter().find(|a| {
                a.iter()
                    .any(|p| matches!(p, libp2p::multiaddr::Protocol::Tcp(_)))
            }) {
                return Ok::<_, anyhow::Error>(address.clone());
            }
            addresses.changed().await?;
        }
    })
    .await??;
    b.connect(owner_id, tcp).await?;
    diagnostic(&b, owner_id, |d| {
        d.state == ConnectionState::Direct && d.identified
    })
    .await?;
    let member_id = member.lock().await.peer();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if owner.lock().await.pending.contains_key(&member_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    owner.lock().await.approve(&member_id)?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if member.lock().await.persisted.active.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    b.refresh_peer(owner_id).await?;
    assert!(member
        .lock()
        .await
        .online
        .contains_key(&owner_id.to_string()));
    a.shutdown.cancel();
    b.shutdown.cancel();
    Ok(())
}
