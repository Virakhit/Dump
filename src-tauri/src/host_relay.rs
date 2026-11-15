//! One-purpose host grants and bounded opaque-byte forwarding. No file cache.
use crate::model::{now, sign, verify, Manifest, Snapshot, MAX_CONTROL};
use anyhow::{ensure, Result};
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const BUFFER_BYTES: usize = 64 * 1024;
pub const BANDWIDTH: u64 = 16 * 1024 * 1024;
pub const HANDSHAKE_SECONDS: u64 = 15;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Purpose {
    Catalog { offset: u32 },
    File { manifest: Manifest },
}
impl Purpose {
    pub fn budget(&self) -> Result<u64> {
        let size = match self {
            Self::Catalog { offset } => {
                ensure!(*offset <= 10000, "Invalid catalog offset");
                0
            }
            Self::File { manifest } => {
                manifest.verify()?;
                ensure!(
                    manifest.size <= MAX_FILE_BYTES,
                    "Host relay supports files up to 8 GiB; larger files need a direct connection"
                );
                manifest.size
            }
        };
        Ok(size + size / 100 + 512 * 1024 + 2 * MAX_CONTROL as u64)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub version: u8,
    pub nonce: Uuid,
    pub generation: Uuid,
    pub issued_at: u64,
    pub expires_at: u64,
    pub snapshot: Snapshot,
    pub receiver: String,
    pub source: String,
    pub purpose: Purpose,
    pub signature: String,
}
impl Grant {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/host-relay/grant/1",
            self.version,
            self.nonce,
            self.generation,
            self.issued_at,
            self.expires_at,
            &self.snapshot,
            &self.receiver,
            &self.source,
            &self.purpose,
        )
    }
    pub fn new(
        key: &Keypair,
        snapshot: Snapshot,
        receiver: String,
        source: String,
        purpose: Purpose,
        generation: Uuid,
    ) -> Result<Self> {
        ensure!(
            key.public().to_peer_id().to_string() == snapshot.owner_peer_id,
            "Only the workspace owner can grant a bridge"
        );
        let issued_at = now();
        let mut grant = Self {
            version: 1,
            nonce: Uuid::new_v4(),
            generation,
            issued_at,
            expires_at: issued_at.saturating_add(HANDSHAKE_SECONDS),
            snapshot,
            receiver,
            source,
            purpose,
            signature: String::new(),
        };
        let signature = sign(key, &grant.payload())?;
        grant.signature = signature;
        grant.verify_at(issued_at)?;
        Ok(grant)
    }
    pub fn verify_at(&self, at: u64) -> Result<()> {
        ensure!(
            self.version == 1
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= HANDSHAKE_SECONDS
                && at < self.expires_at
                && self.issued_at <= at.saturating_add(300),
            "Relay grant expired or invalid"
        );
        self.snapshot.verify()?;
        ensure!(
            self.receiver != self.source
                && self.receiver != self.snapshot.owner_peer_id
                && self.source != self.snapshot.owner_peer_id
                && self.snapshot.contains(&self.receiver)
                && self.snapshot.contains(&self.source),
            "Invalid relay endpoints"
        );
        if let Purpose::File { manifest } = &self.purpose {
            ensure!(
                manifest.workspace_id == self.snapshot.workspace_id
                    && manifest.owner_peer_id == self.source,
                "File identity is outside this grant"
            );
        }
        self.purpose.budget()?;
        verify(
            &self.snapshot.owner_public_key,
            &self.snapshot.owner_peer_id,
            &self.signature,
            &self.payload(),
        )
    }
    pub fn authorize(&self, current: &Snapshot, receiver: &str, generation: Uuid) -> Result<()> {
        self.verify_at(now())?;
        ensure!(
            current == &self.snapshot && self.receiver == receiver && self.generation == generation,
            "Relay workspace, device or generation changed"
        );
        Ok(())
    }
    pub fn authorize_request(&self, request: &crate::network::Request) -> Result<()> {
        use crate::network::Request;
        match (&self.purpose, request) {
            (Purpose::Catalog { offset: expected }, Request::Catalog { snapshot, offset }) => {
                ensure!(
                    snapshot == &self.snapshot && offset == expected,
                    "Catalog request differs from its grant"
                )
            }
            (Purpose::File { manifest: expected }, Request::File { snapshot, manifest }) => {
                ensure!(
                    snapshot == &self.snapshot && manifest == expected,
                    "File request differs from its grant"
                )
            }
            _ => anyhow::bail!("Relay grant cannot carry this protocol request"),
        }
        Ok(())
    }
}

pub struct Bandwidth {
    next: tokio::sync::Mutex<Instant>,
}
impl Default for Bandwidth {
    fn default() -> Self {
        Self {
            next: tokio::sync::Mutex::new(Instant::now()),
        }
    }
}
impl Bandwidth {
    async fn charge(&self, bytes: usize) {
        let mut next = self.next.lock().await;
        let start = (*next).max(Instant::now());
        *next = start
            + Duration::from_nanos(((bytes as u128 * 1_000_000_000) / BANDWIDTH as u128) as u64);
        drop(next);
        tokio::time::sleep_until(start.into()).await;
    }
}
/// One fixed buffer per direction; a slow writer prevents the next upstream read.
pub async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    remaining: Arc<AtomicU64>,
    bandwidth: Arc<Bandwidth>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut bytes = vec![0; BUFFER_BYTES];
    loop {
        let count = tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Host relay cancelled"), result = tokio::time::timeout(Duration::from_secs(30), reader.read(&mut bytes)) => result?? };
        if count == 0 {
            tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Host relay cancelled"), result = tokio::time::timeout(Duration::from_secs(30), writer.close()) => result?? }
            return Ok(());
        }
        ensure!(
            remaining
                .try_update(Ordering::AcqRel, Ordering::Acquire, |old| old
                    .checked_sub(count as u64))
                .is_ok(),
            "Host relay byte budget exceeded"
        );
        tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Host relay cancelled"), _ = bandwidth.charge(count) => {} }
        tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Host relay cancelled"), result = tokio::time::timeout(Duration::from_secs(30), writer.write_all(&bytes[..count])) => result?? }
    }
}
