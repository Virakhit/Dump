#![cfg(windows)]

use anyhow::{Context, Result};
use dump_core::{
    chat::{ChatDeliveryStatus, ChatMessage},
    engine::{Engine, Shared, View},
    model::{Member, Snapshot},
    network::Node,
};
use libp2p::{identity::Keypair, PeerId};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct StopNodes(Vec<CancellationToken>);

impl Drop for StopNodes {
    fn drop(&mut self) {
        for shutdown in &self.0 {
            shutdown.cancel();
        }
    }
}

async fn wait(shared: &Shared, description: &str, predicate: impl Fn(&View) -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if predicate(&shared.lock().await.view()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .with_context(|| format!("timed out waiting for {description}"))?;
    Ok(())
}

async fn address(node: &Node) -> Result<libp2p::Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(address) = addresses.borrow().iter().find(|address| {
                address
                    .iter()
                    .any(|protocol| protocol == libp2p::multiaddr::Protocol::QuicV1)
            }) {
                return Ok(address.clone());
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
    wait(&owner.shared, "join request", |view| {
        view.pending.iter().any(|pending| pending.peer_id == member_id)
    })
    .await?;
    owner.shared.lock().await.approve(&member_id)?;
    wait(&member.shared, "approved membership", |view| {
        view.active.is_some() && !view.joining
    })
    .await
}

// Engine-only cases use an owner-signed roster and independent persisted identities.
// The network test below exercises invitations and approvals over real QUIC streams.
async fn signed_group(root: &Path) -> Result<(Shared, Shared, Shared, Snapshot)> {
    let owner = Engine::open(&root.join("owner"), Arc::new(|_| {}))?;
    let sender = Engine::open(&root.join("sender"), Arc::new(|_| {}))?;
    let receiver = Engine::open(&root.join("receiver"), Arc::new(|_| {}))?;
    owner.lock().await.set_name("Owner".into())?;
    sender.lock().await.set_name("Sender".into())?;
    receiver.lock().await.set_name("Receiver".into())?;
    owner.lock().await.create_workspace("Friends".into())?;
    let mut snapshot = owner.lock().await.active_snapshot()?;
    snapshot.members.push(Member {
        peer_id: sender.lock().await.peer(),
        name: "Sender".into(),
    });
    snapshot.members.push(Member {
        peer_id: receiver.lock().await.peer(),
        name: "Receiver".into(),
    });
    snapshot.revision += 1;
    snapshot.sign(&owner.lock().await.key)?;
    owner.lock().await.accept_snapshot(&snapshot)?;
    let workspace = owner.lock().await.persisted.workspaces[&snapshot.workspace_id].clone();
    for shared in [&sender, &receiver] {
        let mut engine = shared.lock().await;
        let mut next = engine.persisted.clone();
        next.workspaces
            .insert(snapshot.workspace_id, workspace.clone());
        next.active = Some(snapshot.workspace_id);
        engine.persist(next)?;
    }
    Ok((owner, sender, receiver, snapshot))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_quic_chat_continues_without_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let sender = Engine::open(&root.path().join("sender"), Arc::new(|_| {}))?;
    let receiver = Engine::open(&root.path().join("receiver"), Arc::new(|_| {}))?;
    owner.lock().await.set_name("Owner".into())?;
    sender.lock().await.set_name("Sender".into())?;
    receiver.lock().await.set_name("Receiver".into())?;
    owner.lock().await.create_workspace("Friends".into())?;
    let mut stop = StopNodes::default();
    let owner_node = Node::start(owner.clone(), false).await?;
    stop.0.push(owner_node.shutdown.clone());
    let sender_node = Node::start(sender.clone(), false).await?;
    stop.0.push(sender_node.shutdown.clone());
    let receiver_node = Node::start(receiver.clone(), false).await?;
    stop.0.push(receiver_node.shutdown.clone());
    join(&owner_node, &sender_node).await?;
    join(&owner_node, &receiver_node).await?;
    let owner_id = owner.lock().await.peer();
    let sender_id = sender.lock().await.peer();
    let receiver_id = receiver.lock().await.peer();
    let sender_peer: PeerId = sender_id.parse()?;
    receiver_node
        .connect(sender_peer, address(&sender_node).await?)
        .await?;
    wait(&sender, "authenticated receiver connection", |view| {
        view.online.contains(&receiver_id)
    })
    .await?;
    receiver_node.refresh_peer(sender_peer).await?;
    assert_eq!(
        sender.lock().await.active_snapshot()?,
        receiver.lock().await.active_snapshot()?
    );

    let workspace = sender.lock().await.active_snapshot()?.workspace_id;
    wait(&owner, "both approved recipients online", |view| {
        view.online.contains(&sender_id) && view.online.contains(&receiver_id)
    })
    .await?;
    let welcome = owner_node.send_chat(workspace, "Welcome".into()).await?;
    for shared in [&sender, &receiver] {
        wait(shared, "group broadcast receipt", |view| {
            view.chat_messages
                .iter()
                .any(|entry| entry.message.id == welcome && entry.message.body == "Welcome")
        })
        .await?;
    }
    wait(&owner, "both broadcast acknowledgements", |view| {
        view.chat_messages.iter().any(|entry| {
            entry.message.id == welcome
                && entry.delivery.as_ref().is_some_and(|delivery| {
                    delivery.status == ChatDeliveryStatus::Complete
                        && delivery.attempted == 2
                        && delivery.delivered == 2
                })
        })
    })
    .await?;

    owner_node.shutdown_and_wait().await;
    wait(&sender, "owner offline and receiver online", |view| {
        !view.online.contains(&owner_id) && view.online.contains(&receiver_id)
    })
    .await?;
    wait(&receiver, "owner offline and sender online", |view| {
        !view.online.contains(&owner_id) && view.online.contains(&sender_id)
    })
    .await?;
    let body = "สวัสดีเพื่อน 👋\nsecond line\twith a tab";
    let id = sender_node.send_chat(workspace, body.into()).await?;
    assert!(
        sender
            .lock()
            .await
            .view()
            .chat_messages
            .iter()
            .any(|entry| entry.message.id == id),
        "send must save locally before returning"
    );
    wait(&receiver, "chat receipt", |view| {
        view.chat_messages.iter().any(|entry| entry.message.id == id)
    })
    .await?;
    wait(&sender, "chat acknowledgement", |view| {
        view.chat_messages.iter().any(|entry| {
            entry.message.id == id
                && entry.delivery.as_ref().is_some_and(|delivery| {
                    delivery.status == ChatDeliveryStatus::Complete
                })
        })
    })
    .await?;
    let sent = sender.lock().await.view().chat_messages;
    let received = receiver.lock().await.view().chat_messages;
    assert_eq!(sent.len(), 2);
    assert_eq!(received.len(), 2);
    let sent_message = sent.iter().find(|entry| entry.message.id == id).unwrap();
    let received_message = received.iter().find(|entry| entry.message.id == id).unwrap();
    assert_eq!(sent_message.message, received_message.message);
    assert_eq!(received_message.message.body, body);
    assert_eq!(received_message.message.workspace_id, workspace);
    assert_eq!(received_message.message.sender_peer_id, sender_id);
    assert_eq!(received_message.message.sender_name, "Sender");
    assert!(received_message.delivery.is_none());
    let delivery = sent_message.delivery.as_ref().unwrap();
    assert_eq!((delivery.attempted, delivery.delivered), (1, 1));
    let owner_history = owner.lock().await.view().chat_messages;
    assert_eq!(owner_history.len(), 1);
    assert_eq!(owner_history[0].message.id, welcome);

    let reply = receiver_node.send_chat(workspace, "Got it".into()).await?;
    wait(&sender, "reply receipt", |view| {
        view.chat_messages.iter().any(|entry| entry.message.id == reply)
    })
    .await?;
    wait(&receiver, "reply acknowledgement", |view| {
        view.chat_messages.iter().any(|entry| {
            entry.message.id == reply
                && entry.delivery.as_ref().is_some_and(|delivery| {
                    delivery.status == ChatDeliveryStatus::Complete && delivery.delivered == 1
                })
        })
    })
    .await?;
    tokio::join!(
        sender_node.shutdown_and_wait(),
        receiver_node.shutdown_and_wait()
    );
    Ok(())
}

#[tokio::test]
async fn receive_checks_identity_membership_workspace_and_duplicate_contents() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, sender, receiver, snapshot) = signed_group(root.path()).await?;
    let sender_id = sender.lock().await.peer();
    let owner_id = owner.lock().await.peer();
    let message = sender
        .lock()
        .await
        .compose_chat(snapshot.workspace_id, "Original message".into(), 2)?;
    receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, message.clone())?;
    receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, message.clone())?;
    let received = receiver.lock().await.view().chat_messages;
    assert_eq!(
        received.len(),
        1,
        "an identical retry must not add a second row"
    );
    assert_eq!(received[0].message, message);
    assert!(received[0].delivery.is_none());

    let mut forged = message.clone();
    forged.id = uuid::Uuid::new_v4();
    forged.body = "Altered without a signature".into();
    assert!(receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, forged)
        .is_err());
    let mut conflict = message.clone();
    conflict.body = "Different signed content with the same UUID".into();
    conflict.sign(&sender.lock().await.key)?;
    assert!(receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, conflict)
        .is_err());
    assert!(receiver
        .lock()
        .await
        .receive_chat(&owner_id, &snapshot, message.clone())
        .is_err());

    let outsider_key = Keypair::generate_ed25519();
    let outsider = ChatMessage::new(
        snapshot.workspace_id,
        "Outsider".into(),
        "Valid signature, no membership".into(),
        &outsider_key,
    )?;
    let outsider_id = outsider.sender_peer_id.clone();
    assert!(receiver
        .lock()
        .await
        .receive_chat(&outsider_id, &snapshot, outsider)
        .is_err());
    assert_eq!(receiver.lock().await.persisted.chat_messages.len(), 1);

    // Both identities remain approved here, isolating active-workspace rejection.
    receiver.lock().await.create_workspace("Other group".into())?;
    assert!(receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, message.clone())
        .is_err());
    assert!(receiver.lock().await.view().chat_messages.is_empty());
    receiver.lock().await.activate(snapshot.workspace_id)?;
    assert_eq!(receiver.lock().await.view().chat_messages, received);

    owner.lock().await.remove_member(&sender_id)?;
    let revoked = owner.lock().await.active_snapshot()?;
    receiver.lock().await.accept_snapshot(&revoked)?;
    // Revocation applies even to an otherwise idempotent retry carrying an old roster.
    assert!(receiver
        .lock()
        .await
        .receive_chat(&sender_id, &snapshot, message.clone())
        .is_err());
    let after_revocation = ChatMessage::new(
        snapshot.workspace_id,
        "Sender".into(),
        "Signed after removal".into(),
        &sender.lock().await.key,
    )?;
    assert!(receiver
        .lock()
        .await
        .receive_chat(&sender_id, &revoked, after_revocation)
        .is_err());
    assert_eq!(receiver.lock().await.view().chat_messages, received);
    assert_eq!(receiver.lock().await.persisted.chat_messages.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_send_and_restart_preserve_history_and_interrupt_unfinished_delivery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (owner, sender, receiver, snapshot) = signed_group(root.path()).await?;
    let mut stop = StopNodes::default();
    let node = Node::start(sender.clone(), false).await?;
    stop.0.push(node.shutdown.clone());
    let workspace = snapshot.workspace_id;
    // Both approved recipients are offline: the send still has a durable local result.
    let offline = node
        .send_chat(workspace, "Keep this while offline".into())
        .await?;
    wait(&sender, "zero-recipient completion", |view| {
        view.chat_messages.iter().any(|entry| {
            entry.message.id == offline
                && entry.delivery.as_ref().is_some_and(|delivery| {
                    delivery.status == ChatDeliveryStatus::Complete
                })
        })
    })
    .await?;
    let offline_entry = sender.lock().await.view().chat_messages[0].clone();
    let delivery = offline_entry.delivery.as_ref().unwrap();
    assert_eq!((delivery.attempted, delivery.delivered), (0, 0));
    assert!(receiver.lock().await.view().chat_messages.is_empty());

    let pending = sender
        .lock()
        .await
        .compose_chat(workspace, "In progress at exit".into(), 2)?;
    let completed = sender
        .lock()
        .await
        .compose_chat(workspace, "One acknowledgement".into(), 2)?;
    sender.lock().await.finish_chat(completed.id, 1, false)?;
    let incoming = owner
        .lock()
        .await
        .compose_chat(workspace, "Received before restart".into(), 2)?;
    let owner_id = owner.lock().await.peer();
    sender
        .lock()
        .await
        .receive_chat(&owner_id, &snapshot, incoming.clone())?;
    sender.lock().await.create_workspace("Other group".into())?;
    let count_before_rejection = sender.lock().await.persisted.chat_messages.len();
    assert!(node
        .send_chat(workspace, "Stale composer".into())
        .await
        .is_err());
    assert_eq!(
        sender.lock().await.persisted.chat_messages.len(),
        count_before_rejection
    );
    assert!(sender.lock().await.view().chat_messages.is_empty());
    sender.lock().await.activate(workspace)?;
    assert_eq!(
        sender
            .lock()
            .await
            .view()
            .chat_messages
            .iter()
            .find(|entry| entry.message.id == pending.id)
            .unwrap()
            .delivery
            .as_ref()
            .unwrap()
            .status,
        ChatDeliveryStatus::Sending
    );
    node.shutdown_and_wait().await;

    let reopened = Engine::open(&root.path().join("sender"), Arc::new(|_| {}))?;
    let restored = reopened.lock().await.view().chat_messages;
    assert_eq!(restored.len(), 4);
    assert_eq!(
        restored
            .iter()
            .find(|entry| entry.message.id == offline)
            .unwrap(),
        &offline_entry
    );
    let pending_entry = restored
        .iter()
        .find(|entry| entry.message.id == pending.id)
        .unwrap();
    assert_eq!(pending_entry.message, pending);
    let delivery = pending_entry.delivery.as_ref().unwrap();
    assert_eq!(delivery.status, ChatDeliveryStatus::Interrupted);
    assert_eq!((delivery.attempted, delivery.delivered), (2, 0));
    let complete_entry = restored
        .iter()
        .find(|entry| entry.message.id == completed.id)
        .unwrap();
    assert_eq!(complete_entry.message, completed);
    let delivery = complete_entry.delivery.as_ref().unwrap();
    assert_eq!(delivery.status, ChatDeliveryStatus::Complete);
    assert_eq!((delivery.attempted, delivery.delivered), (2, 1));
    let incoming_entry = restored
        .iter()
        .find(|entry| entry.message.id == incoming.id)
        .unwrap();
    assert_eq!(incoming_entry.message, incoming);
    assert!(incoming_entry.delivery.is_none());
    let reopened_again = Engine::open(&root.path().join("sender"), Arc::new(|_| {}))?;
    assert_eq!(reopened_again.lock().await.view().chat_messages, restored);
    Ok(())
}
