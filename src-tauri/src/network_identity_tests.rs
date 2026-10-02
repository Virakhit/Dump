use super::*;
use crate::{
    contact::Contact,
    engine::{share_paths, Engine, View},
};

#[derive(Default)]
struct NetworkCleanup {
    nodes: Vec<Node>,
    hosts: Vec<(CancellationToken, tokio::task::JoinHandle<()>)>,
}
impl NetworkCleanup {
    async fn close(&mut self) -> Result<()> {
        for node in &self.nodes {
            node.shutdown.cancel();
        }
        for (stop, _) in &self.hosts {
            stop.cancel();
        }
        for node in &self.nodes {
            tokio::time::timeout(Duration::from_secs(10), node.shutdown_and_wait())
                .await
                .context("identity node cleanup timed out")?;
        }
        for (_, task) in self.hosts.drain(..) {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .context("identity relay cleanup timed out")??;
        }
        Ok(())
    }
}
impl Drop for NetworkCleanup {
    fn drop(&mut self) {
        for node in &self.nodes {
            node.shutdown.cancel();
        }
        for (stop, task) in &self.hosts {
            stop.cancel();
            task.abort();
        }
    }
}

fn engine(directory: &Path) -> Result<(Shared, watch::Receiver<Option<View>>)> {
    let (updates, receiver) = watch::channel(None);
    let shared = Engine::open(
        directory,
        Arc::new(move |view| {
            updates.send_replace(Some(view));
        }),
    )?;
    Ok((shared, receiver))
}

async fn state_stage(
    stage: &str,
    shared: &Shared,
    updates: &mut watch::Receiver<Option<View>>,
    node: &Node,
    predicate: impl Fn(&View) -> bool,
) -> Result<()> {
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let view = shared.lock().await.view();
            if let Some(transfer) = view
                .transfers
                .iter()
                .find(|t| ["Failed", "Cancelled"].contains(&t.status.as_str()))
            {
                bail!(
                    "{stage}: transfer {}: {}",
                    transfer.status,
                    transfer.error.as_deref().unwrap_or("no error")
                );
            }
            if predicate(&view) {
                return Ok::<_, anyhow::Error>(());
            }
            updates.changed().await?;
        }
    })
    .await;
    if result.is_err() {
        let view = shared.lock().await.view();
        let connections: Vec<_> = node
            .diagnostics
            .borrow()
            .values()
            .map(|d| (d.state, d.identified, d.supports_contacts, d.last_failure))
            .collect();
        bail!("{stage} timed out after {:?}: network={}, joining={}, pending={}, files={}, connections={connections:?}", started.elapsed(), view.network_status, view.joining, view.pending.len(), view.files.len());
    }
    result??;
    eprintln!("identity stage {stage}: {:?}", started.elapsed());
    Ok(())
}

async fn node_address(node: &Node) -> Result<Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(address) = addresses.borrow().iter().find(|a| !is_relayed(a)).cloned() {
                return Ok::<_, anyhow::Error>(address);
            }
            addresses.changed().await?;
        }
    })
    .await?
}

async fn relay_reservation(node: &Node, peer: PeerId, address: Multiaddr) -> Result<()> {
    let mut addresses = node.relay_addresses.clone();
    node.reserve_relay(peer, address).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !addresses.borrow().is_empty() {
                return Ok::<_, anyhow::Error>(());
            }
            addresses.changed().await?;
        }
    })
    .await
    .context("relay reservation stage timed out")??;
    Ok(())
}

async fn app_connection(node: &Node, peer: PeerId, expected: ConnectionState) -> Result<()> {
    let mut diagnostics = node.diagnostics.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if diagnostics
                .borrow()
                .get(&peer)
                .is_some_and(|d| d.state == expected)
            {
                return Ok::<_, anyhow::Error>(());
            }
            diagnostics.changed().await?;
        }
    })
    .await
    .context("application connection stage timed out")??;
    Ok(())
}

async fn owner_also_hosts_relay(relay_first: bool) -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, mut owner_updates) = engine(&root.path().join("owner"))?;
    let (member, mut member_updates) = engine(&root.path().join("member"))?;
    owner
        .lock()
        .await
        .create_workspace("Service endpoints".into())?;
    let mut invite = Invitation::parse(&owner.lock().await.create_invite()?)?;
    let relay_key = owner.lock().await.relay_key.clone();
    let relay_peer = relay_key.public().to_peer_id();
    let mut relay = crate::relay_host::swarm(relay_key, crate::relay_host::config())?;
    relay.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
    let relay_address = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = relay.select_next_some().await {
                break address;
            }
        }
    })
    .await
    .context("relay listen stage timed out")?;
    relay.add_external_address(relay_address.clone());
    let stop = CancellationToken::new();
    let task_stop = stop.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! { _ = task_stop.cancelled() => break, _ = relay.select_next_some() => {} }
        }
    });
    let mut cleanup = NetworkCleanup {
        hosts: vec![(stop, task)],
        nodes: Vec::new(),
    };
    let result = async {
        let a = Node::start(owner.clone(), false).await?;
        cleanup.nodes.push(a.clone());
        let b = Node::start_inner(member.clone(), false, true).await?;
        cleanup.nodes.push(b.clone());
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        assert_ne!(
            relay_peer, owner_peer,
            "relay service must have a distinct persisted endpoint identity"
        );
        let member_peer = member.lock().await.peer();
        let app_address = node_address(&a).await?;
        let owner_route = if relay_first {
            // The owner app can reserve its co-hosted relay only when the two
            // authenticated service endpoints have distinct persisted identities.
            relay_reservation(&a, relay_peer, relay_address.clone()).await?;
            let circuit = a
                .relay_addresses
                .borrow()
                .first()
                .cloned()
                .context("missing owner circuit locator")?;
            let route = peer_address(owner_peer, circuit)?
                .with(libp2p::multiaddr::Protocol::P2p(owner_peer));
            assert!(route
                .iter()
                .any(|p| p == libp2p::multiaddr::Protocol::P2p(relay_peer)));
            assert_eq!(
                route.iter().last(),
                Some(libp2p::multiaddr::Protocol::P2p(owner_peer))
            );
            route
        } else {
            app_address.clone()
        };
        let expected_route = if relay_first {
            ConnectionState::Relay
        } else {
            ConnectionState::Direct
        };
        invite.contact = Some(Contact::issue_routes(
            &owner.lock().await.key,
            1,
            vec![owner_route.to_string()],
            true,
        )?);
        if relay_first {
            relay_reservation(&b, relay_peer, relay_address.clone()).await?;
            eprintln!("identity relay-first: reservation ready; app-route dial requested");
            b.connect(owner_peer, owner_route.clone()).await?;
            app_connection(&b, owner_peer, expected_route).await?;
            // A transport connection to the forwarding service cannot stand in for
            // successful application-protocol negotiation at the owner's app endpoint.
            let result = b
                .request(
                    owner_peer,
                    &Request::Join {
                        invitation: invite.clone(),
                        name: "Member".into(),
                    },
                )
                .await;
            ensure!(
                matches!(result, Ok(Response::Waiting)),
                "relay-first application Join stage: {}",
                result
                    .err()
                    .map_or_else(|| "unexpected response".into(), |e| format!("{e:#}"))
            );
        }
        {
            let mut e = member.lock().await;
            let mut next = e.persisted.clone();
            next.joining = Some(invite);
            e.persist(next)?;
            e.emit();
        }
        state_stage("owner join request", &owner, &mut owner_updates, &b, |v| {
            v.pending.iter().any(|p| p.peer_id == member_peer)
        })
        .await?;
        owner.lock().await.approve(&member_peer)?;
        assert!(
            !owner
                .lock()
                .await
                .active_snapshot()?
                .contains(&relay_peer.to_string()),
            "co-hosting a relay must not grant workspace membership"
        );
        state_stage("member approval", &member, &mut member_updates, &b, |v| {
            v.active.is_some() && !v.joining
        })
        .await?;
        if !relay_first {
            relay_reservation(&b, relay_peer, relay_address).await?;
        }
        let payload = vec![41; BLOCK + 31];
        let source = root.path().join("owner-and-relay.bin");
        tokio::fs::write(&source, &payload).await?;
        share_paths(owner.clone(), vec![source]).await?;
        state_stage(
            "authorized catalog",
            &member,
            &mut member_updates,
            &b,
            |v| v.files.iter().any(|f| !f.mine),
        )
        .await?;
        let manifest = member
            .lock()
            .await
            .remote
            .values()
            .next()
            .context("missing manifest")?
            .clone();
        let output = root.path().join("output");
        tokio::fs::create_dir(&output).await?;
        let id = b.receive(manifest.file_id, output.clone()).await?;
        state_stage(
            "verified file transfer",
            &member,
            &mut member_updates,
            &b,
            |v| {
                v.transfers
                    .iter()
                    .any(|t| t.id == id && t.status == "Completed")
            },
        )
        .await?;
        ensure!(
            tokio::fs::read(output.join("owner-and-relay.bin")).await? == payload,
            "identity file verification: received bytes differ"
        );
        assert_eq!(
            member.lock().await.transfers[&id].relayed,
            relay_first,
            "verified transfer must use the intended service route"
        );
        let (outsider, _) = engine(&root.path().join("outsider"))?;
        let c = Node::start(outsider, false).await?;
        cleanup.nodes.push(c.clone());
        c.connect(owner_peer, owner_route).await?;
        app_connection(&c, owner_peer, expected_route).await?;
        let snapshot = owner.lock().await.active_snapshot()?;
        assert!(matches!(
            c.request(
                owner_peer,
                &Request::Catalog {
                    snapshot: snapshot.clone(),
                    offset: 0
                }
            )
            .await?,
            Response::Denied
        ));
        let mut control = c.stream_control(owner_peer);
        let mut stream =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(owner_peer, CONTACT))
                .await
                .context("outsider contact protocol negotiation timed out")??;
        write_frame(
            &mut stream,
            &ContactRequest {
                snapshot: snapshot.clone(),
                contact: None,
                offset: 0,
            },
        )
        .await?;
        assert!(
            read_frame::<_, ContactResponse>(&mut stream).await.is_err(),
            "outsider must not receive contacts"
        );
        let mut stream =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(owner_peer, FILE))
                .await
                .context("outsider file protocol negotiation timed out")??;
        write_frame(&mut stream, &Request::File { snapshot, manifest }).await?;
        assert!(
            matches!(
                read_frame::<_, Response>(&mut stream).await?,
                Response::Denied
            ),
            "outsider must not receive file data"
        );
        eprintln!("identity nonmember catalog/contact/file denied");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let closed = cleanup.close().await;
    result?;
    closed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_relay_first_requires_an_authorized_app_endpoint() -> Result<()> {
    owner_also_hosts_relay(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_app_first_keeps_the_app_usable_when_relay_connects() -> Result<()> {
    owner_also_hosts_relay(false).await
}

#[tokio::test]
async fn legacy_relay_identity_migrates_once_without_changing_member_state_or_remote_pins(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, _) = engine(root.path())?;
    owner
        .lock()
        .await
        .create_workspace("Preserved workspace".into())?;
    owner.lock().await.create_invite()?;
    let source = root.path().join("preserved.bin");
    tokio::fs::write(&source, b"signed file remains valid").await?;
    share_paths(owner.clone(), vec![source]).await?;
    let mut legacy = owner.lock().await.persisted.clone();
    let original_app = legacy.identity.clone();
    let original_workspace = legacy.workspaces.values().next().unwrap().snapshot.clone();
    let original_manifest = legacy.shares.values().next().unwrap().manifest.clone();
    let app_peer = legacy.peer_id()?;
    let remote_peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    legacy.network.help_network = true;
    legacy.network.public_addresses = vec![format!("/ip4/8.8.8.8/tcp/42042/p2p/{app_peer}")];
    legacy.network.relays = vec![format!("/ip4/1.1.1.1/tcp/42042/p2p/{remote_peer}")];
    let original_public = legacy.network.public_addresses.clone();
    let original_remote = legacy.network.relays.clone();
    // Serialize an actual legacy document, without either of the new fields.
    let mut document = serde_json::to_value(&legacy)?;
    document.as_object_mut().unwrap().remove("relay_identity");
    document
        .as_object_mut()
        .unwrap()
        .remove("relay_identity_migrated");
    let legacy: Persisted = serde_json::from_value(document)?;
    owner.lock().await.store.save(&legacy)?;
    drop(owner);
    let (reopened, _) = engine(root.path())?;
    let e = reopened.lock().await;
    assert!(
        e.persisted.identity == original_app,
        "original app identity bytes changed"
    );
    assert_eq!(e.peer(), app_peer);
    assert_eq!(
        e.persisted
            .workspaces
            .values()
            .next()
            .unwrap()
            .snapshot
            .signature,
        original_workspace.signature
    );
    e.persisted
        .workspaces
        .values()
        .next()
        .unwrap()
        .snapshot
        .verify()?;
    assert_eq!(
        e.persisted.shares.values().next().unwrap().manifest,
        original_manifest
    );
    e.persisted
        .shares
        .values()
        .next()
        .unwrap()
        .manifest
        .verify()?;
    assert!(!e.persisted.network.help_network && e.persisted.relay_identity_migrated);
    assert_eq!(e.persisted.network.public_addresses, original_public);
    assert_eq!(e.persisted.network.relays, original_remote);
    assert!(
        e.persisted
            .network
            .validate(e.relay_key.public().to_peer_id())
            .is_err(),
        "old hosting suffix must require explicit adjustment"
    );
    let relay_identity = e.persisted.relay_identity.clone();
    assert_ne!(e.relay_key.public().to_peer_id().to_string(), app_peer);
    let second_load = e.store.load()?;
    assert!(
        second_load.relay_identity == relay_identity,
        "relay identity changed after reload"
    );
    assert!(!second_load.network.help_network && second_load.relay_identity_migrated);
    assert_eq!(second_load.network.relays, original_remote);
    let protected = tokio::fs::read(root.path().join("state.bin")).await?;
    assert!(!String::from_utf8_lossy(&protected).contains(&relay_identity));
    assert!(!String::from_utf8_lossy(&protected).contains(&original_app));
    Ok(())
}
