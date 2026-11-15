//! Versioned chat stream; the legacy control/file messages are untouched.
use super::*;
use crate::chat::ChatMessage;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatRequest {
    snapshot: Snapshot,
    message: ChatMessage,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum ChatResponse {
    Stored { message_id: Uuid },
    Denied,
}

impl Node {
    /// Returns after durable local save. Network ACKs update the stored history.
    /// No offline recipients are queued and no failed transmission is retried.
    pub async fn send_chat(&self, workspace: Uuid, body: String) -> Result<Uuid> {
        ensure!(!self.shutdown.is_cancelled(), "Networking is stopped");
        let permit = self
            .limits
            .chat_send
            .clone()
            .try_acquire_owned()
            .context("Wait for current chat sends to finish")?;
        let (message, targets, cancelled) = {
            let mut e = self.shared.lock().await;
            let snapshot = e.active_snapshot()?;
            ensure!(
                snapshot.workspace_id == workspace && snapshot.contains(&e.peer()),
                "Chat workspace changed or membership was revoked"
            );
            let own_peer = e.peer();
            let targets = snapshot
                .members
                .iter()
                .filter(|member| {
                    member.peer_id != own_peer
                        && e.online
                            .get(&member.peer_id)
                            .is_some_and(|seen| now().saturating_sub(*seen) < 15)
                })
                .map(|member| member.peer_id.parse::<PeerId>())
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let offline = snapshot.members.len() - 1 - targets.len();
            let message = e.compose_chat(workspace, body, targets.len(), offline)?;
            (message, targets, e.chat_cancel.clone())
        };
        let id = message.id;
        let node = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let sends = futures::stream::iter(targets.into_iter().map(|peer| {
                let sender = node.clone();
                let message = message.clone();
                async move { sender.send_chat_to(peer, &message).await }
            }))
            .buffer_unordered(4);
            tokio::pin!(sends);
            let mut acknowledged = 0;
            let mut interrupted = false;
            loop {
                tokio::select! {
                    biased;
                    _ = node.shutdown.cancelled() => { interrupted = true; break; },
                    _ = cancelled.cancelled() => { interrupted = true; break; },
                    result = sends.next() => match result {
                        Some(Ok(())) => acknowledged += 1,
                        Some(Err(_)) => {},
                        None => break,
                    },
                }
            }
            let mut e = node.shared.lock().await;
            if e.finish_chat(id, acknowledged, interrupted).is_err() {
                e.error("Chat was saved locally, but its delivery result could not be saved.");
            }
        });
        Ok(id)
    }
    async fn chat_snapshot(&self, peer: PeerId, message: &ChatMessage) -> Result<Snapshot> {
        let e = self.shared.lock().await;
        let snapshot = e.active_snapshot()?;
        ensure!(
            snapshot.workspace_id == message.workspace_id
                && snapshot.revision == message.roster_revision
                && message.sender_peer_id == e.peer()
                && snapshot.contains(&e.peer())
                && snapshot.contains(&peer.to_string()),
            "Chat membership or workspace changed"
        );
        Ok(snapshot)
    }
    async fn send_chat_to(&self, peer: PeerId, message: &ChatMessage) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            self.chat_snapshot(peer, message).await?;
            let mut control = self.control.clone();
            let mut stream = control.open_stream(peer, CHAT).await?;
            let snapshot = self.chat_snapshot(peer, message).await?;
            write_frame(
                &mut stream,
                &ChatRequest {
                    snapshot,
                    message: message.clone(),
                },
            )
            .await?;
            match read_frame::<_, ChatResponse>(&mut stream).await? {
                ChatResponse::Stored { message_id } if message_id == message.id => Ok(()),
                _ => bail!("Chat storage was not acknowledged"),
            }
        })
        .await?
    }
}

pub(super) async fn serve(shared: Shared, peer: PeerId, mut stream: Stream) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request: ChatRequest = read_frame(&mut stream).await?;
    let id = request.message.id;
    let response = {
        let mut e = shared.lock().await;
        if e.receive_chat(&peer.to_string(), &request.snapshot, request.message)
            .is_ok()
        {
            ChatResponse::Stored { message_id: id }
        } else {
            ChatResponse::Denied
        }
    };
    // receive_chat must save message + replay receipt before this ACK is written.
    write_frame(&mut stream, &response).await?;
    stream.close().await?;
    Ok(())
}
