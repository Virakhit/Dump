use crate::model::{public_key, sign, validate_label, verify, MAX_MEMBERS, VERSION};
use anyhow::{ensure, Result};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const MAX_CHAT_BODY: usize = 4096;
pub const MAX_CHAT_PER_WORKSPACE: usize = 500;
pub const MAX_CHAT_TOTAL: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    pub version: u8,
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub sender_peer_id: String,
    pub sender_public_key: String,
    pub sender_name: String,
    pub body: String,
    pub sent_at: u64,
    pub signature: String,
}

impl ChatMessage {
    pub fn new(
        workspace_id: Uuid,
        sender_name: String,
        body: String,
        key: &Keypair,
    ) -> Result<Self> {
        validate_label(&sender_name)?;
        validate_body(&body)?;
        let mut message = Self {
            version: VERSION,
            id: Uuid::new_v4(),
            workspace_id,
            sender_peer_id: key.public().to_peer_id().to_string(),
            sender_public_key: public_key(key),
            sender_name,
            body,
            sent_at: crate::model::now(),
            signature: String::new(),
        };
        message.sign(key)?;
        Ok(message)
    }

    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/chat/1",
            self.version,
            self.id,
            self.workspace_id,
            &self.sender_peer_id,
            &self.sender_public_key,
            &self.sender_name,
            &self.body,
            self.sent_at,
        )
    }

    pub fn sign(&mut self, key: &Keypair) -> Result<()> {
        let signature = sign(key, &self.payload())?;
        self.signature = signature;
        self.verify()
    }

    pub fn verify(&self) -> Result<()> {
        ensure!(self.version == VERSION, "Unsupported chat version");
        validate_label(&self.sender_name)?;
        validate_body(&self.body)?;
        verify(
            &self.sender_public_key,
            &self.sender_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
}

pub fn validate_body(body: &str) -> Result<()> {
    ensure!(!body.trim().is_empty(), "Write a message before sending");
    ensure!(
        body.len() <= MAX_CHAT_BODY,
        "Chat messages must be at most 4096 UTF-8 bytes"
    );
    ensure!(
        !body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t'),
        "Chat messages may contain newlines and tabs, but no other control characters"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatDeliveryStatus {
    Sending,
    Complete,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatDelivery {
    pub attempted: usize,
    pub delivered: usize,
    pub status: ChatDeliveryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoredChat {
    pub message: ChatMessage,
    pub delivery: Option<ChatDelivery>,
}

/// Retain the newest arrivals, without trusting sender clocks for ordering.
pub fn trim_history(messages: &mut Vec<StoredChat>) {
    let mut counts = BTreeMap::<Uuid, usize>::new();
    messages.reverse();
    messages.retain(|entry| {
        let count = counts.entry(entry.message.workspace_id).or_default();
        *count += 1;
        *count <= MAX_CHAT_PER_WORKSPACE
    });
    messages.truncate(MAX_CHAT_TOTAL);
    messages.reverse();
}

/// Validate persisted history once on load. Current rosters deliberately do not
/// govern already received messages: removing a member does not erase history.
pub fn validate_history(messages: &[StoredChat], own_peer: &str) -> Result<()> {
    ensure!(messages.len() <= MAX_CHAT_TOTAL, "Too much chat history");
    let mut counts = BTreeMap::<Uuid, usize>::new();
    let mut ids = BTreeSet::new();
    for entry in messages {
        entry.message.verify()?;
        ensure!(ids.insert(entry.message.id), "Duplicate chat message ID");
        let count = counts.entry(entry.message.workspace_id).or_default();
        *count += 1;
        ensure!(
            *count <= MAX_CHAT_PER_WORKSPACE,
            "Too much workspace chat history"
        );
        if let Some(delivery) = &entry.delivery {
            ensure!(
                entry.message.sender_peer_id == own_peer
                    && delivery.attempted < MAX_MEMBERS
                    && delivery.delivered <= delivery.attempted,
                "Invalid chat delivery state"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_limits_count_utf8_bytes_and_allow_plain_text() -> Result<()> {
        validate_body(&"x".repeat(MAX_CHAT_BODY))?;
        assert!(validate_body(&"x".repeat(MAX_CHAT_BODY + 1)).is_err());
        let thai = format!("{}x", "ก".repeat(MAX_CHAT_BODY / "ก".len()));
        assert_eq!(thai.len(), MAX_CHAT_BODY);
        validate_body(&thai)?;
        assert!(validate_body(&(thai + "ก")).is_err());
        validate_body("สวัสดี\r\nเพื่อน\t<script>plain text</script>")?;
        for body in [
            "",
            " \t\r\n ",
            "null\0",
            "bell\u{7}",
            "escape\u{1b}",
            "delete\u{7f}",
        ] {
            assert!(validate_body(body).is_err(), "accepted {body:?}");
        }
        Ok(())
    }

    fn reject_change(message: &ChatMessage, change: impl FnOnce(&mut ChatMessage)) {
        let mut altered = message.clone();
        change(&mut altered);
        assert!(altered.verify().is_err());
    }

    #[test]
    fn signature_binds_every_message_field_and_author() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let other = Keypair::generate_ed25519();
        let message = ChatMessage::new(Uuid::new_v4(), "Alice".into(), "Hello".into(), &key)?;
        message.verify()?;
        reject_change(&message, |m| m.version += 1);
        reject_change(&message, |m| m.id = Uuid::new_v4());
        reject_change(&message, |m| m.workspace_id = Uuid::new_v4());
        reject_change(&message, |m| {
            m.sender_peer_id = other.public().to_peer_id().to_string()
        });
        reject_change(&message, |m| m.sender_public_key = public_key(&other));
        reject_change(&message, |m| m.sender_name = "Bob".into());
        reject_change(&message, |m| m.body = "Different text".into());
        reject_change(&message, |m| m.sent_at = m.sent_at.wrapping_add(1));
        reject_change(&message, |m| m.signature.push('A'));
        let mut wrong_signer = message;
        assert!(wrong_signer.sign(&other).is_err());
        Ok(())
    }

    #[test]
    fn retention_keeps_latest_arrivals_per_workspace_and_globally() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let rooms: Vec<_> = (0..5).map(|_| Uuid::new_v4()).collect();
        let base = ChatMessage::new(rooms[0], "Alice".into(), "Hello".into(), &key)?;
        let make_entry = |index: usize, workspace_id: Uuid| {
            // Retention uses only arrival order and room IDs, not signatures
            // or author clocks. Deliberately make timestamps run backwards.
            let mut message = base.clone();
            message.id = Uuid::from_u128(index as u128 + 1);
            message.workspace_id = workspace_id;
            message.sent_at = u64::MAX - index as u64;
            StoredChat {
                message,
                delivery: None,
            }
        };
        let mut one_room: Vec<_> = (0..MAX_CHAT_PER_WORKSPACE + 3)
            .map(|index| make_entry(index, rooms[0]))
            .collect();
        let expected = one_room[3..].to_vec();
        trim_history(&mut one_room);
        assert_eq!(one_room, expected);
        assert_eq!(one_room.len(), MAX_CHAT_PER_WORKSPACE);

        let mut many_rooms: Vec<_> = (0..(MAX_CHAT_PER_WORKSPACE + 3) * rooms.len())
            .map(|index| make_entry(index, rooms[index % rooms.len()]))
            .collect();
        let expected = many_rooms[many_rooms.len() - MAX_CHAT_TOTAL..].to_vec();
        trim_history(&mut many_rooms);
        assert_eq!(many_rooms, expected);
        assert_eq!(many_rooms.len(), MAX_CHAT_TOTAL);
        for room in rooms {
            assert!(
                many_rooms
                    .iter()
                    .filter(|entry| entry.message.workspace_id == room)
                    .count()
                    <= MAX_CHAT_PER_WORKSPACE
            );
        }
        Ok(())
    }

    #[test]
    fn older_state_without_chat_history_still_decodes() -> Result<()> {
        let original = crate::model::Persisted::new()?;
        let mut value = serde_json::to_value(&original)?;
        value.as_object_mut().unwrap().remove("chat_messages");
        let restored: crate::model::Persisted = serde_json::from_value(value)?;
        assert!(restored.chat_messages.is_empty());
        assert_eq!(restored.identity, original.identity);
        assert_eq!(restored.relay_identity, original.relay_identity);
        Ok(())
    }
}
