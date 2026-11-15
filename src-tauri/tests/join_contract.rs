//! Baseline authorization contract, with fresh protected stores and no app profile.
//! Direct handler tests do not establish discovery, socket delivery or physical LAN.
#![cfg(windows)]

use anyhow::Result;
use dump_core::{
    engine::Engine,
    model::{digest, Invitation},
    network::{handle_control, Request, Response},
};
use libp2p::identity::Keypair;
use std::sync::Arc;

#[tokio::test]
async fn received_join_waits_for_manual_approval_and_binds_authenticated_fingerprint() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let requester = Engine::open(&root.path().join("requester"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Office".into())?;
    let invitation = Invitation::parse(&owner.lock().await.create_invite()?)?;
    let requester_key = requester.lock().await.key.clone();
    let peer = requester_key.public().to_peer_id();
    let request = Request::Join {
        invitation: invitation.clone(),
        name: "Friend".into(),
    };

    for _ in 0..2 {
        assert!(matches!(
            handle_control(owner.clone(), peer, request.clone()).await?,
            Response::Waiting
        ));
    }
    {
        let e = owner.lock().await;
        let view = e.view();
        assert_eq!(view.pending.len(), 1, "retry must not duplicate requests");
        let pending = &view.pending[0];
        assert_eq!(pending.peer_id, peer.to_string());
        assert_eq!(pending.workspace_id, invitation.workspace_id);
        assert_eq!(pending.name, "Friend");
        assert_eq!(
            pending.fingerprint,
            digest(&requester_key.public().encode_protobuf())
        );
        assert!(!e.active_snapshot()?.contains(&peer.to_string()));
        // Pending-card diagnostics omit private admission fields. The complete
        // View intentionally contains the owner's invitation URL for copying.
        let serialized = serde_json::to_value(pending)?;
        assert!(serialized.get("token_hash").is_none());
        assert!(serialized.get("requested_at").is_none());
    }
    owner.lock().await.approve(&peer.to_string())?;
    let Response::Approved { workspace } = handle_control(owner.clone(), peer, request).await?
    else {
        panic!("manual approval must return a signed workspace");
    };
    workspace.snapshot.verify()?;
    assert_eq!(workspace.snapshot.workspace_id, invitation.workspace_id);
    assert_eq!(workspace.snapshot.owner_peer_id, invitation.owner_peer_id);
    assert!(workspace.snapshot.contains(&peer.to_string()));
    let e = owner.lock().await;
    assert!(e.view().pending.is_empty());
    let saved = e.store.load()?;
    assert!(saved.workspaces[&invitation.workspace_id]
        .snapshot
        .contains(&peer.to_string()));
    Ok(())
}

#[tokio::test]
async fn wrong_selected_workspace_denies_then_recovers_and_rejection_revokes_invitation(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(root.path(), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Invited".into())?;
    let invitation = Invitation::parse(&owner.lock().await.create_invite()?)?;
    owner.lock().await.create_workspace("Other room".into())?;
    let peer = Keypair::generate_ed25519().public().to_peer_id();
    let request = Request::Join {
        invitation: invitation.clone(),
        name: "Friend".into(),
    };

    assert!(handle_control(owner.clone(), peer, request.clone())
        .await
        .is_err());
    assert!(owner.lock().await.pending.is_empty());
    owner.lock().await.activate(invitation.workspace_id)?;
    assert!(matches!(
        handle_control(owner.clone(), peer, request.clone()).await?,
        Response::Waiting
    ));
    assert_eq!(owner.lock().await.view().pending.len(), 1);
    owner.lock().await.reject(&peer.to_string())?;
    assert!(handle_control(owner.clone(), peer, request).await.is_err());
    let e = owner.lock().await;
    assert!(e.view().pending.is_empty());
    assert!(!e.active_snapshot()?.contains(&peer.to_string()));
    assert!(e.store.load()?.invites.is_empty());
    Ok(())
}

#[tokio::test]
async fn forged_owner_token_workspace_and_expired_invites_create_no_pending_request() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let owner = Engine::open(root.path(), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Office".into())?;
    let invitation = Invitation::parse(&owner.lock().await.create_invite()?)?;
    let peer = Keypair::generate_ed25519().public().to_peer_id();
    let mut wrong_owner = invitation.clone();
    wrong_owner.owner_peer_id = peer.to_string();
    let mut wrong_token = invitation.clone();
    wrong_token.token = dump_core::model::random_secret();
    let mut wrong_workspace = invitation.clone();
    wrong_workspace.workspace_id = uuid::Uuid::new_v4();
    for invalid in [wrong_owner, wrong_token, wrong_workspace] {
        assert!(handle_control(
            owner.clone(),
            peer,
            Request::Join {
                invitation: invalid,
                name: "Friend".into()
            }
        )
        .await
        .is_err());
        assert!(owner.lock().await.pending.is_empty());
    }
    owner.lock().await.persisted.invites[0].expires_at = 0;
    assert!(handle_control(
        owner.clone(),
        peer,
        Request::Join {
            invitation,
            name: "Friend".into()
        }
    )
    .await
    .is_err());
    let e = owner.lock().await;
    assert!(e.pending.is_empty());
    assert!(!e.active_snapshot()?.contains(&peer.to_string()));
    Ok(())
}

#[test]
fn waiting_response_remains_compatible_with_v1_cbor() -> Result<()> {
    // Hand-encoded CBOR text string "Waiting", the existing externally tagged enum.
    let legacy = b"\x67Waiting";
    let mut encoded = Vec::new();
    ciborium::into_writer(&Response::Waiting, &mut encoded)?;
    assert_eq!(encoded.as_slice(), legacy);
    let decoded: Response = ciborium::from_reader(legacy.as_slice())?;
    assert!(matches!(decoded, Response::Waiting));
    Ok(())
}
