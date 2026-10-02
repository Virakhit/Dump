#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use dump_core::{
    connectivity::ConnectionState,
    engine::{share_paths, Engine, Shared, View},
    model::*,
    network::{read_frame, write_frame, Node, Request, Response},
};
use futures::StreamExt;
use libp2p::{
    identify,
    multiaddr::Protocol,
    noise, relay,
    swarm::{NetworkBehaviour, StreamProtocol, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
}

async fn relay_server() -> Result<(PeerId, Multiaddr, CancellationToken)> {
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| RelayBehaviour {
            relay: relay::Behaviour::new(
                key.public().to_peer_id(),
                relay::Config {
                    max_reservations: 8,
                    max_reservations_per_peer: 1,
                    max_circuits: 8,
                    max_circuits_per_peer: 4,
                    max_circuit_bytes: 512 * BLOCK as u64,
                    max_circuit_duration: Duration::from_secs(60),
                    ..Default::default()
                },
            ),
            identify: identify::Behaviour::new(
                identify::Config::new("/dump/test".into(), key.public())
                    .with_hide_listen_addrs(true)
                    .with_cache_size(0),
            ),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    swarm.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
    let peer = *swarm.local_peer_id();
    let address = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                return address;
            }
        }
    })
    .await?;
    // A real self-hosted relay must publish its reachable address. Test-only loopback.
    swarm.add_external_address(address.clone());
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! { _ = stop.cancelled() => break, _ = swarm.select_next_some() => {}, }
        }
    });
    Ok((peer, address, shutdown))
}

async fn wait(shared: &Shared, predicate: impl Fn(&View) -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

async fn circuit(node: &Node) -> Result<Multiaddr> {
    let mut addresses = node.relay_addresses.clone();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(address) = addresses.borrow().first() {
                return Ok(address.clone());
            }
            addresses.changed().await?;
        }
    })
    .await
    .context("waiting for relay reservation")?
}

async fn connected(node: &Node, peer: PeerId, state: ConnectionState) -> Result<()> {
    let mut diagnostics = node.diagnostics.clone();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if diagnostics
                .borrow()
                .get(&peer)
                .is_some_and(|d| d.state == state)
            {
                return Ok(());
            }
            diagnostics.changed().await?;
        }
    })
    .await
    .context("waiting for end-peer connection")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn circuit_carries_join_catalog_file_and_direct_takes_over() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Private".into())?;
    member
        .lock()
        .await
        .join(&owner.lock().await.create_invite()?)?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(member.clone(), false).await?;
    let (relay_id, address, stop) = relay_server().await?;
    a.reserve_relay(relay_id, address).await?;
    let route = circuit(&a).await?;
    let owner_id: PeerId = owner.lock().await.peer().parse()?;
    let member_id = member.lock().await.peer();
    b.connect(owner_id, route).await?;
    connected(&b, owner_id, ConnectionState::Relay).await?;
    let mut diagnostics = b.diagnostics.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if diagnostics
                .borrow()
                .get(&owner_id)
                .is_some_and(|d| d.identified)
            {
                break;
            }
            diagnostics.changed().await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert!(
        b.diagnostics.borrow()[&owner_id]
            .advertised_addresses
            .is_empty(),
        "private relay address leaked through Identify"
    );
    wait(&owner, |v| v.pending.iter().any(|p| p.peer_id == member_id)).await?;
    owner.lock().await.approve(&member_id)?;
    wait(&member, |v| v.active.is_some() && !v.joining).await?;
    let source = root.path().join("circuit.bin");
    let payload: Vec<_> = (0..2 * BLOCK + 17).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &payload)?;
    share_paths(owner.clone(), vec![source]).await?;
    wait(&member, |v| !v.files.is_empty()).await?;
    let manifest = member.lock().await.remote.values().next().unwrap().clone();
    let destination = root.path().join("received");
    std::fs::create_dir(&destination)?;
    let transfer = b.receive(manifest.file_id, destination.clone()).await?;
    wait(&member, |v| {
        v.transfers.iter().any(|t| {
            t.id == transfer && ["Completed", "Failed", "Cancelled"].contains(&t.status.as_str())
        })
    })
    .await?;
    {
        let e = member.lock().await;
        ensure!(
            e.transfers[&transfer].status == "Completed",
            "circuit transfer failed: {:?}",
            e.transfers[&transfer]
        );
    }
    ensure!(
        std::fs::read(destination.join(&manifest.name))? == payload,
        "received file bytes differ"
    );
    assert!(member.lock().await.persisted.partials.is_empty());
    assert_eq!(
        a.diagnostics.borrow()[&member_id.parse()?].hole_punch_succeeded,
        Some(false),
        "private candidates must keep the circuit fallback"
    );
    // The relay's real public identity is not a workspace member.
    let outsider = Engine::open(&root.path().join("outsider"), Arc::new(|_| {}))?;
    let c = Node::start(outsider, false).await?;
    c.connect(owner_id, circuit(&a).await?).await?;
    connected(&c, owner_id, ConnectionState::Relay).await?;
    let snapshot = owner.lock().await.active_snapshot()?;
    assert!(matches!(
        c.request(
            owner_id,
            &Request::Catalog {
                snapshot: snapshot.clone(),
                offset: 0
            }
        )
        .await?,
        Response::Denied
    ));
    let mut control = c.stream_control(owner_id);
    let mut stream = control
        .open_stream(owner_id, StreamProtocol::new("/dump/file/1"))
        .await?;
    write_frame(
        &mut stream,
        &Request::File {
            snapshot: snapshot.clone(),
            manifest,
        },
    )
    .await?;
    assert!(matches!(
        read_frame::<_, Response>(&mut stream).await?,
        Response::Denied
    ));
    assert!(dump_core::network::handle_control(
        owner.clone(),
        relay_id,
        Request::Catalog {
            snapshot,
            offset: 0
        }
    )
    .await
    .is_err());
    // Register a direct route alongside the existing circuit, then stop the relay.
    let direct = a
        .addresses
        .borrow()
        .iter()
        .find(|a| {
            a.iter().any(|p| p == Protocol::QuicV1) && !a.iter().any(|p| p == Protocol::P2pCircuit)
        })
        .unwrap()
        .clone();
    b.connect(owner_id, direct).await?;
    connected(&b, owner_id, ConnectionState::Direct).await?;
    stop.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    b.refresh_peer(owner_id).await?;
    assert_eq!(
        b.diagnostics.borrow()[&owner_id].state,
        ConnectionState::Direct
    );
    a.shutdown.cancel();
    b.shutdown.cancel();
    c.shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_loss_during_transfer_never_commits_a_partial_file() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Loss".into())?;
    member
        .lock()
        .await
        .join(&owner.lock().await.create_invite()?)?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(member.clone(), false).await?;
    let (relay_id, address, stop) = relay_server().await?;
    a.reserve_relay(relay_id, address).await?;
    let owner_id: PeerId = owner.lock().await.peer().parse()?;
    b.connect(owner_id, circuit(&a).await?).await?;
    let member_id = member.lock().await.peer();
    wait(&owner, |v| v.pending.iter().any(|p| p.peer_id == member_id)).await?;
    owner.lock().await.approve(&member_id)?;
    wait(&member, |v| v.active.is_some() && !v.joining).await?;
    let source = root.path().join("unfinished.bin");
    std::fs::File::create(&source)?.set_len(64 * BLOCK as u64)?;
    share_paths(owner.clone(), vec![source]).await?;
    wait(&member, |v| !v.files.is_empty()).await?;
    let manifest = member.lock().await.remote.values().next().unwrap().clone();
    let destination = root.path().join("received");
    std::fs::create_dir(&destination)?;
    let transfer = b.receive(manifest.file_id, destination.clone()).await?;
    let part = destination.join(format!(".dump-{transfer}.part"));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::metadata(&part).is_ok_and(|m| m.len() > 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await?;
    stop.cancel();
    wait(&member, |v| {
        v.transfers
            .iter()
            .any(|t| t.id == transfer && ["Failed", "Cancelled"].contains(&t.status.as_str()))
    })
    .await?;
    assert!(!destination.join(&manifest.name).exists());
    assert!(!part.exists());
    assert!(member.lock().await.persisted.partials.is_empty());
    a.shutdown.cancel();
    b.shutdown.cancel();
    Ok(())
}
