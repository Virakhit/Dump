use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use libp2p::identity::{Keypair, PublicKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const VERSION: u8 = 1;
pub const MAX_CONTROL: usize = 64 * 1024;
pub const MAX_MEMBERS: usize = 128;
pub const PAGE_SIZE: usize = 32;
pub const BLOCK: usize = 1024 * 1024;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn random_secret() -> String {
    let mut bytes = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    B64.encode(bytes)
}
pub fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
pub fn public_key(key: &Keypair) -> String {
    B64.encode(key.public().encode_protobuf())
}
pub fn decode_public(value: &str) -> Result<PublicKey> {
    ensure!(value.len() <= 128, "Invalid public key");
    Ok(PublicKey::try_decode_protobuf(&B64.decode(value)?)?)
}
fn encoded<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes)?;
    Ok(bytes)
}
fn sign<T: Serialize>(key: &Keypair, value: &T) -> Result<String> {
    Ok(B64.encode(key.sign(&encoded(value)?)?))
}
fn verify<T: Serialize>(public: &str, peer: &str, signature: &str, value: &T) -> Result<()> {
    let key = decode_public(public)?;
    ensure!(
        key.to_peer_id().to_string() == peer,
        "Identity does not match signing key"
    );
    ensure!(
        signature.len() <= 128 && key.verify(&encoded(value)?, &B64.decode(signature)?),
        "Invalid signature"
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub peer_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u8,
    pub workspace_id: Uuid,
    pub name: String,
    pub owner_peer_id: String,
    pub owner_public_key: String,
    pub revision: u64,
    pub members: Vec<Member>,
    pub signature: String,
}
impl Snapshot {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/membership/1",
            self.version,
            self.workspace_id,
            &self.name,
            &self.owner_peer_id,
            &self.owner_public_key,
            self.revision,
            &self.members,
        )
    }
    pub fn sign(&mut self, key: &Keypair) -> Result<()> {
        self.members.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        ensure!(
            key.public().to_peer_id().to_string() == self.owner_peer_id,
            "Only workspace owner can change membership"
        );
        let signature = sign(key, &self.payload())?;
        self.signature = signature;
        self.verify()
    }
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.version == VERSION && self.revision > 0,
            "Unsupported membership version"
        );
        validate_label(&self.name)?;
        ensure!(
            !self.members.is_empty() && self.members.len() <= MAX_MEMBERS,
            "Invalid member count"
        );
        let mut previous = None;
        for member in &self.members {
            validate_label(&member.name)?;
            member.peer_id.parse::<libp2p::PeerId>()?;
            ensure!(
                previous.is_none_or(|p: &str| p < member.peer_id.as_str()),
                "Duplicate or unsorted membership"
            );
            previous = Some(member.peer_id.as_str());
        }
        ensure!(
            self.contains(&self.owner_peer_id),
            "Workspace owner cannot be removed"
        );
        verify(
            &self.owner_public_key,
            &self.owner_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
    pub fn contains(&self, peer: &str) -> bool {
        self.members.iter().any(|m| m.peer_id == peer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub file_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub owner_peer_id: String,
    pub owner_public_key: String,
    pub signature: String,
}
impl Manifest {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/file/1",
            self.version,
            self.file_id,
            self.workspace_id,
            &self.name,
            self.size,
            &self.sha256,
            &self.owner_peer_id,
            &self.owner_public_key,
        )
    }
    pub fn sign(&mut self, key: &Keypair) -> Result<()> {
        let signature = sign(key, &self.payload())?;
        self.signature = signature;
        self.verify()
    }
    pub fn verify(&self) -> Result<()> {
        ensure!(self.version == VERSION, "Unsupported manifest version");
        validate_filename(&self.name)?;
        ensure!(
            self.sha256.len() == 64 && hex::decode(&self.sha256)?.len() == 32,
            "Invalid content hash"
        );
        verify(
            &self.owner_public_key,
            &self.owner_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub version: u8,
    pub workspace_id: Uuid,
    pub owner_peer_id: String,
    pub token: String,
}
impl Invitation {
    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 2048, "Invite is too long");
        let payload = value
            .trim()
            .strip_prefix("dump://join/")
            .context("Use a dump://join/ invitation")?;
        let invitation: Self = serde_json::from_slice(&B64.decode(payload)?)?;
        ensure!(
            invitation.version == VERSION && B64.decode(&invitation.token)?.len() == 32,
            "Invalid invite"
        );
        invitation.owner_peer_id.parse::<libp2p::PeerId>()?;
        Ok(invitation)
    }
    pub fn url(&self) -> Result<String> {
        Ok(format!(
            "dump://join/{}",
            B64.encode(serde_json::to_vec(self)?)
        ))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub snapshot: Snapshot,
    pub secret: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct InviteRecord {
    pub workspace_id: Uuid,
    pub token_hash: String,
    pub expires_at: u64,
    pub approved_peer: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LocalShare {
    pub manifest: Manifest,
    pub path: PathBuf,
    pub stamp: String,
    pub available: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Persisted {
    pub version: u8,
    pub identity: String,
    pub device_name: String,
    pub active: Option<Uuid>,
    pub workspaces: BTreeMap<Uuid, Workspace>,
    pub shares: BTreeMap<Uuid, LocalShare>,
    pub invites: Vec<InviteRecord>,
    pub joining: Option<Invitation>,
    pub partials: Vec<PathBuf>,
    pub history: Vec<AuditEvent>,
    #[serde(default)]
    pub network: crate::relay_host::Settings,
}
impl Persisted {
    pub fn new() -> Result<Self> {
        Ok(Self {
            version: VERSION,
            identity: B64.encode(Keypair::generate_ed25519().to_protobuf_encoding()?),
            device_name: "My device".into(),
            active: None,
            workspaces: BTreeMap::new(),
            shares: BTreeMap::new(),
            invites: Vec::new(),
            joining: None,
            partials: Vec::new(),
            history: Vec::new(),
            network: Default::default(),
        })
    }
    pub fn key(&self) -> Result<Keypair> {
        Ok(Keypair::from_protobuf_encoding(
            &B64.decode(&self.identity)?,
        )?)
    }
    pub fn peer_id(&self) -> Result<String> {
        Ok(self.key()?.public().to_peer_id().to_string())
    }
    pub fn audit(&mut self, kind: &str, workspace_id: Uuid, peer_id: &str) {
        self.history.push(AuditEvent {
            at: now(),
            kind: kind.into(),
            workspace_id,
            peer_id: peer_id.into(),
        });
        if self.history.len() > 200 {
            self.history.remove(0);
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: u64,
    pub kind: String,
    pub workspace_id: Uuid,
    pub peer_id: String,
}

pub fn validate_label(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 100 && !value.chars().any(char::is_control),
        "Use a name between 1 and 100 bytes without control characters"
    );
    Ok(())
}
pub fn validate_filename(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 255 && name != "." && name != "..",
        "Invalid filename"
    );
    ensure!(
        !name.ends_with(['.', ' '])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "Unsafe Windows filename"
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()) {
        bail!("Reserved Windows filename");
    }
    for prefix in ["COM", "LPT"] {
        if let Some(suffix) = stem.strip_prefix(prefix) {
            ensure!(
                !["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&suffix),
                "Reserved Windows filename"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_signatures_and_untrusted_names() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let mut manifest = Manifest {
            version: VERSION,
            file_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            name: "hello.txt".into(),
            size: 0,
            sha256: digest(b""),
            owner_peer_id: key.public().to_peer_id().to_string(),
            owner_public_key: public_key(&key),
            signature: String::new(),
        };
        manifest.sign(&key)?;
        manifest.verify()?;
        manifest.workspace_id = Uuid::new_v4();
        assert!(manifest.verify().is_err());
        for name in [
            "../secret",
            "C:\\secret",
            "x:ads",
            "CON.txt",
            "COM1",
            "LPT².txt",
            "NUL",
            "foo.",
            "foo ",
            "a\n",
            "",
        ] {
            assert!(validate_filename(name).is_err(), "{name}");
        }
        for name in ["file.zip", "รายงาน.txt", ".env", "COM10.txt"] {
            validate_filename(name)?;
        }
        Ok(())
    }
}
