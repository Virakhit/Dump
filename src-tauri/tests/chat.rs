#![cfg(windows)]
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine as _};

#[tokio::test]
async fn clock_correction_does_not_make_signed_local_history_unreadable() -> Result<()> {
    let root = tempfile::tempdir()?;
    let shared = Engine::open(root.path(), Arc::new(|_| {}))?;
    let mut e = shared.lock().await;
    e.create_workspace("Office".into())?;
    let room = e.active_snapshot()?.workspace_id;
    e.compose_chat(room, "Saved before clock correction".into(), 0, 0)?;
    let mut next = e.persisted.clone();
    let message = &mut next.chat.messages[0].message;
    // Simulate valid saved history after clock rollback without touching OS time.
    message.sent_at = dump_core::model::now() + 600;
    let mut payload = Vec::new();
    ciborium::into_writer(
        &(
            "dump/chat/2",
            message.version,
            message.id,
            message.workspace_id,
            &message.sender_peer_id,
            &message.sender_public_key,
            message.roster_revision,
            message.sequence,
            message.sent_at,
            &message.body,
        ),
        &mut payload,
    )?;
    message.signature = B64.encode(e.key.sign(&payload)?);
    e.persist(next)?;
    drop(e);
    let reopened = Engine::open(root.path(), Arc::new(|_| {}))?;
    let e = reopened.lock().await;
    let saved = &e.persisted.chat.messages[0].message;
    saved.verify()?;
    assert!(
        saved.verify_incoming().is_err(),
        "fresh future network messages must still be denied"
    );
    assert_eq!(saved.body, "Saved before clock correction");
    Ok(())
}
use dump_core::{
    chat::{ChatMessage, ChatState, DeliveryStatus},
    engine::{Engine, Shared},
    model::{Invitation, Persisted},
    network::{handle_control, Node, Request, Response},
};
use libp2p::identity::Keypair;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

async fn approved_pair(root: &std::path::Path) -> Result<(Shared, Shared)> {
    let owner = Engine::open(&root.join("owner"), Arc::new(|_| {}))?;
    let member = Engine::open(&root.join("member"), Arc::new(|_| {}))?;
    owner.lock().await.create_workspace("Office".into())?;
    let invite = Invitation::parse(&owner.lock().await.create_invite()?)?;
    let peer = member.lock().await.key.public().to_peer_id();
    let join = Request::Join {
        invitation: invite,
        name: "Member".into(),
    };
    assert!(matches!(
        handle_control(owner.clone(), peer, join.clone()).await?,
        Response::Waiting
    ));
    owner.lock().await.approve(&peer.to_string())?;
    let Response::Approved { workspace } = handle_control(owner.clone(), peer, join).await? else {
        anyhow::bail!("no signed grant")
    };
    workspace.snapshot.verify()?;
    let mut e = member.lock().await;
    let mut next = e.persisted.clone();
    next.active = Some(workspace.snapshot.workspace_id);
    next.workspaces
        .insert(workspace.snapshot.workspace_id, workspace);
    e.persist(next)?;
    drop(e);
    Ok((owner, member))
}

#[test]
fn chat_signatures_and_utf8_bounds_reject_modified_or_empty_messages() -> Result<()> {
    let key = Keypair::generate_ed25519();
    let message = ChatMessage::new(&key, Uuid::new_v4(), 1, 1, "Hello".into())?;
    message.verify()?;
    let mut forged = message.clone();
    forged.body.push('!');
    assert!(forged.verify().is_err());
    forged = message.clone();
    forged.sender_peer_id = Keypair::generate_ed25519()
        .public()
        .to_peer_id()
        .to_string();
    assert!(forged.verify().is_err());
    assert!(ChatMessage::new(&key, message.workspace_id, 1, 2, "a".repeat(4096)).is_ok());
    assert!(ChatMessage::new(&key, message.workspace_id, 1, 2, "😀".repeat(1025)).is_err());
    assert!(ChatMessage::new(&key, message.workspace_id, 1, 2, " \n".into()).is_err());
    Ok(())
}

#[tokio::test]
async fn offline_recipients_are_not_queued_or_counted_as_acknowledged() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, member) = approved_pair(root.path()).await?;
    let node = Node::start(owner.clone(), false).await?;
    struct Stop(tokio_util::sync::CancellationToken);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
    let _stop = Stop(node.shutdown.clone());
    let room = owner.lock().await.active_snapshot()?.workspace_id;
    let id = node.send_chat(room, "Local only".into()).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let e = owner.lock().await;
            if e.persisted.chat.messages.iter().any(|m| {
                m.message.id == id
                    && m.delivery
                        .as_ref()
                        .is_some_and(|d| d.status == DeliveryStatus::Complete)
            }) {
                break;
            }
            drop(e);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let e = owner.lock().await;
    let delivery = e.persisted.chat.messages[0].delivery.as_ref().unwrap();
    assert_eq!(
        (delivery.attempted, delivery.acknowledged, delivery.offline),
        (0, 0, 1)
    );
    assert!(member.lock().await.store.load()?.chat.messages.is_empty());
    Ok(())
}

#[test]
fn durable_replay_window_survives_history_pruning_and_rejects_conflicting_sequences() -> Result<()>
{
    let key = Keypair::generate_ed25519();
    let room = Uuid::new_v4();
    let mut state = ChatState::default();
    let first = ChatMessage::new(&key, room, 1, 1, "First".into())?;
    assert!(state.record_received(first.clone(), "Sender".into())?);
    assert!(!state.record_received(first.clone(), "Sender".into())?);
    let conflicting = ChatMessage::new(&key, room, 1, 1, "Changed".into())?;
    assert!(state.record_received(conflicting, "Sender".into()).is_err());
    for sequence in 2..=501 {
        state.record_received(
            ChatMessage::new(&key, room, 1, sequence, "Later".into())?,
            "Sender".into(),
        )?;
    }
    assert_eq!(state.messages.len(), 500);
    let mut reloaded: ChatState = serde_json::from_slice(&serde_json::to_vec(&state)?)?;
    reloaded.validate()?;
    assert!(reloaded.record_received(first, "Sender".into()).is_err());
    let newest = reloaded.messages.last().unwrap().message.clone();
    assert!(!reloaded.record_received(newest, "Sender".into())?);
    assert_eq!(reloaded.messages.len(), 500);
    Ok(())
}

#[tokio::test]
async fn chat_requires_current_room_authenticated_sender_and_live_membership() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, member) = approved_pair(root.path()).await?;
    let snapshot = owner.lock().await.active_snapshot()?;
    let key = member.lock().await.key.clone();
    let peer = key.public().to_peer_id().to_string();
    let message = ChatMessage::new(
        &key,
        snapshot.workspace_id,
        snapshot.revision,
        1,
        "Hi".into(),
    )?;
    assert!(owner
        .lock()
        .await
        .receive_chat("wrong-peer", &snapshot, message.clone())
        .is_err());
    let outsider = Keypair::generate_ed25519();
    let outsider_message = ChatMessage::new(
        &outsider,
        snapshot.workspace_id,
        snapshot.revision,
        1,
        "Outsider".into(),
    )?;
    assert!(owner
        .lock()
        .await
        .receive_chat(
            &outsider.public().to_peer_id().to_string(),
            &snapshot,
            outsider_message
        )
        .is_err());
    owner
        .lock()
        .await
        .receive_chat(&peer, &snapshot, message.clone())?;
    owner
        .lock()
        .await
        .receive_chat(&peer, &snapshot, message.clone())?;
    assert_eq!(owner.lock().await.view().chat_messages.len(), 1);
    owner.lock().await.create_workspace("Private".into())?;
    assert!(owner
        .lock()
        .await
        .receive_chat(&peer, &snapshot, message.clone())
        .is_err());
    owner.lock().await.activate(snapshot.workspace_id)?;
    owner.lock().await.remove_member(&peer)?;
    assert!(owner
        .lock()
        .await
        .receive_chat(&peer, &snapshot, message)
        .is_err());
    assert_eq!(owner.lock().await.view().chat_messages.len(), 1);
    Ok(())
}

#[tokio::test]
async fn failed_persistence_does_not_accept_chat_or_consume_sender_sequence() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, member) = approved_pair(root.path()).await?;
    let snapshot = owner.lock().await.active_snapshot()?;
    let key = member.lock().await.key.clone();
    let message = ChatMessage::new(
        &key,
        snapshot.workspace_id,
        snapshot.revision,
        1,
        "Hi".into(),
    )?;
    std::fs::create_dir(root.path().join("owner/state.new"))?;
    let mut e = owner.lock().await;
    assert!(e
        .receive_chat(
            &key.public().to_peer_id().to_string(),
            &snapshot,
            message.clone()
        )
        .is_err());
    assert!(e.persisted.chat.messages.is_empty());
    assert!(e.persisted.chat.replay.is_empty());
    assert!(e
        .compose_chat(snapshot.workspace_id, "Outgoing".into(), 0, 1)
        .is_err());
    assert!(e.persisted.chat.sequences.is_empty());
    std::fs::remove_dir(root.path().join("owner/state.new"))?;
    e.receive_chat(&key.public().to_peer_id().to_string(), &snapshot, message)?;
    assert_eq!(e.store.load()?.chat.messages.len(), 1);
    Ok(())
}

#[tokio::test]
async fn local_history_is_scoped_and_inflight_send_becomes_interrupted_after_restart() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let e = Engine::open(root.path(), Arc::new(|_| {}))?;
    e.lock().await.create_workspace("Office".into())?;
    let room = e.lock().await.active_snapshot()?.workspace_id;
    e.lock()
        .await
        .compose_chat(room, "Saved before network".into(), 0, 0)?;
    let original_peer = e.lock().await.peer();
    let restarted = Engine::open(root.path(), Arc::new(|_| {}))?;
    assert_eq!(restarted.lock().await.peer(), original_peer);
    assert_eq!(
        restarted.lock().await.view().chat_messages[0]
            .delivery
            .as_ref()
            .unwrap()
            .status,
        DeliveryStatus::Interrupted
    );
    restarted.lock().await.create_workspace("Other".into())?;
    assert!(restarted.lock().await.view().chat_messages.is_empty());
    let original = e.lock().await.persisted.clone();
    let mut legacy = serde_json::to_value(&original)?;
    legacy.as_object_mut().unwrap().remove("chat");
    let restored: Persisted = serde_json::from_value(legacy)?;
    assert!(restored.chat.messages.is_empty());
    assert_eq!(restored.peer_id()?, original.peer_id()?);
    assert_eq!(restored.active, original.active);
    assert_eq!(restored.workspaces.len(), original.workspaces.len());
    assert_eq!(restored.invites.len(), original.invites.len());
    for (id, workspace) in &original.workspaces {
        assert_eq!(restored.workspaces[id].snapshot, workspace.snapshot);
        assert_eq!(restored.workspaces[id].secret, workspace.secret);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_quic_chat_receipt_means_receiver_persisted_message() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, member) = approved_pair(root.path()).await?;
    let a = Node::start(owner.clone(), false).await?;
    let b = Node::start(member.clone(), false).await?;
    struct Stop(Vec<tokio_util::sync::CancellationToken>);
    impl Drop for Stop {
        fn drop(&mut self) {
            for token in &self.0 {
                token.cancel();
            }
        }
    }
    let _stop = Stop(vec![a.shutdown.clone(), b.shutdown.clone()]);
    let mut addresses = b.addresses.clone();
    let route = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(route) = addresses.borrow().first().cloned() {
                return Ok::<_, anyhow::Error>(route);
            }
            addresses.changed().await?;
        }
    })
    .await??;
    let peer = member.lock().await.key.public().to_peer_id();
    a.connect(peer, route).await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        while a.refresh_peer(peer).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    let room = owner.lock().await.active_snapshot()?.workspace_id;
    let id = a
        .send_chat(room, "Actual authenticated stream".into())
        .await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let e = owner.lock().await;
            let delivery = e
                .persisted
                .chat
                .messages
                .iter()
                .find(|m| m.message.id == id)
                .and_then(|m| m.delivery.as_ref());
            if delivery.is_some_and(|d| d.status == DeliveryStatus::Complete) {
                break;
            }
            drop(e);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?;
    let e = owner.lock().await;
    let delivery = e
        .persisted
        .chat
        .messages
        .iter()
        .find(|m| m.message.id == id)
        .unwrap()
        .delivery
        .as_ref()
        .unwrap();
    ensure!(
        delivery.attempted == 1 && delivery.acknowledged == 1,
        "receiver did not confirm durable storage"
    );
    assert_eq!(
        member.lock().await.store.load()?.chat.messages[0]
            .message
            .id,
        id
    );
    Ok(())
}
