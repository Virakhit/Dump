//! Only owner/member-signed overlay routes leave the device. LAN IPs use mDNS only.
use crate::model::{
    digest, now, public_key, random_secret, sign, verify, Invitation, Snapshot, VERSION,
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine as _};
use libp2p::{identity::Keypair, multiaddr::Protocol, Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};
use uuid::Uuid;
pub const LIFETIME: u64 = 86400;
pub const MAX_ROUTES: usize = 8;
pub const MAX_CACHE: usize = 1024;
pub const PREFIX: &str = "dump://connect/";
pub fn overlay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let a = ip.octets();
            a[0] == 100 && (64..=127).contains(&a[1])
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}
pub fn overlay_route(peer: PeerId, value: &str) -> Result<Multiaddr> {
    ensure!(value.len() <= 256, "Route is too long");
    let mut route: Multiaddr = value.parse()?;
    ensure!(
        route.pop() == Some(Protocol::P2p(peer)),
        "Route identity does not match"
    );
    let mut parts = route.iter();
    let ip = match parts.next() {
        Some(Protocol::Ip4(ip)) => IpAddr::V4(ip),
        Some(Protocol::Ip6(ip)) => IpAddr::V6(ip),
        _ => anyhow::bail!("Use a literal overlay IP"),
    };
    ensure!(
        overlay_ip(ip)
            && matches!(parts.next(), Some(Protocol::Udp(port)) if port != 0)
            && matches!(parts.next(), Some(Protocol::QuicV1))
            && parts.next().is_none(),
        "Only Tailscale QUIC routes are allowed"
    );
    Ok(route)
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteRecord {
    pub version: u8,
    pub workspace_id: Uuid,
    pub peer_id: String,
    pub public_key: String,
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub addresses: Vec<String>,
    pub signature: String,
}
impl RouteRecord {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/member-routes/1",
            self.version,
            self.workspace_id,
            &self.peer_id,
            &self.public_key,
            self.sequence,
            self.issued_at,
            self.expires_at,
            &self.addresses,
        )
    }
    pub fn new(
        key: &Keypair,
        workspace_id: Uuid,
        sequence: u64,
        mut addresses: Vec<String>,
    ) -> Result<Self> {
        addresses.sort();
        addresses.dedup();
        let issued_at = now();
        let mut record = Self {
            version: 1,
            workspace_id,
            peer_id: key.public().to_peer_id().to_string(),
            public_key: public_key(key),
            sequence,
            issued_at,
            expires_at: issued_at.saturating_add(LIFETIME),
            addresses,
            signature: String::new(),
        };
        let signature = sign(key, &record.payload())?;
        record.signature = signature;
        record.verify_signed()?;
        Ok(record)
    }
    pub fn key(&self) -> String {
        format!("{}:{}", self.workspace_id, self.peer_id)
    }
    pub fn verify_signed(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.sequence > 0
                && self.issued_at > 0
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= LIFETIME
                && self.addresses.len() <= MAX_ROUTES,
            "Invalid route record"
        );
        let peer: PeerId = self.peer_id.parse()?;
        let mut seen = BTreeSet::new();
        for address in &self.addresses {
            ensure!(
                seen.insert(overlay_route(peer, address)?.to_string()),
                "Duplicate route"
            );
        }
        verify(
            &self.public_key,
            &self.peer_id,
            &self.signature,
            &self.payload(),
        )
    }
    pub fn verify_at(&self, at: u64) -> Result<()> {
        self.verify_signed()?;
        ensure!(
            at < self.expires_at && self.issued_at <= at.saturating_add(300),
            "Invitation or route expired; ask for a fresh link"
        );
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    version: u8,
    pub invitation: Invitation,
    pub routes: RouteRecord,
    signature: String,
}
impl Envelope {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/invitation-envelope/2",
            self.version,
            self.invitation.version,
            self.invitation.workspace_id,
            &self.invitation.owner_peer_id,
            digest(self.invitation.token.as_bytes()),
            &self.routes,
        )
    }
    pub fn new(key: &Keypair, workspace: Uuid, routes: RouteRecord) -> Result<Self> {
        let mut envelope = Self {
            version: 2,
            invitation: Invitation {
                version: VERSION,
                workspace_id: workspace,
                owner_peer_id: key.public().to_peer_id().to_string(),
                token: random_secret(),
            },
            routes,
            signature: String::new(),
        };
        let signature = sign(key, &envelope.payload())?;
        envelope.signature = signature;
        envelope.verify()?;
        Ok(envelope)
    }
    fn verify(&self) -> Result<()> {
        ensure!(
            self.version == 2
                && self.invitation.workspace_id == self.routes.workspace_id
                && self.invitation.owner_peer_id == self.routes.peer_id,
            "Invitation owner or room mismatch"
        );
        Invitation::parse(&self.invitation.url()?)?;
        self.routes.verify_signed()?;
        verify(
            &self.routes.public_key,
            &self.invitation.owner_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
    pub fn url(&self) -> Result<String> {
        self.verify()?;
        let url = format!("{PREFIX}{}", B64.encode(serde_json::to_vec(self)?));
        ensure!(url.len() <= 8192, "Invitation is too long");
        Ok(url)
    }
}
pub struct JoinInput {
    pub invitation: Invitation,
    pub routes: Option<RouteRecord>,
}
impl JoinInput {
    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 8192, "Invitation is too long");
        if let Some(encoded) = value.trim().strip_prefix(PREFIX) {
            let envelope: Envelope = serde_json::from_slice(&B64.decode(encoded)?)?;
            envelope.verify()?;
            Ok(Self {
                invitation: envelope.invitation,
                routes: Some(envelope.routes),
            })
        } else {
            Ok(Self {
                invitation: Invitation::parse(value)?,
                routes: None,
            })
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedInvite {
    pub workspace_id: Uuid,
    pub expires_at: u64,
    pub approved: BTreeSet<String>,
    pub rejected: BTreeSet<String>,
}
pub fn validate_persisted(state: &crate::model::Persisted) -> Result<()> {
    ensure!(
        state.routes.len() <= MAX_CACHE && state.shared_invites.len() + state.invites.len() <= 128,
        "Invitation state exceeds limits"
    );
    for (key, record) in &state.routes {
        record.verify_signed()?;
        ensure!(
            *key == record.key() && state.workspaces.contains_key(&record.workspace_id),
            "Invalid cached workspace route"
        );
    }
    if let Some(record) = &state.joining_routes {
        record.verify_signed()?;
        ensure!(
            state
                .joining
                .as_ref()
                .is_some_and(|invite| invite.workspace_id == record.workspace_id
                    && invite.owner_peer_id == record.peer_id),
            "Invalid pending invitation route"
        );
    }
    let own_peer = state.peer_id()?;
    for (hash, record) in &state.shared_invites {
        ensure!(
            hash.len() == 64
                && hex::decode(hash)?.len() == 32
                && record.approved.len() <= crate::model::MAX_MEMBERS
                && record.rejected.len() <= crate::model::MAX_MEMBERS
                && record.approved.is_disjoint(&record.rejected),
            "Invalid shared invitation"
        );
        ensure!(
            state
                .workspaces
                .get(&record.workspace_id)
                .is_some_and(|workspace| workspace.snapshot.owner_peer_id == own_peer),
            "Shared invitation is not owned locally"
        );
        for peer in record.approved.iter().chain(record.rejected.iter()) {
            peer.parse::<PeerId>()?;
        }
    }
    Ok(())
}
pub fn merge_record(
    cache: &mut BTreeMap<String, RouteRecord>,
    record: RouteRecord,
    snapshot: &Snapshot,
) -> Result<bool> {
    record.verify_at(now())?;
    ensure!(
        record.workspace_id == snapshot.workspace_id && snapshot.contains(&record.peer_id),
        "Route is outside authorized membership"
    );
    let key = record.key();
    if let Some(old) = cache.get(&key) {
        if old.sequence > record.sequence {
            return Ok(false);
        }
        if old.sequence == record.sequence {
            ensure!(old == &record, "Conflicting signed route sequence");
            return Ok(false);
        }
    }
    ensure!(
        cache.contains_key(&key) || cache.len() < MAX_CACHE,
        "Route cache is full"
    );
    cache.insert(key, record);
    Ok(true)
}
