#![cfg(windows)]
use anyhow::Result;
use dump_core::{
    engine::Engine,
    invite_routes::{JoinInput, RouteRecord},
    model::Invitation,
    network::{handle_control, Request, Response},
};
use libp2p::identity::Keypair;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn route_records_require_current_membership_and_cannot_cross_rooms_or_replace_equal_sequences(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(root.path(), Arc::new(|_| {}))?;
    let mut e = owner.lock().await;
    e.create_workspace("Office".into())?;
    let snapshot = e.active_snapshot()?;
    let record = RouteRecord::new(&e.key, snapshot.workspace_id, 1, Vec::new())?;
    e.cache_routes(vec![record], &snapshot)?;
    let conflicting = RouteRecord::new(
        &e.key,
        snapshot.workspace_id,
        1,
        vec![format!(
            "/ip4/100.80.1.2/udp/32123/quic-v1/p2p/{}",
            e.peer()
        )],
    )?;
    assert!(e.cache_routes(vec![conflicting], &snapshot).is_err());
    let cross_room = RouteRecord::new(&e.key, Uuid::new_v4(), 2, Vec::new())?;
    assert!(e.cache_routes(vec![cross_room], &snapshot).is_err());
    let outsider = RouteRecord::new(
        &Keypair::generate_ed25519(),
        snapshot.workspace_id,
        1,
        Vec::new(),
    )?;
    assert!(e.cache_routes(vec![outsider], &snapshot).is_err());
    Ok(())
}

#[test]
fn shared_envelope_pins_owner_and_rejects_private_public_dns_and_wrong_peer_routes() -> Result<()> {
    let key = Keypair::generate_ed25519();
    let peer = key.public().to_peer_id();
    let room = Uuid::new_v4();
    let route = format!("/ip4/100.80.1.2/udp/32123/quic-v1/p2p/{peer}");
    let record = RouteRecord::new(&key, room, 1, vec![route])?;
    record.verify_at(record.issued_at)?;
    assert!(record.verify_at(record.expires_at).is_err());
    for address in [
        "/ip4/192.168.1.2/udp/123/quic-v1",
        "/ip4/8.8.8.8/udp/123/quic-v1",
        "/dns4/example.com/udp/123/quic-v1",
        "/ip4/127.0.0.1/udp/123/quic-v1",
        "/ip4/100.80.1.2/tcp/123",
    ] {
        assert!(RouteRecord::new(&key, room, 1, vec![format!("{address}/p2p/{peer}")]).is_err());
    }
    let other = Keypair::generate_ed25519().public().to_peer_id();
    assert!(RouteRecord::new(
        &key,
        room,
        1,
        vec![format!("/ip4/100.80.1.2/udp/123/quic-v1/p2p/{other}")]
    )
    .is_err());
    let mut forged = record;
    forged.sequence += 1;
    assert!(forged.verify_at(forged.issued_at).is_err());
    Ok(())
}

#[tokio::test]
async fn identical_link_requires_separate_approval_for_each_device_and_preserves_legacy_links(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(root.path(), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Office".into())?;
    let legacy = owner.lock().await.create_invite()?;
    assert!(JoinInput::parse(&legacy)?.routes.is_none());
    let url = owner.lock().await.create_shared_invite(Vec::new())?;
    let input = JoinInput::parse(&url)?;
    assert!(input.routes.is_some());
    let a = Keypair::generate_ed25519().public().to_peer_id();
    let b = Keypair::generate_ed25519().public().to_peer_id();
    let request = |name: &str| Request::Join {
        invitation: input.invitation.clone(),
        name: name.into(),
    };
    assert!(matches!(
        handle_control(owner.clone(), a, request("A")).await?,
        Response::Waiting
    ));
    owner.lock().await.approve(&a.to_string())?;
    assert!(matches!(
        handle_control(owner.clone(), a, request("A")).await?,
        Response::Approved { .. }
    ));
    assert!(matches!(
        handle_control(owner.clone(), b, request("B")).await?,
        Response::Waiting
    ));
    assert!(!owner
        .lock()
        .await
        .active_snapshot()?
        .contains(&b.to_string()));
    owner.lock().await.approve(&b.to_string())?;
    assert!(matches!(
        handle_control(owner.clone(), b, request("B")).await?,
        Response::Approved { .. }
    ));
    let mut bytes = url.into_bytes();
    let last = bytes.len() - 1;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    assert!(JoinInput::parse(std::str::from_utf8(&bytes)?).is_err());
    assert!(Invitation::parse(&legacy).is_ok());
    Ok(())
}

#[tokio::test]
async fn reject_one_peer_does_not_revoke_other_requests_but_owner_revocation_does() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(root.path(), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Office".into())?;
    let url = owner.lock().await.create_shared_invite(Vec::new())?;
    let input = JoinInput::parse(&url)?;
    let a = Keypair::generate_ed25519().public().to_peer_id();
    let b = Keypair::generate_ed25519().public().to_peer_id();
    let request = Request::Join {
        invitation: input.invitation,
        name: "Friend".into(),
    };
    handle_control(owner.clone(), a, request.clone()).await?;
    owner.lock().await.reject(&a.to_string())?;
    assert!(handle_control(owner.clone(), a, request.clone())
        .await
        .is_err());
    assert!(matches!(
        handle_control(owner.clone(), b, request.clone()).await?,
        Response::Waiting
    ));
    owner.lock().await.revoke_current_invite()?;
    assert!(owner.lock().await.view().pending.is_empty());
    assert!(handle_control(owner.clone(), b, request).await.is_err());
    Ok(())
}
