//! Actual authenticated sockets with test-only locator-to-loopback translation.
//! These tests do not establish real Tailscale sharing, grants, or DERP behavior.
use super::*;
use crate::{
    engine::{share_paths, Engine, View},
    overlay::WorkspaceNetworkPolicy,
    tailscale::{TailscaleNode, TailscaleSnapshot, TailscaleState},
};
use libp2p::multiaddr::Protocol;
use std::net::IpAddr;

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
    async fn close(&self) {
        for node in &self.0 {
            node.shutdown.cancel();
        }
        for node in &self.0 {
            node.shutdown_and_wait().await;
        }
    }
}

fn engine(path: &Path) -> Result<(Shared, watch::Receiver<()>)> {
    let (changed, updates) = watch::channel(());
    let shared = Engine::open(
        path,
        Arc::new(move |_| {
            changed.send_replace(());
        }),
    )?;
    Ok((shared, updates))
}

fn tail_node(id: &str, ip: &str) -> Result<TailscaleNode> {
    Ok(TailscaleNode {
        id: id.into(),
        dns_name: format!("{id}.test-tail.ts.net"),
        ips: vec![ip.parse()?],
        online: true,
    })
}

async fn ready(shared: &Shared, own: TailscaleNode, peers: Vec<TailscaleNode>) {
    let mut e = shared.lock().await;
    e.persisted.network.use_tailscale = true;
    e.tailscale = TailscaleSnapshot {
        state: TailscaleState::Ready,
        self_node: Some(own),
        peers,
    };
}

async fn wait_view(
    shared: &Shared,
    updates: &mut watch::Receiver<()>,
    stage: &'static str,
    predicate: impl Fn(&View) -> bool,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return Ok(());
            }
            updates.changed().await?;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {stage}"))?
}

async fn overlay_connected(node: &Node, peer: PeerId) -> Result<()> {
    let mut peers = node.overlay_peers.clone();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if peers.borrow().contains(&peer) {
                return Ok(());
            }
            peers.changed().await?;
        }
    })
    .await;
    if result.is_err() {
        let addresses = node.addresses.borrow().clone();
        let diagnostic = node.diagnostics.borrow().get(&peer).cloned();
        let view = node.shared.lock().await.view();
        eprintln!("overlay reconnect state={:?} listener_ready={} overlay_peers={} addresses={addresses:?} diagnostic={diagnostic:?} last_error={:?}",view.tailscale.state,view.tailscale.listener_ready,peers.borrow().len(),view.last_error);
    }
    result.context("Overlay connection was not established")?
}

fn config(listen: &str, remotes: &[(&str, &str)]) -> Result<OverlayTestConfig> {
    Ok(OverlayTestConfig {
        listen: listen.parse()?,
        remotes: remotes
            .iter()
            .map(|(id, ip)| Ok(((*id).into(), ip.parse::<IpAddr>()?)))
            .collect::<Result<_>>()?,
    })
}

async fn transfer_file(
    sender: &Shared,
    receiver: &Shared,
    node: &Node,
    updates: &mut watch::Receiver<()>,
    root: &Path,
    name: &str,
    payload: &[u8],
) -> Result<()> {
    let source = root.join(name);
    tokio::fs::write(&source, payload).await?;
    share_paths(sender.clone(), vec![source]).await?;
    wait_view(receiver, updates, "overlay catalog", |view| {
        view.files
            .iter()
            .any(|f| !f.mine && f.manifest.name == name)
    })
    .await?;
    let manifest = receiver
        .lock()
        .await
        .remote
        .values()
        .find(|m| m.name == name)
        .context("Missing shared manifest")?
        .clone();
    assert_eq!(manifest.sha256, crate::model::digest(payload));
    let output = root.join(format!("received-{name}"));
    tokio::fs::create_dir(&output).await?;
    let id = node.receive(manifest.file_id, output.clone()).await?;
    wait_view(receiver, updates, "overlay file completion", |view| {
        view.transfers
            .iter()
            .any(|t| t.id == id && t.status == "Completed")
    })
    .await?;
    let actual = tokio::fs::read(output.join(name)).await?;
    assert_eq!(crate::model::digest(&actual), manifest.sha256);
    assert_eq!(actual, payload);
    let e = receiver.lock().await;
    let transfer = &e.transfers[&id];
    assert!(
        transfer.tailscale,
        "Overlay transfer must identify its actual adapter"
    );
    assert!(!transfer.relayed, "Tailscale is not a Dump relay circuit");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_invite_without_mdns_requires_approval_then_transfers_and_chats_both_ways(
) -> Result<()> {
    overlay_lifecycle(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_tcp_noise_invite_approval_file_hash_and_chat_both_directions() -> Result<()> {
    overlay_lifecycle(true).await
}

async fn overlay_lifecycle(tcp: bool) -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, mut owner_updates) = engine(&root.path().join("owner"))?;
    let (member, mut member_updates) = engine(&root.path().join("member"))?;
    owner
        .lock()
        .await
        .create_workspace("Overlay sockets".into())?;
    let owner_tail = tail_node("nowner100", "100.80.10.2")?;
    let member_tail = tail_node("nmember100", "100.80.10.3")?;
    // One-way sharing: only the recipient can resolve the shared host.
    ready(&owner, owner_tail.clone(), vec![]).await;
    ready(&member, member_tail, vec![owner_tail]).await;
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let member_peer = member.lock().await.key.public().to_peer_id();
    let (owner_ip, member_ip) = if tcp {
        ("127.100.13.2", "127.100.13.3")
    } else {
        ("127.100.10.2", "127.100.10.3")
    };
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_overlay_test(owner.clone(), config(owner_ip, &[])?).await?;
        nodes.0.push(a.clone());
        let b = Node::start_overlay_test(
            member.clone(),
            config(member_ip, &[("nowner100", owner_ip)])?,
        )
        .await?;
        nodes.0.push(b.clone());
        wait_view(&owner, &mut owner_updates, "owner overlay listener", |v| {
            v.tailscale.listener_ready
        })
        .await?;
        wait_view(
            &member,
            &mut member_updates,
            "member overlay listener",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        let invite = owner.lock().await.create_overlay_invite()?;
        if tcp {
            // Establish TCP/Noise before the invitation schedules its default
            // QUIC-first routes. This is explicit TCP coverage, not UDP-blocking.
            b.connect(
                owner_peer,
                overlay_network::listener(owner_ip.parse()?, true),
            )
            .await?;
            overlay_connected(&b, owner_peer).await?;
            overlay_connected(&a, member_peer).await?;
        }
        member.lock().await.join(&invite)?;
        wait_view(
            &owner,
            &mut owner_updates,
            "explicit approval request",
            |v| {
                v.pending
                    .iter()
                    .any(|p| p.peer_id == member_peer.to_string())
            },
        )
        .await?;
        wait_view(
            &member,
            &mut member_updates,
            "approval acknowledgement",
            |v| v.join_stage == Some("awaiting_approval"),
        )
        .await?;
        assert!(member.lock().await.persisted.workspaces.is_empty());
        assert_eq!(owner.lock().await.active_snapshot()?.members.len(), 1);
        assert_eq!(
            owner.lock().await.overlay_policy(),
            WorkspaceNetworkPolicy::TailscaleOnly
        );
        owner.lock().await.approve(&member_peer.to_string())?;
        wait_view(&member, &mut member_updates, "approved membership", |v| {
            v.active.is_some() && !v.joining
        })
        .await?;
        overlay_connected(&a, member_peer).await?;
        overlay_connected(&b, owner_peer).await?;
        assert!(owner.lock().await.tailscale.peers.is_empty());
        assert_eq!(
            member.lock().await.active_snapshot()?,
            owner.lock().await.active_snapshot()?
        );
        transfer_file(
            &owner,
            &member,
            &b,
            &mut member_updates,
            root.path(),
            "from-owner.bin",
            &vec![23; 2 * BLOCK + 17],
        )
        .await?;
        transfer_file(
            &member,
            &owner,
            &a,
            &mut owner_updates,
            root.path(),
            "from-member.bin",
            b"reverse transfer over the recipient-established connection",
        )
        .await?;
        wait_view(&owner, &mut owner_updates, "owner sees recipient", |v| {
            v.online.contains(&member_peer.to_string())
        })
        .await?;
        wait_view(&member, &mut member_updates, "recipient sees owner", |v| {
            v.online.contains(&owner_peer.to_string())
        })
        .await?;
        let workspace = owner.lock().await.active_snapshot()?.workspace_id;
        let outbound = a.send_chat(workspace, "From shared host".into()).await?;
        wait_view(&member, &mut member_updates, "host chat", |v| {
            v.chat_messages
                .iter()
                .any(|m| m.message.id == outbound && m.message.body == "From shared host")
        })
        .await?;
        let reply = b
            .send_chat(workspace, "From sharing recipient".into())
            .await?;
        wait_view(&owner, &mut owner_updates, "recipient chat", |v| {
            v.chat_messages
                .iter()
                .any(|m| m.message.id == reply && m.message.body == "From sharing recipient")
        })
        .await?;
        assert!(a.relay_addresses.borrow().is_empty());
        assert!(b.relay_addresses.borrow().is_empty());
        if !tcp {
            logout_interrupts_owned_partial_without_retry(
                &owner,
                &member,
                &b,
                &mut member_updates,
                root.path(),
                owner_peer,
            )
            .await?;
        }
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

async fn logout_interrupts_owned_partial_without_retry(
    owner: &Shared,
    member: &Shared,
    node: &Node,
    updates: &mut watch::Receiver<()>,
    root: &Path,
    owner_peer: PeerId,
) -> Result<()> {
    let name = "interrupted-by-logout.bin";
    let source = root.join(name);
    tokio::fs::write(&source, vec![42; 3 * BLOCK + 19]).await?;
    share_paths(owner.clone(), vec![source]).await?;
    wait_view(member, updates, "logout test catalog", |view| {
        view.files
            .iter()
            .any(|f| !f.mine && f.manifest.name == name)
    })
    .await?;
    let manifest = member
        .lock()
        .await
        .remote
        .values()
        .find(|m| m.name == name)
        .context("Missing logout test manifest")?
        .clone();
    let output = root.join("interrupted-output");
    tokio::fs::create_dir(&output).await?;
    let (started, progress) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    *node.limits.receive_pause.lock().await = Some(ReceivePause {
        started,
        resume: resumed,
    });
    let id = node.receive(manifest.file_id, output.clone()).await?;
    tokio::time::timeout(Duration::from_secs(10), progress)
        .await
        .context("Receive did not reach first disk write")??;
    let original_snapshot = {
        let mut e = member.lock().await;
        let transfer = &e.transfers[&id];
        ensure!(
            transfer.status == "Receiving" && transfer.bytes > 0 && transfer.bytes < transfer.total,
            "Receive was not held during active transfer"
        );
        assert!(
            e.set_network_policy(WorkspaceNetworkPolicy::Legacy)
                .is_err(),
            "Policy changed during an active receive"
        );
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        assert_eq!(e.persisted.partials.len(), 1);
        assert!(e.persisted.partials[0].exists());
        let original = e.tailscale.clone();
        e.tailscale.state = TailscaleState::NeedsLogin;
        e.tailscale.peers.clear();
        e.tailscale_generation = e.tailscale_generation.wrapping_add(1);
        e.emit();
        original
    };
    wait_view(member, updates, "logout cancels active receive", |view| {
        view.transfers
            .iter()
            .any(|t| t.id == id && t.status == "Cancelled")
    })
    .await?;
    let mut peers = node.overlay_peers.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !peers.borrow().contains(&owner_peer) {
                return Ok::<_, anyhow::Error>(());
            }
            peers.changed().await?;
        }
    })
    .await
    .context("Logout left an overlay connection open")??;
    {
        let e = member.lock().await;
        assert!(!e.overlay_listener_ready);
        assert!(e.check_transport(true).is_err());
        assert!(e.check_transport(false).is_err());
    }
    assert!(
        node.addresses
            .borrow()
            .contains(&overlay_network::listener("127.100.10.3".parse()?, false)),
        "Temporary logout discarded the owned fixed-port listener"
    );
    assert!(
        node.overlay_scope.read().unwrap().contains(
            None,
            &overlay_network::listener("127.100.10.2".parse()?, false)
        ),
        "Temporary logout lost the classification of a pending outbound overlay dial"
    );
    assert!(
        !output.join(name).exists(),
        "Incomplete destination was committed"
    );
    assert!(
        !output.join(format!(".dump-{id}.part")).exists(),
        "Owned temporary file was not cleaned up"
    );
    assert_eq!(std::fs::read_dir(&output)?.count(), 0);
    let before = {
        let mut e = member.lock().await;
        assert!(e.persisted.partials.is_empty());
        let before: BTreeSet<_> = e.transfers.keys().copied().collect();
        e.tailscale = original_snapshot;
        e.tailscale_generation = e.tailscale_generation.wrapping_add(1);
        e.emit();
        before
    };
    overlay_connected(node, owner_peer).await?;
    wait_view(member, updates, "catalog after login restored", |view| {
        view.tailscale.listener_ready && view.online.contains(&owner_peer.to_string())
    })
    .await?;
    let e = member.lock().await;
    assert_eq!(
        e.transfers.keys().copied().collect::<BTreeSet<_>>(),
        before,
        "Reconnect silently retried an interrupted transfer"
    );
    assert_eq!(e.transfers[&id].status, "Cancelled");
    assert!(!output.join(name).exists());
    drop(e);
    drop(resume);
    Ok(())
}

async fn legacy_listener(node: &Node) -> Result<Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let found = addresses
                .borrow()
                .iter()
                .find(|a| {
                    a.iter()
                        .any(|p| matches!(p, Protocol::Udp(port) if port != crate::tailscale::PORT))
                })
                .cloned();
            if let Some(address) = found {
                return Ok(address);
            }
            addresses.changed().await?;
        }
    })
    .await
    .context("Missing legacy listener")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_overlay_join_cannot_replay_old_invitation_over_legacy() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, _) = engine(&root.path().join("owner"))?;
    let (member, _) = engine(&root.path().join("member"))?;
    owner
        .lock()
        .await
        .create_workspace("Cancelled overlay".into())?;
    ready(&owner, tail_node("nowner105", "100.80.15.2")?, vec![]).await;
    ready(&member, tail_node("nmember105", "100.80.15.3")?, vec![]).await;
    // Finish and cancel the join before any networking task can poll it.
    owner.lock().await.overlay_listener_ready = true;
    member.lock().await.overlay_listener_ready = true;
    let url = owner.lock().await.create_overlay_invite()?;
    let invitation = {
        let mut e = member.lock().await;
        e.join(&url)?;
        let invitation = e
            .persisted
            .joining
            .clone()
            .context("Missing pending overlay join")?;
        e.cancel_join()?;
        assert!(e.persisted.joining.is_none());
        assert!(e.persisted.overlay_joining.is_none());
        invitation
    };
    owner
        .lock()
        .await
        .set_network_policy(WorkspaceNetworkPolicy::Legacy)?;
    assert!(
        !owner.lock().await.persisted.invites.is_empty(),
        "Owner token must remain valid to exercise stale outgoing join rejection"
    );
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_overlay_test(owner.clone(), config("127.100.15.2", &[])?).await?;
        nodes.0.push(a.clone());
        let b = Node::start_overlay_test(member.clone(), config("127.100.15.3", &[])?).await?;
        nodes.0.push(b.clone());
        b.connect(owner_peer, legacy_listener(&a).await?).await?;
        let mut diagnostics = b.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if diagnostics
                    .borrow()
                    .get(&owner_peer)
                    .is_some_and(|d| d.state == ConnectionState::Direct)
                {
                    return Ok::<_, anyhow::Error>(());
                }
                diagnostics.changed().await?;
            }
        })
        .await
        .context("Cancelled-join regression needs an established legacy connection")??;
        assert!(owner.lock().await.pending.is_empty());
        let result = b
            .request(
                owner_peer,
                &Request::Join {
                    invitation,
                    name: "Cancelled member".into(),
                },
            )
            .await;
        assert!(
            result.is_err(),
            "Cancelled overlay join was silently resent through a legacy connection"
        );
        assert!(
            owner.lock().await.pending.is_empty(),
            "Cancelled join reached the owner"
        );
        assert!(member.lock().await.persisted.workspaces.is_empty());
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_live_overlay_pair_to_legacy_recovers_known_lan_catalog_and_file() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, mut owner_updates) = engine(&root.path().join("owner"))?;
    let (member, mut member_updates) = engine(&root.path().join("member"))?;
    owner
        .lock()
        .await
        .create_workspace("Return to LAN".into())?;
    let host = tail_node("nowner104", "100.80.14.2")?;
    ready(&owner, host.clone(), vec![]).await;
    ready(&member, tail_node("nmember104", "100.80.14.3")?, vec![host]).await;
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let member_peer = member.lock().await.key.public().to_peer_id();
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_overlay_test(owner.clone(), config("127.100.14.2", &[])?).await?;
        nodes.0.push(a.clone());
        let b = Node::start_overlay_test(
            member.clone(),
            config("127.100.14.3", &[("nowner104", "127.100.14.2")])?,
        )
        .await?;
        nodes.0.push(b.clone());
        wait_view(
            &owner,
            &mut owner_updates,
            "LAN-recovery owner listener",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        wait_view(
            &member,
            &mut member_updates,
            "LAN-recovery member listener",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        let invite = owner.lock().await.create_overlay_invite()?;
        member.lock().await.join(&invite)?;
        wait_view(
            &owner,
            &mut owner_updates,
            "LAN-recovery approval request",
            |v| {
                v.pending
                    .iter()
                    .any(|p| p.peer_id == member_peer.to_string())
            },
        )
        .await?;
        owner.lock().await.approve(&member_peer.to_string())?;
        wait_view(
            &member,
            &mut member_updates,
            "LAN-recovery approved member",
            |v| v.active.is_some() && !v.joining,
        )
        .await?;
        overlay_connected(&a, member_peer).await?;
        overlay_connected(&b, owner_peer).await?;
        // Record a known route without manually dialing or disconnecting. Up to
        // this point both sockets were established exclusively by overlay join.
        b.test_mdns(owner_peer, legacy_listener(&a).await?, true)
            .await?;
        assert!(a.overlay_peers.borrow().contains(&member_peer));
        assert!(b.overlay_peers.borrow().contains(&owner_peer));
        owner
            .lock()
            .await
            .set_network_policy(WorkspaceNetworkPolicy::Legacy)?;
        member
            .lock()
            .await
            .set_network_policy(WorkspaceNetworkPolicy::Legacy)?;
        let payload = b"LAN traffic after leaving a live overlay workspace";
        let name = "back-on-lan.bin";
        let source = root.path().join(name);
        tokio::fs::write(&source, payload).await?;
        share_paths(owner.clone(), vec![source]).await?;
        wait_view(
            &member,
            &mut member_updates,
            "legacy catalog recovered without manual reconnect",
            |v| {
                v.online.contains(&owner_peer.to_string())
                    && v.files.iter().any(|f| !f.mine && f.manifest.name == name)
            },
        )
        .await?;
        let manifest = member
            .lock()
            .await
            .remote
            .values()
            .find(|m| m.name == name)
            .context("Missing recovered LAN manifest")?
            .clone();
        assert_eq!(manifest.sha256, crate::model::digest(payload));
        let output = root.path().join("legacy-output");
        tokio::fs::create_dir(&output).await?;
        let id = b.receive(manifest.file_id, output.clone()).await?;
        wait_view(
            &member,
            &mut member_updates,
            "legacy receive after overlay policy change",
            |v| {
                v.transfers
                    .iter()
                    .any(|t| t.id == id && t.status == "Completed")
            },
        )
        .await?;
        let actual = tokio::fs::read(output.join(name)).await?;
        assert_eq!(actual, payload);
        assert_eq!(crate::model::digest(&actual), manifest.sha256);
        let e = member.lock().await;
        assert!(
            !e.transfers[&id].tailscale,
            "Recovered LAN transfer retained overlay route metadata"
        );
        assert!(!e.transfers[&id].relayed);
        assert!(e.persisted.partials.is_empty());
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strict_workspace_does_not_reuse_same_peer_legacy_connection_or_leak_catalog() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let (owner, mut owner_updates) = engine(&root.path().join("owner"))?;
    let (member, mut member_updates) = engine(&root.path().join("member"))?;
    owner
        .lock()
        .await
        .create_workspace("Strict sockets".into())?;
    let member_peer = member.lock().await.key.public().to_peer_id();
    // Install a signed, already-approved roster to isolate transport policy.
    let workspace = {
        let mut e = owner.lock().await;
        let id = e.persisted.active.context("No active workspace")?;
        let mut workspace = e.persisted.workspaces[&id].clone();
        workspace.snapshot.members.push(Member {
            peer_id: member_peer.to_string(),
            name: "Member".into(),
        });
        workspace.snapshot.revision += 1;
        workspace.snapshot.sign(&e.key)?;
        e.persisted.workspaces.insert(id, workspace.clone());
        workspace
    };
    {
        let mut e = member.lock().await;
        e.persisted.active = Some(workspace.snapshot.workspace_id);
        e.persisted
            .workspaces
            .insert(workspace.snapshot.workspace_id, workspace.clone());
    }
    ready(&owner, tail_node("nowner101", "100.80.11.2")?, vec![]).await;
    ready(&member, tail_node("nmember101", "100.80.11.3")?, vec![]).await;
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_overlay_test(owner.clone(), config("127.100.11.2", &[])?).await?;
        nodes.0.push(a.clone());
        let b = Node::start_overlay_test(member.clone(), config("127.100.11.3", &[])?).await?;
        nodes.0.push(b.clone());
        wait_view(&owner, &mut owner_updates, "strict owner listener", |v| {
            v.tailscale.listener_ready
        })
        .await?;
        wait_view(
            &member,
            &mut member_updates,
            "strict member listener",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        let legacy = legacy_listener(&a).await?;
        b.connect(owner_peer, legacy.clone()).await?;
        let mut diagnostics = b.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if diagnostics
                    .borrow()
                    .get(&owner_peer)
                    .is_some_and(|d| d.state == ConnectionState::Direct)
                {
                    return Ok::<_, anyhow::Error>(());
                }
                diagnostics.changed().await?;
            }
        })
        .await
        .context("Legacy connection was not actually established")??;
        owner
            .lock()
            .await
            .set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        member
            .lock()
            .await
            .set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        owner.lock().await.online.clear();
        member.lock().await.remote.clear();
        let secret = root.path().join("strict-private.bin");
        tokio::fs::write(&secret, b"catalog must remain private on legacy").await?;
        share_paths(owner.clone(), vec![secret]).await?;
        assert!(
            b.open_stream(owner_peer, CONTROL).await.is_err(),
            "Strict routing reused a legacy connection to the same Peer ID"
        );
        assert!(!b.overlay_peers.borrow().contains(&owner_peer));
        {
            let mut e = member.lock().await;
            e.persisted.joining = Some(Invitation {
                version: VERSION,
                workspace_id: Uuid::new_v4(),
                owner_peer_id: owner_peer.to_string(),
                token: crate::model::random_secret(),
                contact: None,
            });
        }
        assert!(
            b.open_stream(owner_peer, CONTROL).await.is_err(),
            "A pending legacy join changed the active strict workspace's stream route"
        );
        member.lock().await.persisted.joining = None;
        // Attempt legacy explicitly after the policy change as an untrusted sender.
        let _ = b.connect(owner_peer, legacy).await;
        let attempted = tokio::time::timeout(PREAUTH_TIMEOUT, async {
            let mut stream = b.control.clone().open_stream(owner_peer, CONTROL).await?;
            write_frame(
                &mut stream,
                &Request::Catalog {
                    snapshot: workspace.snapshot.clone(),
                    offset: 0,
                },
            )
            .await?;
            read_frame::<_, Response>(&mut stream).await
        })
        .await;
        if let Ok(Ok(response)) = attempted {
            assert!(
                matches!(response, Response::Denied),
                "Strict owner leaked a catalog over legacy transport"
            );
        }
        assert!(owner.lock().await.pending.is_empty());
        assert!(!owner
            .lock()
            .await
            .online
            .contains_key(&member_peer.to_string()));
        assert_eq!(owner.lock().await.active_snapshot()?.members.len(), 2);
        assert!(member.lock().await.remote.is_empty());
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_locator_wrong_authenticated_peer_is_not_admitted() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, _) = engine(&root.path().join("owner"))?;
    let (member, mut member_updates) = engine(&root.path().join("member"))?;
    let (impostor, mut impostor_updates) = engine(&root.path().join("impostor"))?;
    owner
        .lock()
        .await
        .create_workspace("Pinned identity".into())?;
    let host = tail_node("nowner102", "100.80.12.2")?;
    ready(&owner, host.clone(), vec![]).await;
    owner.lock().await.overlay_listener_ready = true;
    let invite = owner.lock().await.create_overlay_invite()?;
    ready(&member, tail_node("nmember102", "100.80.12.3")?, vec![host]).await;
    ready(&impostor, tail_node("nimpostor102", "100.80.12.4")?, vec![]).await;
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let impostor_peer = impostor.lock().await.key.public().to_peer_id();
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        // Resolve the signed owner locator to a socket controlled by a different Dump key.
        let bad = Node::start_overlay_test(impostor.clone(), config("127.100.12.4", &[])?).await?;
        nodes.0.push(bad);
        wait_view(
            &impostor,
            &mut impostor_updates,
            "impostor socket listening",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        let b = Node::start_overlay_test(
            member.clone(),
            config("127.100.12.3", &[("nowner102", "127.100.12.4")])?,
        )
        .await?;
        nodes.0.push(b.clone());
        wait_view(
            &member,
            &mut member_updates,
            "member socket listening",
            |v| v.tailscale.listener_ready,
        )
        .await?;
        member.lock().await.join(&invite)?;
        let mut diagnostics = b.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if diagnostics
                    .borrow()
                    .get(&owner_peer)
                    .is_some_and(|d| d.last_failure == Some(DialFailure::WrongPeer))
                {
                    return Ok::<_, anyhow::Error>(());
                }
                diagnostics.changed().await?;
            }
        })
        .await
        .context("Wrong-PeerId handshake was not exercised")??;
        assert!(!b.overlay_peers.borrow().contains(&owner_peer));
        assert!(!b.overlay_peers.borrow().contains(&impostor_peer));
        assert!(b.open_stream(owner_peer, CONTROL).await.is_err());
        assert!(member.lock().await.persisted.workspaces.is_empty());
        assert!(impostor.lock().await.pending.is_empty());
        assert!(member.lock().await.remote.is_empty());
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_overlay_udp_port_recovers_after_conflict_without_cli_refresh() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (shared, mut updates) = engine(root.path())?;
    shared
        .lock()
        .await
        .create_workspace("Port conflict recovery".into())?;
    ready(&shared, tail_node("nowner106", "100.80.16.2")?, vec![]).await;
    let actual_ip: IpAddr = "127.100.16.2".parse()?;
    let occupied = std::net::UdpSocket::bind((actual_ip, crate::tailscale::PORT))?;
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let node = Node::start_overlay_test(shared.clone(), config("127.100.16.2", &[])?).await?;
        nodes.0.push(node.clone());
        wait_view(&shared, &mut updates, "real UDP port conflict", |view| {
            !view.tailscale.listener_ready
                && view.last_error.as_deref().is_some_and(|error| {
                    error.contains("Tailscale listener") && error.contains("42043")
                })
        })
        .await?;
        let generation = shared.lock().await.tailscale_generation;
        // The port becomes available without a CLI refresh, new snapshot,
        // settings write, or user pressing Check Tailscale.
        drop(occupied);
        let tcp = overlay_network::listener(actual_ip, true);
        let quic = overlay_network::listener(actual_ip, false);
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let ready = shared.lock().await.view().tailscale.listener_ready;
                let addresses = node.addresses.borrow().clone();
                if ready && addresses.contains(&tcp) && addresses.contains(&quic) {
                    return Ok::<_, anyhow::Error>(());
                }
                updates.changed().await?;
            }
        })
        .await
        .context("Freed overlay port did not recover without CLI refresh")??;
        let mut e = shared.lock().await;
        assert_eq!(
            e.tailscale_generation, generation,
            "Recovery required a hidden Tailscale refresh"
        );
        assert_eq!(e.tailscale.state, TailscaleState::Ready);
        assert!(e.persisted.network.use_tailscale);
        let url = e.create_overlay_invite()?;
        let crate::overlay::OverlayLink::Invite { contact, .. } =
            crate::overlay::OverlayLink::parse(&url)?
        else {
            bail!("Port recovery did not produce an overlay invitation");
        };
        contact.verify()?;
        assert_eq!(contact.peer_id, e.peer());
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}

/// Local installed-client/listener smoke only. No remote device or transfer is
/// involved, and no installed Dump profile, tailnet policy, or firewall is edited.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires installed logged-in Tailscale and free port42043"]
async fn installed_tailscale_opens_real_adapter_listeners_and_issues_valid_invite() -> Result<()> {
    let installed = crate::tailscale::inspect().await;
    ensure!(
        installed.state == TailscaleState::Ready,
        "Prerequisite failed: installed Tailscale must be running and logged in"
    );
    let installed_self = installed.ready_node()?;
    let expected_node = installed_self.id.clone();
    let installed_ips = installed_self.ips.clone();
    ensure!(
        !installed_ips.is_empty(),
        "Prerequisite failed: installed Tailscale has no local adapter IP"
    );
    let root = tempfile::tempdir()?;
    let (shared, mut updates) = engine(root.path())?;
    let original_peer = {
        let mut e = shared.lock().await;
        e.create_workspace("Installed client smoke".into())?;
        let peer = e.peer();
        let mut persisted = e.persisted.clone();
        persisted.network.use_tailscale = true;
        e.persist(persisted)?;
        peer
    };
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        // Exercise the production startup and installed CLI, without locator
        // translation or injected Tailscale snapshots used by loopback tests.
        let node = Node::start(shared.clone(), true).await?;
        nodes.0.push(node.clone());
        wait_view(
            &shared,
            &mut updates,
            "installed Tailscale listeners",
            |v| v.tailscale.state == TailscaleState::Ready && v.tailscale.listener_ready,
        )
        .await?;
        let addresses = node.addresses.borrow().clone();
        let has_route = |ip: IpAddr, tcp: bool| {
            addresses.iter().any(|address| {
                let correct_ip = address.iter().any(|part| match (part, ip) {
                    (Protocol::Ip4(actual), IpAddr::V4(expected)) => actual == expected,
                    (Protocol::Ip6(actual), IpAddr::V6(expected)) => actual == expected,
                    _ => false,
                });
                let correct_port = address.iter().any(|part| {
                    if tcp {
                        matches!(part, Protocol::Tcp(42043))
                    } else {
                        matches!(part, Protocol::Udp(42043))
                    }
                });
                correct_ip
                    && correct_port
                    && (tcp || address.iter().any(|part| part == Protocol::QuicV1))
            })
        };
        ensure!(
            installed_ips
                .iter()
                .any(|ip| has_route(*ip, true) && has_route(*ip, false)),
            "Installed Tailscale IP has no paired TCP/QUIC listeners on port 42043"
        );
        let mut e = shared.lock().await;
        ensure!(
            e.tailscale.ready_node()?.id == expected_node,
            "Installed Tailscale node changed during smoke test"
        );
        let url = e.create_overlay_invite()?;
        let crate::overlay::OverlayLink::Invite {
            invitation,
            contact,
        } = crate::overlay::OverlayLink::parse(&url)?
        else {
            bail!("Expected an overlay invitation");
        };
        contact.verify()?;
        assert_eq!(contact.node_id, expected_node);
        assert_eq!(contact.peer_id, original_peer);
        assert_eq!(invitation.owner_peer_id, original_peer);
        assert_eq!(invitation.workspace_id, e.active_snapshot()?.workspace_id);
        assert_eq!(e.peer(), original_peer);
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}
