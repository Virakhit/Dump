use crate::{engine::Engine, lan_invite, model::*, network, storage::Store};
use anyhow::Result;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::Mutex;

// Construct a real engine without opening persisted storage. These checks cover
// memory-only LAN state on every platform; Windows tests cover DPAPI and approval.
pub(crate) fn engine(directory: &Path) -> Result<Engine> {
    let mut persisted = Persisted::new()?;
    let key = persisted.key()?;
    let relay_key = persisted.relay_key()?;
    let peer = persisted.peer_id()?;
    let workspace_id = uuid::Uuid::new_v4();
    let mut snapshot = Snapshot {
        version: VERSION,
        workspace_id,
        name: "LAN workspace".into(),
        owner_peer_id: peer.clone(),
        owner_public_key: public_key(&key),
        revision: 1,
        members: vec![Member {
            peer_id: peer,
            name: "Owner".into(),
        }],
        signature: String::new(),
    };
    snapshot.sign(&key)?;
    persisted.workspaces.insert(
        workspace_id,
        Workspace {
            snapshot,
            secret: random_secret(),
        },
    );
    persisted.active = Some(workspace_id);
    Ok(Engine {
        persisted,
        key,
        relay_key,
        store: Store::new(directory)?,
        pending: BTreeMap::new(),
        remote: BTreeMap::new(),
        online: BTreeMap::new(),
        request_budget: BTreeMap::new(),
        transfers: BTreeMap::new(),
        cancellations: BTreeMap::new(),
        preparing: Vec::new(),
        network_status: String::new(),
        relay_peers: Vec::new(),
        invite_url: None,
        last_error: None,
        notify: Arc::new(|_| {}),
        lan_invites: BTreeMap::new(),
        lan_joining: None,
    })
}

#[test]
fn lan_invites_are_temporary_and_never_persisted() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = engine(root.path())?;
    let before = now();
    let url = owner.create_lan_invite()?;
    let code = lan_invite::parse_url(&url)?;
    assert_eq!(url.len(), 27);
    let invite = &owner.lan_invites[&code];
    assert!(invite.record.expires_at >= before + 600);
    assert!(invite.record.expires_at <= now() + 600);
    assert!(invite.invitation.contact.is_none());
    assert!(owner.persisted.invites.is_empty());
    assert!(!root.path().join("state.bin").exists());
    assert!(owner.join(&url).is_err());

    owner.pending.insert(
        "requester".into(),
        crate::engine::Pending {
            peer_id: "requester".into(),
            name: "Member".into(),
            workspace_id: owner.lan_invites[&code].record.workspace_id,
            token_hash: owner.lan_invites[&code].record.token_hash.clone(),
            fingerprint: String::new(),
            requested_at: now(),
        },
    );

    owner.lan_invites.get_mut(&code).unwrap().record.expires_at = 0;
    owner.expire_lan_invites();
    assert!(owner.lan_invites.is_empty());
    assert!(owner.pending.is_empty());
    let _ = owner.create_lan_invite()?;
    owner.reset_runtime();
    assert!(owner.lan_invites.is_empty());
    assert!(owner.invite_url.is_none());
    Ok(())
}

#[test]
fn lan_join_can_be_cancelled_and_expires_without_saved_state() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = engine(&root.path().join("owner"))?;
    let mut member = engine(&root.path().join("member"))?;
    let url = owner.create_lan_invite()?;
    member.join(&url)?;
    assert!(member.view().joining);
    assert!(member.persisted.joining.is_none());
    assert!(!root.path().join("member/state.bin").exists());
    member.cancel_join()?;
    assert!(!member.view().joining);

    member.join(&url)?;
    member.lan_joining.as_mut().unwrap().expires_at = 0;
    member.expire_lan_invites();
    assert!(!member.view().joining);
    assert!(member.last_error.as_deref().unwrap().contains("expired"));
    member.join(&url)?;
    assert!(member.last_error.is_none());
    member.reset_runtime();
    assert!(!member.view().joining);
    Ok(())
}

#[test]
fn lan_invites_are_bounded_and_require_the_workspace_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = engine(root.path())?;
    for _ in 0..128 {
        owner.create_lan_invite()?;
    }
    assert!(owner.create_lan_invite().is_err());
    for invite in owner.lan_invites.values_mut() {
        invite.record.expires_at = 0;
    }
    owner.create_lan_invite()?;
    assert_eq!(owner.lan_invites.len(), 1);

    owner.persisted.active = None;
    assert!(owner.create_lan_invite().is_err());
    let id = *owner.persisted.workspaces.keys().next().unwrap();
    owner.persisted.active = Some(id);
    owner
        .persisted
        .workspaces
        .get_mut(&id)
        .unwrap()
        .snapshot
        .owner_peer_id = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id()
        .to_string();
    assert!(owner.create_lan_invite().is_err());
    Ok(())
}

#[test]
fn failed_approval_save_does_not_consume_a_lan_invitation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("owner");
    let mut owner = engine(&directory)?;
    let url = owner.create_lan_invite()?;
    let code = lan_invite::parse_url(&url)?;
    let key = libp2p::identity::Keypair::generate_ed25519();
    let peer = key.public().to_peer_id().to_string();
    let record = owner.lan_invites[&code].record.clone();
    owner.pending.insert(
        peer.clone(),
        crate::engine::Pending {
            peer_id: peer.clone(),
            name: "Member".into(),
            workspace_id: record.workspace_id,
            token_hash: record.token_hash,
            fingerprint: digest(&key.public().encode_protobuf()),
            requested_at: now(),
        },
    );
    // Remove this test's empty directory so saving fails even on Windows.
    std::fs::remove_dir(&directory)?;
    assert!(owner.approve(&peer).is_err());
    assert!(owner.lan_invites[&code].record.approved_peer.is_none());
    assert!(owner.pending.contains_key(&peer));
    assert!(!owner.active_snapshot()?.contains(&peer));
    Ok(())
}

#[tokio::test]
async fn lan_token_cannot_be_replayed_on_the_regular_protocol() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut owner = engine(root.path())?;
    let url = owner.create_lan_invite()?;
    let code = lan_invite::parse_url(&url)?;
    let invitation = owner.lan_invites[&code].invitation.clone();
    let shared = Arc::new(Mutex::new(owner));
    let peer = libp2p::identity::Keypair::generate_ed25519()
        .public()
        .to_peer_id();
    assert!(network::handle_control(
        shared.clone(),
        peer,
        network::Request::Join {
            invitation,
            name: "Member".into(),
        }
    )
    .await
    .is_err());
    assert!(shared.lock().await.pending.is_empty());
    Ok(())
}
