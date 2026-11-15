//! Signed, online-only chat. Replay receipts outlive display-history pruning.
use crate::model::{digest, now, public_key, sign, validate_label, verify, MAX_MEMBERS};
use anyhow::{ensure, Context, Result};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const MAX_BODY: usize = 4096;
const PER_ROOM: usize = 500;
const TOTAL: usize = 2000;
const REPLAY_SENDERS: usize = 1024;
const WINDOW: u64 = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    pub version: u8,
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub sender_peer_id: String,
    pub sender_public_key: String,
    pub roster_revision: u64,
    pub sequence: u64,
    pub sent_at: u64,
    pub body: String,
    pub signature: String,
}
impl ChatMessage {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/chat/2",
            self.version,
            self.id,
            self.workspace_id,
            &self.sender_peer_id,
            &self.sender_public_key,
            self.roster_revision,
            self.sequence,
            self.sent_at,
            &self.body,
        )
    }
    pub fn new(
        key: &Keypair,
        workspace: Uuid,
        revision: u64,
        sequence: u64,
        body: String,
    ) -> Result<Self> {
        validate_body(&body)?;
        let mut message = Self {
            version: 2,
            id: Uuid::new_v4(),
            workspace_id: workspace,
            sender_peer_id: key.public().to_peer_id().to_string(),
            sender_public_key: public_key(key),
            roster_revision: revision,
            sequence,
            sent_at: now(),
            body,
            signature: String::new(),
        };
        let signature = sign(key, &message.payload())?;
        message.signature = signature;
        message.verify()?;
        Ok(message)
    }
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.version == 2 && self.roster_revision > 0 && self.sequence > 0,
            "Unsupported chat message"
        );
        validate_body(&self.body)?;
        verify(
            &self.sender_public_key,
            &self.sender_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
    pub fn verify_incoming(&self) -> Result<()> {
        self.verify()?;
        ensure!(
            self.sent_at <= now().saturating_add(300),
            "Chat timestamp is in the future"
        );
        Ok(())
    }
}
pub fn validate_body(body: &str) -> Result<()> {
    ensure!(
        !body.trim().is_empty() && body.len() <= MAX_BODY,
        "Use a message between 1 and 4096 UTF-8 bytes"
    );
    ensure!(
        body.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t'),
        "Message contains unsupported control characters"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Sending,
    Complete,
    Interrupted,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub status: DeliveryStatus,
    pub attempted: usize,
    pub acknowledged: usize,
    pub offline: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredChat {
    pub message: ChatMessage,
    pub sender_name: String,
    pub delivery: Option<Delivery>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    sequence: u64,
    id: Uuid,
    digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWindow {
    workspace: Uuid,
    sender: String,
    epoch: u64,
    highest: u64,
    receipts: Vec<Receipt>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatState {
    pub messages: Vec<StoredChat>,
    pub sequences: BTreeMap<Uuid, u64>,
    pub replay: BTreeMap<String, ReplayWindow>,
}
impl ChatState {
    pub fn next_sequence(&mut self, workspace: Uuid) -> Result<u64> {
        ensure!(
            self.sequences.contains_key(&workspace) || self.sequences.len() < REPLAY_SENDERS,
            "Chat room limit reached"
        );
        let sequence = self.sequences.entry(workspace).or_default();
        *sequence = sequence.checked_add(1).context("Chat sequence exhausted")?;
        Ok(*sequence)
    }
    pub fn append(&mut self, message: StoredChat) {
        self.messages.push(message);
        let mut rooms = BTreeMap::<Uuid, usize>::new();
        let mut kept: Vec<_> = self
            .messages
            .iter()
            .rev()
            .filter(|entry| {
                let count = rooms.entry(entry.message.workspace_id).or_default();
                *count += 1;
                *count <= PER_ROOM
            })
            .take(TOTAL)
            .cloned()
            .collect();
        kept.reverse();
        self.messages = kept;
    }
    /// Caller must first authenticate sender and authorize the current signed roster.
    /// Mutate a cloned Persisted value, so failure to save cannot consume a receipt.
    pub fn record_received(&mut self, message: ChatMessage, sender_name: String) -> Result<bool> {
        message.verify_incoming()?;
        validate_label(&sender_name)?;
        let key = format!("{}:{}", message.workspace_id, message.sender_peer_id);
        ensure!(
            self.replay.contains_key(&key) || self.replay.len() < REPLAY_SENDERS,
            "Chat replay state is full"
        );
        let bytes = serde_json::to_vec(&message)?;
        let fingerprint = digest(&bytes);
        if let Some(old) = self.messages.iter().find(|entry| {
            entry.message.workspace_id == message.workspace_id
                && entry.message.sender_peer_id == message.sender_peer_id
                && entry.message.id == message.id
        }) {
            ensure!(old.message == message, "Conflicting chat message ID");
        }
        let window = self.replay.entry(key).or_insert_with(|| ReplayWindow {
            workspace: message.workspace_id,
            sender: message.sender_peer_id.clone(),
            epoch: message.roster_revision,
            highest: 0,
            receipts: Vec::new(),
        });
        ensure!(
            message.roster_revision >= window.epoch,
            "Stale chat membership epoch"
        );
        if message.roster_revision > window.epoch {
            window.epoch = message.roster_revision;
            window.highest = 0;
            window.receipts.clear();
        }
        ensure!(
            message.sequence > window.highest.saturating_sub(WINDOW),
            "Chat sequence is outside the replay window"
        );
        if let Some(old) = window
            .receipts
            .iter()
            .find(|entry| entry.sequence == message.sequence || entry.id == message.id)
        {
            ensure!(
                old.sequence == message.sequence
                    && old.id == message.id
                    && old.digest == fingerprint,
                "Conflicting chat sequence"
            );
            return Ok(false);
        }
        window.highest = window.highest.max(message.sequence);
        let minimum = window.highest.saturating_sub(WINDOW);
        window.receipts.retain(|receipt| receipt.sequence > minimum);
        window.receipts.push(Receipt {
            sequence: message.sequence,
            id: message.id,
            digest: fingerprint,
        });
        self.append(StoredChat {
            message,
            sender_name,
            delivery: None,
        });
        Ok(true)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.messages.len() <= TOTAL
                && self.replay.len() <= REPLAY_SENDERS
                && self.sequences.len() <= REPLAY_SENDERS,
            "Chat state exceeds limits"
        );
        let mut rooms = BTreeMap::<Uuid, usize>::new();
        let mut ids = BTreeSet::new();
        for entry in &self.messages {
            entry.message.verify()?;
            validate_label(&entry.sender_name)?;
            ensure!(
                ids.insert((
                    entry.message.workspace_id,
                    &entry.message.sender_peer_id,
                    entry.message.id
                )),
                "Duplicate chat history"
            );
            let count = rooms.entry(entry.message.workspace_id).or_default();
            *count += 1;
            ensure!(*count <= PER_ROOM, "Chat room history exceeds limit");
            if let Some(delivery) = &entry.delivery {
                ensure!(
                    delivery.acknowledged <= delivery.attempted
                        && delivery.attempted.saturating_add(delivery.offline) < MAX_MEMBERS,
                    "Invalid chat delivery counts"
                );
            }
        }
        for (key, window) in &self.replay {
            ensure!(
                *key == format!("{}:{}", window.workspace, window.sender)
                    && window.epoch > 0
                    && window.highest > 0
                    && !window.receipts.is_empty()
                    && window.receipts.len() <= WINDOW as usize,
                "Invalid chat replay state"
            );
            let mut sequences = BTreeSet::new();
            let mut ids = BTreeSet::new();
            for receipt in &window.receipts {
                ensure!(
                    receipt.sequence > window.highest.saturating_sub(WINDOW)
                        && receipt.sequence <= window.highest
                        && sequences.insert(receipt.sequence)
                        && ids.insert(receipt.id)
                        && receipt.digest.len() == 64
                        && hex::decode(&receipt.digest)?.len() == 32,
                    "Invalid chat receipt"
                );
            }
        }
        Ok(())
    }
    pub fn validate_for(
        &self,
        own_peer: &str,
        workspaces: &BTreeMap<Uuid, crate::model::Workspace>,
    ) -> Result<()> {
        self.validate()?;
        for entry in &self.messages {
            ensure!(
                workspaces.contains_key(&entry.message.workspace_id),
                "Unknown chat workspace"
            );
            ensure!(
                entry.delivery.is_none() || entry.message.sender_peer_id == own_peer,
                "Invalid outgoing chat sender"
            );
        }
        for window in self.replay.values() {
            ensure!(
                workspaces
                    .get(&window.workspace)
                    .is_some_and(|w| window.epoch <= w.snapshot.revision),
                "Invalid replay membership epoch"
            );
        }
        Ok(())
    }
}
