use crate::{engine::Engine, model::Snapshot, overlay::OverlayContact};
use anyhow::{ensure, Result};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite};
use libp2p::PeerId;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

const MAX_FRAME: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OverlayRequest {
    pub workspace_id: Uuid,
    pub revision: u64,
    pub roster_signature: String,
    pub contact: Option<OverlayContact>,
    pub offset: u32,
}

impl OverlayRequest {
    pub fn new(snapshot: &Snapshot, contact: Option<OverlayContact>, offset: u32) -> Self {
        Self {
            workspace_id: snapshot.workspace_id,
            revision: snapshot.revision,
            roster_signature: snapshot.signature.clone(),
            contact,
            offset,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OverlayResponse {
    pub total: u32,
    pub contacts: Vec<OverlayContact>,
}

pub(super) async fn write<S: AsyncWrite + Unpin, T: Serialize>(
    stream: &mut S,
    value: &T,
) -> Result<()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)?;
    ensure!(bytes.len() <= MAX_FRAME, "Overlay frame exceeds limit");
    // Keep the existing CBOR/length-prefix encoding and pre-authentication
    // deadline after enforcing this protocol's stricter payload limit.
    super::super::write_frame(stream, value).await
}

pub(super) async fn read<S: AsyncRead + Unpin, T: DeserializeOwned>(stream: &mut S) -> Result<T> {
    tokio::time::timeout(super::super::PREAUTH_TIMEOUT, async {
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).await?;
        let length = u32::from_be_bytes(prefix) as usize;
        ensure!(
            length > 0 && length <= MAX_FRAME,
            "Invalid overlay frame length"
        );
        let mut bytes = vec![0u8; length];
        stream.read_exact(&mut bytes).await?;
        let mut cursor = std::io::Cursor::new(bytes);
        let value = ciborium::from_reader(&mut cursor)?;
        ensure!(
            cursor.position() as usize == length,
            "Trailing overlay data"
        );
        Ok(value)
    })
    .await?
}

pub(super) fn authorize_reference(
    engine: &mut Engine,
    peer: PeerId,
    request: &OverlayRequest,
) -> Result<Snapshot> {
    ensure!(request.offset <= 256, "Invalid overlay offset");
    let current = engine.active_snapshot()?;
    ensure!(
        request.workspace_id == current.workspace_id
            && request.revision == current.revision
            && request.roster_signature == current.signature,
        "Overlay roster reference is out of sync; refresh workspace membership"
    );
    // The regular authenticated control protocol supplies full signed rosters.
    // This smaller exchange can only use the exact roster already held locally.
    engine.authorize(&peer.to_string(), &current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{public_key, Member, MAX_MEMBERS, VERSION};
    use futures::io::Cursor;
    use libp2p::identity::Keypair;
    #[cfg(windows)]
    use std::sync::Arc;

    fn full_roster(owner: &Keypair) -> Result<Snapshot> {
        let mut members = vec![Member {
            peer_id: owner.public().to_peer_id().to_string(),
            name: "Owner".into(),
        }];
        for index in 1..MAX_MEMBERS {
            members.push(Member {
                peer_id: Keypair::generate_ed25519()
                    .public()
                    .to_peer_id()
                    .to_string(),
                name: format!("Approved member {index}"),
            });
        }
        let mut snapshot = Snapshot {
            version: VERSION,
            workspace_id: Uuid::new_v4(),
            name: "Full approved roster".into(),
            owner_peer_id: owner.public().to_peer_id().to_string(),
            owner_public_key: public_key(owner),
            revision: 5,
            members,
            signature: String::new(),
        };
        snapshot.sign(owner)?;
        Ok(snapshot)
    }

    #[tokio::test]
    async fn maximum_signed_roster_reference_and_contact_fit_overlay_frame_and_roundtrip(
    ) -> Result<()> {
        let owner = Keypair::generate_ed25519();
        let snapshot = full_roster(&owner)?;
        snapshot.verify()?;
        assert_eq!(snapshot.members.len(), 128);
        let mut old_roster_payload = Vec::new();
        ciborium::into_writer(&snapshot, &mut old_roster_payload)?;
        assert!(old_roster_payload.len() > 8192);
        let contact = OverlayContact::issue(
            &owner,
            snapshot.workspace_id,
            "owner-node".into(),
            "owner.tail123.ts.net".into(),
            1,
        )?;
        let request = OverlayRequest::new(&snapshot, Some(contact.clone()), 0);
        let mut wire = Cursor::new(Vec::new());
        write(&mut wire, &request).await?;
        let bytes = wire.get_ref();
        assert!(bytes.len() <= 8192 + 4);
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into()?) as usize,
            bytes.len() - 4
        );
        wire.set_position(0);
        let decoded: OverlayRequest = read(&mut wire).await?;
        assert_eq!(decoded.workspace_id, snapshot.workspace_id);
        assert_eq!(decoded.revision, snapshot.revision);
        assert_eq!(decoded.roster_signature, snapshot.signature);
        assert_eq!(decoded.contact, Some(contact));
        assert_eq!(decoded.offset, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn reference_requires_current_exact_roster_and_both_approved_members() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut engine = shared.lock().await;
        engine.create_workspace("Approved pair".into())?;
        let friend = Keypair::generate_ed25519().public().to_peer_id();
        let mut snapshot = engine.active_snapshot()?;
        snapshot.members.push(Member {
            peer_id: friend.to_string(),
            name: "Friend".into(),
        });
        snapshot.revision += 1;
        snapshot.sign(&engine.key)?;
        engine
            .persisted
            .workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        let request = OverlayRequest::new(&snapshot, None, 0);
        assert_eq!(
            authorize_reference(&mut engine, friend, &request)?,
            snapshot
        );
        let mut wrong_revision = request.clone();
        wrong_revision.revision -= 1;
        assert!(authorize_reference(&mut engine, friend, &wrong_revision).is_err());
        wrong_revision.revision = snapshot.revision + 1;
        assert!(authorize_reference(&mut engine, friend, &wrong_revision).is_err());
        let mut wrong_signature = request.clone();
        wrong_signature.roster_signature.push('A');
        assert!(authorize_reference(&mut engine, friend, &wrong_signature).is_err());
        let mut wrong_workspace = request.clone();
        wrong_workspace.workspace_id = Uuid::new_v4();
        assert!(authorize_reference(&mut engine, friend, &wrong_workspace).is_err());
        let outsider = Keypair::generate_ed25519().public().to_peer_id();
        assert!(authorize_reference(&mut engine, outsider, &request).is_err());
        let mut bad_offset = request.clone();
        bad_offset.offset = 257;
        assert!(authorize_reference(&mut engine, friend, &bad_offset).is_err());
        let last_offset = OverlayRequest::new(&snapshot, None, 256);
        authorize_reference(&mut engine, friend, &last_offset)?;
        assert_eq!(engine.active_snapshot()?, snapshot);
        let old_key = engine.key.clone();
        engine.key = Keypair::generate_ed25519();
        assert!(authorize_reference(&mut engine, friend, &request).is_err());
        engine.key = old_key;
        engine.persisted.active = None;
        assert!(authorize_reference(&mut engine, friend, &request).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn response_roundtrip_and_malformed_or_oversized_frames_are_bounded() -> Result<()> {
        let mut wire = Cursor::new(Vec::new());
        write(
            &mut wire,
            &OverlayResponse {
                total: 0,
                contacts: Vec::new(),
            },
        )
        .await?;
        wire.set_position(0);
        let response: OverlayResponse = read(&mut wire).await?;
        assert_eq!(response.total, 0);
        assert!(response.contacts.is_empty());
        for bytes in [
            0u32.to_be_bytes().to_vec(),
            8193u32.to_be_bytes().to_vec(),
            vec![0, 0, 0, 1, 0xff],
            vec![0, 0, 0, 4, 0xa0],
        ] {
            let result: Result<OverlayRequest> = read(&mut Cursor::new(bytes)).await;
            assert!(result.is_err());
        }
        let mut oversized = Cursor::new(Vec::new());
        assert!(write(&mut oversized, &vec![0u8; 8193]).await.is_err());
        assert!(oversized.into_inner().is_empty());
        let mut oversized_payload = Vec::new();
        ciborium::into_writer(&vec![0u8; 8190], &mut oversized_payload)?;
        assert_eq!(oversized_payload.len(), 8193);
        let mut oversized_wire = Cursor::new(8193u32.to_be_bytes().to_vec());
        oversized_wire
            .get_mut()
            .extend_from_slice(&oversized_payload);
        let result: Result<Vec<u8>> = read(&mut oversized_wire).await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn wire_rejects_unknown_fields_and_trailing_cbor() -> Result<()> {
        let owner = Keypair::generate_ed25519();
        let snapshot = full_roster(&owner)?;
        let mut value = serde_json::to_value(OverlayRequest::new(&snapshot, None, 0))?;
        value["unexpected"] = serde_json::json!(true);
        let mut wire = Cursor::new(Vec::new());
        // The generic codec permits encoding arbitrary values; the request schema
        // still rejects unrecognized fields within the byte limit.
        let mut encoded = Vec::new();
        ciborium::into_writer(&value, &mut encoded)?;
        wire.get_mut()
            .extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        wire.get_mut().extend_from_slice(&encoded);
        let result: Result<OverlayRequest> = read(&mut wire).await;
        assert!(result.is_err());

        let request = OverlayRequest::new(&snapshot, None, 0);
        let mut encoded = Vec::new();
        ciborium::into_writer(&request, &mut encoded)?;
        encoded.push(0);
        let mut trailing = Cursor::new((encoded.len() as u32).to_be_bytes().to_vec());
        trailing.get_mut().extend_from_slice(&encoded);
        let result: Result<OverlayRequest> = read(&mut trailing).await;
        assert!(result.is_err());
        Ok(())
    }
}
