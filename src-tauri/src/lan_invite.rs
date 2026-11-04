use crate::model::{Invitation, InviteRecord, VERSION};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use libp2p::PeerId;
use sha2::{Digest, Sha256};

pub const PREFIX: &str = "dump://lan/";
pub const LIFETIME: u64 = 600;

/// Kept only in memory: a LAN invitation cannot be resumed after a restart or
/// presented to the ordinary Internet-capable join protocol.
#[derive(Clone)]
pub struct LanInvite {
    pub invitation: Invitation,
    pub record: InviteRecord,
}

#[derive(Clone)]
pub struct LanJoin {
    pub code: String,
    pub expires_at: u64,
    pub owner_peer_id: Option<String>,
}

/// The link commits to the complete invitation, including its owner's identity.
/// A different discovered peer cannot replace it with an invitation of its own.
pub fn code(invitation: &Invitation) -> Result<String> {
    ensure!(
        invitation.version == VERSION && invitation.contact.is_none(),
        "Invalid LAN invitation"
    );
    let token = B64.decode(&invitation.token)?;
    ensure!(token.len() == 32, "Invalid invitation token");
    let peer: PeerId = invitation.owner_peer_id.parse()?;
    let mut hash = Sha256::new();
    hash.update(b"dump/lan-invite/1\0");
    hash.update(invitation.workspace_id.as_bytes());
    hash.update(token);
    hash.update(peer.to_bytes());
    Ok(B64.encode(&hash.finalize()[..12]))
}

pub fn parse_url(value: &str) -> Result<String> {
    ensure!(value.len() <= 8192, "Invite is too long");
    let code = value
        .trim()
        .strip_prefix(PREFIX)
        .context("Use a dump://lan/ invitation")?;
    ensure!(
        code.len() == 16 && B64.decode(code)?.len() == 12,
        "Invalid LAN invitation"
    );
    Ok(code.into())
}

pub fn verify(invitation: &Invitation, expected_code: &str, peer: PeerId) -> Result<()> {
    ensure!(
        invitation.owner_peer_id == peer.to_string() && code(invitation)? == expected_code,
        "LAN invitation does not match its owner or link"
    );
    Ok(())
}
