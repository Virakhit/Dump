use super::*;
#[cfg(windows)]
use crate::engine::Engine;
use crate::lan_invite;
use libp2p::multiaddr::Protocol;

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

async fn listener(node: &Node, quic: bool) -> Result<Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(address) = addresses
                .borrow()
                .iter()
                .find(|address| address.iter().any(|part| part == Protocol::QuicV1) == quic)
                .cloned()
            {
                return Ok(address);
            }
            addresses.changed().await?;
        }
    })
    .await?
}

async fn connected(node: &Node, peer: PeerId) -> Result<()> {
    let mut diagnostics = node.diagnostics.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if diagnostics
                .borrow()
                .get(&peer)
                .is_some_and(|state| state.state == ConnectionState::Direct)
            {
                return Ok(());
            }
            diagnostics.changed().await?;
        }
    })
    .await?
}

async fn request(node: &Node, owner: PeerId, code: &str) -> Result<LanInviteResponse> {
    let mut control = node.lan_control.clone();
    tokio::time::timeout(PREAUTH_TIMEOUT, async {
        let mut stream = control.open_stream(owner, LAN_INVITE).await?;
        write_frame(
            &mut stream,
            &LanInviteRequest {
                code: code.into(),
                name: Some("Member".into()),
            },
        )
        .await?;
        read_frame(&mut stream).await
    })
    .await?
}

#[tokio::test]
async fn anonymous_lan_probe_returns_commitment_without_a_pending_join() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = crate::lan_invite_tests::engine(root.path())?;
    let code = lan_invite::parse_url(&owner.create_lan_invite()?)?;
    let owner_peer = owner.key.public().to_peer_id();
    let member_peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    let shared = Arc::new(Mutex::new(owner));
    let probe: LanInviteRequest = serde_json::from_value(serde_json::json!({ "code": code }))?;
    let LanInviteResponse::Found {
        invitation,
        response: Response::Waiting,
    } = handle_lan_invite(shared.clone(), member_peer, probe).await?
    else {
        bail!("Anonymous probe did not return the invitation commitment");
    };
    lan_invite::verify(&invitation, &code, owner_peer)?;
    assert!(shared.lock().await.pending.is_empty());
    assert!(matches!(
        handle_lan_invite(
            shared.clone(),
            member_peer,
            LanInviteRequest {
                code: code.clone(),
                name: Some("Private device name".into()),
            },
        )
        .await?,
        LanInviteResponse::Found {
            response: Response::Waiting,
            ..
        }
    ));
    assert_eq!(
        shared.lock().await.pending[&member_peer.to_string()].name,
        "Private device name"
    );
    shared
        .lock()
        .await
        .lan_invites
        .get_mut(&code)
        .unwrap()
        .record
        .approved_peer = Some(owner_peer.to_string());
    assert!(
        handle_lan_invite(shared, member_peer, LanInviteRequest { code, name: None },)
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn lan_lookup_sends_a_device_name_only_after_authenticating_that_exact_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = crate::lan_invite_tests::engine(&root.path().join("owner"))?;
    let mut member = crate::lan_invite_tests::engine(&root.path().join("member"))?;
    member.persisted.device_name = "Private device name".into();
    let url = owner.create_lan_invite()?;
    let code = lan_invite::parse_url(&url)?;
    let invitation = owner.lan_invites[&code].invitation.clone();
    let owner_peer = owner.key.public().to_peer_id();
    let unrelated_peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    member.join(&url)?;
    let deadline = member.lan_joining.as_ref().unwrap().expires_at;

    for target in [unrelated_peer, owner_peer] {
        let probe = prepare_lan_request(&member, target, &code, deadline)?;
        assert!(serde_json::to_value(probe)?.get("name").is_none());
    }
    let mut tampered = invitation.clone();
    tampered.token = random_secret();
    assert!(apply_lan_response(
        &mut member,
        owner_peer,
        &code,
        deadline,
        LanInviteResponse::Found {
            invitation: Box::new(tampered),
            response: Response::Waiting
        },
    )
    .is_err());
    assert!(apply_lan_response(
        &mut member,
        unrelated_peer,
        &code,
        deadline,
        LanInviteResponse::Found {
            invitation: Box::new(invitation.clone()),
            response: Response::Waiting
        },
    )
    .is_err());
    assert!(member.lan_joining.as_ref().unwrap().owner_peer_id.is_none());
    assert!(prepare_lan_request(&member, owner_peer, &code, deadline)?
        .name
        .is_none());

    apply_lan_response(
        &mut member,
        owner_peer,
        &code,
        deadline,
        LanInviteResponse::Found {
            invitation: Box::new(invitation.clone()),
            response: Response::Waiting,
        },
    )?;
    assert_eq!(
        prepare_lan_request(&member, owner_peer, &code, deadline)?
            .name
            .as_deref(),
        Some("Private device name")
    );
    // Another candidate task may already be queued when the owner is pinned.
    let late_probe = prepare_lan_request(&member, unrelated_peer, &code, deadline)?;
    assert!(serde_json::to_value(late_probe)?.get("name").is_none());
    assert!(prepare_lan_request(&member, owner_peer, &code, deadline + 1).is_err());
    assert!(prepare_lan_request(&member, owner_peer, "AAAAAAAAAAAAAAAA", deadline).is_err());
    member.cancel_join()?;
    assert!(prepare_lan_request(&member, owner_peer, &code, deadline).is_err());
    assert!(apply_lan_response(
        &mut member,
        owner_peer,
        &code,
        deadline,
        LanInviteResponse::Found {
            invitation: Box::new(invitation),
            response: Response::Waiting
        },
    )
    .is_err());
    assert!(member.lan_joining.is_none());
    Ok(())
}

#[test]
fn lan_routes_require_private_quic_and_live_discovery() -> Result<()> {
    let peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    for address in [
        "/ip4/192.168.1.20/udp/42/quic-v1",
        "/ip4/10.0.0.20/udp/42/quic-v1",
        "/ip4/172.16.1.20/udp/42/quic-v1",
        "/ip4/169.254.1.20/udp/42/quic-v1",
        "/ip6/fd12::20/udp/42/quic-v1",
        "/ip6/fe80::20/udp/42/quic-v1",
    ] {
        assert!(lan_address(peer, &address.parse()?, false).is_some());
    }
    for address in [
        "/ip4/8.8.8.8/udp/42/quic-v1",
        "/ip6/2001:4860:4860::8888/udp/42/quic-v1",
        "/ip4/127.0.0.1/udp/42/quic-v1",
        "/ip6/::1/udp/42/quic-v1",
        "/ip4/192.168.1.20/tcp/42",
        "/ip4/192.168.1.20/udp/0/quic-v1",
        "/ip4/0.0.0.0/udp/42/quic-v1",
        "/ip4/224.0.0.1/udp/42/quic-v1",
        "/dns4/localhost/udp/42/quic-v1",
    ] {
        assert!(lan_address(peer, &address.parse()?, false).is_none());
    }
    let circuit: Multiaddr = format!("/ip4/192.168.1.20/tcp/42/p2p/{peer}/p2p-circuit").parse()?;
    assert!(lan_address(peer, &circuit, false).is_none());

    let mut discovered = LanDiscovery::default();
    let lan: Multiaddr = "/ip4/192.168.1.20/udp/42/quic-v1".parse()?;
    discovered.remember(peer, &"/ip4/8.8.8.8/udp/42/quic-v1".parse()?, false);
    discovered.remember(peer, &"/ip4/192.168.1.20/tcp/42".parse()?, false);
    assert!(!discovered.contains(&peer));
    discovered.remember(peer, &lan, false);
    assert!(discovered.contains(&peer));
    discovered.expire(peer, &lan, false);
    assert!(!discovered.contains(&peer));
    Ok(())
}

#[test]
fn bounded_scan_queries_every_ready_peer_before_lookup_timeout() -> Result<()> {
    let mut discovered = LanDiscovery::default();
    for index in 1..=MAX_LAN_DISCOVERED {
        let peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let address = format!("/ip4/192.168.1.{index}/udp/42/quic-v1").parse()?;
        discovered.remember(peer, &address, false);
    }
    let mut connections = BTreeMap::new();
    let mut attempts = BTreeMap::new();
    let mut polled = BTreeSet::new();
    let mut queried = BTreeSet::new();
    let started = Instant::now();
    let mut next_connection = 0;
    for tick in 0..(LAN_LOOKUP_TIMEOUT.as_secs() / 2) {
        let planned = discovered.schedule(
            None,
            &connections,
            &BTreeSet::new(),
            &mut attempts,
            &mut polled,
            started + Duration::from_secs(tick * 2),
        );
        assert!(
            planned
                .iter()
                .filter(|attempt| matches!(attempt, LanAttempt::Dial(..)))
                .count()
                <= MAX_LAN_IN_FLIGHT
        );
        assert!(
            planned
                .iter()
                .filter(|attempt| matches!(attempt, LanAttempt::Poll(..)))
                .count()
                <= MAX_LAN_IN_FLIGHT
        );
        for attempt in planned {
            match attempt {
                LanAttempt::Dial(peer, _) => {
                    next_connection += 1;
                    connections.insert(ConnectionId::new_unchecked(next_connection), peer);
                }
                LanAttempt::Poll(peer) => {
                    queried.insert(peer);
                }
            }
        }
    }
    assert_eq!(queried.len(), MAX_LAN_DISCOVERED);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_protocol_requires_discovery_and_rejects_tcp_even_for_a_lan_peer() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Arc::new(Mutex::new(crate::lan_invite_tests::engine(
        &root.path().join("owner"),
    )?));
    let member = Arc::new(Mutex::new(crate::lan_invite_tests::engine(
        &root.path().join("member"),
    )?));
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let member_peer = member.lock().await.key.public().to_peer_id();
    let code = lan_invite::parse_url(&owner.lock().await.create_lan_invite()?)?;
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_inner(owner.clone(), false, true).await?;
        nodes.0.push(a.clone());
        let b = Node::start_inner(member.clone(), false, true).await?;
        nodes.0.push(b.clone());
        let owner_quic = listener(&a, true).await?;
        let member_quic = listener(&b, true).await?;
        b.connect(owner_peer, owner_quic).await?;
        connected(&b, owner_peer).await?;

        // Private QUIC alone is insufficient: the requester must also have
        // current mDNS provenance on the owner.
        assert!(request(&b, owner_peer, &code).await.is_err());
        assert!(owner.lock().await.pending.is_empty());
        a.test_mdns(member_peer, member_quic.clone(), true).await?;
        assert!(matches!(
            request(&b, owner_peer, "AAAAAAAAAAAAAAAA").await?,
            LanInviteResponse::Unavailable
        ));
        assert!(owner.lock().await.pending.is_empty());
        let LanInviteResponse::Found {
            invitation,
            response: Response::Waiting,
        } = request(&b, owner_peer, &code).await?
        else {
            bail!("LAN link did not produce a pending join");
        };
        lan_invite::verify(&invitation, &code, owner_peer)?;
        assert!(owner
            .lock()
            .await
            .pending
            .contains_key(&member_peer.to_string()));
        assert!(matches!(
            b.request(
                owner_peer,
                &Request::Join {
                    invitation: *invitation,
                    name: "Member".into(),
                },
            )
            .await?,
            Response::Denied
        ));

        // A second connection with the same authenticated Peer ID is TCP.
        // A peer-level "has a LAN connection" check would incorrectly admit it.
        let tcp = Node::start_inner(member.clone(), false, true).await?;
        nodes.0.push(tcp.clone());
        tcp.connect(owner_peer, listener(&a, false).await?).await?;
        connected(&tcp, owner_peer).await?;
        let mut control = tcp.control.clone();
        let rejected =
            tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(owner_peer, LAN_INVITE))
                .await?;
        assert!(matches!(
            rejected,
            Err(libp2p_stream::OpenStreamError::UnsupportedProtocol(_))
        ));

        a.test_mdns(member_peer, member_quic, false).await?;
        assert!(request(&b, owner_peer, &code).await.is_err());
        Ok(())
    }
    .await;
    nodes.close().await;
    result?;
    assert!(owner.lock().await.lan_invites.is_empty());
    assert!(owner.lock().await.pending.is_empty());
    assert!(owner.lock().await.invite_url.is_none());
    Ok(())
}

#[cfg(windows)]
async fn view_changed(
    shared: &Shared,
    updates: &mut watch::Receiver<()>,
    predicate: impl Fn(&crate::engine::View) -> bool,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return Ok(());
            }
            updates.changed().await?;
        }
    })
    .await?
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_invite_joins_after_approval_rejects_replay_and_discards_cancelled_approval(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner_changed, mut owner_updates) = watch::channel(());
    let (member_changed, mut member_updates) = watch::channel(());
    let owner = Engine::open(
        &root.path().join("owner"),
        Arc::new(move |_| {
            owner_changed.send_replace(());
        }),
    )?;
    let member = Engine::open(
        &root.path().join("member"),
        Arc::new(move |_| {
            member_changed.send_replace(());
        }),
    )?;
    let outsider = Engine::open(&root.path().join("outsider"), Arc::new(|_| {}))?;
    owner
        .lock()
        .await
        .create_workspace("Temporary LAN".into())?;
    let workspace_id = owner.lock().await.active_snapshot()?.workspace_id;
    let url = owner.lock().await.create_lan_invite()?;
    let code = lan_invite::parse_url(&url)?;
    member.lock().await.join(&url)?;
    let owner_peer = owner.lock().await.key.public().to_peer_id();
    let member_peer = member.lock().await.key.public().to_peer_id();
    let outsider_peer = outsider.lock().await.key.public().to_peer_id();
    let mut nodes = Nodes::default();
    let result: Result<()> = async {
        let a = Node::start_inner(owner.clone(), false, true).await?;
        nodes.0.push(a.clone());
        let b = Node::start_inner(member.clone(), false, true).await?;
        nodes.0.push(b.clone());
        let c = Node::start_inner(outsider.clone(), false, true).await?;
        nodes.0.push(c.clone());
        let owner_address = listener(&a, true).await?;
        a.test_mdns(member_peer, listener(&b, true).await?, true)
            .await?;
        b.test_mdns(owner_peer, owner_address.clone(), true).await?;
        a.test_mdns(outsider_peer, listener(&c, true).await?, true)
            .await?;
        c.test_mdns(owner_peer, owner_address.clone(), true).await?;

        view_changed(&owner, &mut owner_updates, |view| {
            view.pending
                .iter()
                .any(|pending| pending.peer_id == member_peer.to_string())
        })
        .await?;
        assert!(member.lock().await.persisted.joining.is_none());
        assert!(!member
            .lock()
            .await
            .persisted
            .workspaces
            .contains_key(&workspace_id));
        c.connect(owner_peer, owner_address).await?;
        connected(&c, owner_peer).await?;
        assert!(matches!(
            request(&c, owner_peer, &code).await?,
            LanInviteResponse::Found {
                response: Response::Waiting,
                ..
            }
        ));
        assert_eq!(owner.lock().await.pending.len(), 2);
        owner.lock().await.approve(&member_peer.to_string())?;
        assert!(owner.lock().await.pending.is_empty());
        view_changed(&member, &mut member_updates, |view| {
            view.active == Some(workspace_id) && !view.joining
        })
        .await?;
        member.lock().await.active_snapshot()?.verify()?;
        assert!(member.lock().await.lan_joining.is_none());

        assert!(matches!(
            request(&c, owner_peer, &code).await?,
            LanInviteResponse::Unavailable
        ));

        let cancelled_url = owner.lock().await.create_lan_invite()?;
        let cancelled_code = lan_invite::parse_url(&cancelled_url)?;
        outsider.lock().await.join(&cancelled_url)?;
        let deadline = outsider
            .lock()
            .await
            .lan_joining
            .as_ref()
            .unwrap()
            .expires_at;
        assert!(matches!(
            request(&c, owner_peer, &cancelled_code).await?,
            LanInviteResponse::Found {
                response: Response::Waiting,
                ..
            }
        ));
        outsider.lock().await.cancel_join()?;
        owner.lock().await.approve(&outsider_peer.to_string())?;
        assert!(c
            .poll_lan_invite(owner_peer, &cancelled_code, deadline)
            .await
            .is_err());
        let late_approval = request(&c, owner_peer, &cancelled_code).await?;
        assert!(matches!(
            &late_approval,
            LanInviteResponse::Found {
                response: Response::Approved { .. },
                ..
            }
        ));
        assert!(apply_lan_response(
            &mut *outsider.lock().await,
            owner_peer,
            &cancelled_code,
            deadline,
            late_approval,
        )
        .is_err());
        assert!(!outsider
            .lock()
            .await
            .persisted
            .workspaces
            .contains_key(&workspace_id));

        owner
            .lock()
            .await
            .lan_invites
            .get_mut(&code)
            .unwrap()
            .record
            .expires_at = 0;
        assert!(matches!(
            request(&b, owner_peer, &code).await?,
            LanInviteResponse::Unavailable
        ));
        Ok(())
    }
    .await;
    nodes.close().await;
    result
}
