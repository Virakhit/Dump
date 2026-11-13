use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr};

pub const PORT: u16 = 42043;
const MAX_STATUS_BYTES: usize = 2 * 1024 * 1024;
const MAX_PEERS: usize = 4096;
const MAX_NODE_IPS: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailscaleNode {
    pub id: String,
    pub dns_name: String,
    pub ips: Vec<IpAddr>,
    pub online: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailscaleSnapshot {
    pub state: TailscaleState,
    pub self_node: Option<TailscaleNode>,
    pub peers: Vec<TailscaleNode>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TailscaleState {
    #[default]
    NotInstalled,
    ServiceUnavailable,
    NeedsLogin,
    Ready,
    Invalid,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CliStatus {
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<CliNode>,
    peer: Option<BTreeMap<String, CliNode>>,
}

#[derive(Deserialize)]
struct CliNode {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "DNSName")]
    dns_name: String,
    #[serde(rename = "TailscaleIPs")]
    ips: Vec<IpAddr>,
    #[serde(rename = "Online")]
    online: bool,
}

impl TryFrom<CliNode> for TailscaleNode {
    type Error = anyhow::Error;

    fn try_from(node: CliNode) -> Result<Self> {
        let node = Self {
            id: node.id,
            dns_name: canonical_dns_name(&node.dns_name)?,
            ips: node.ips,
            online: node.online,
        };
        validate_node(&node)?;
        Ok(node)
    }
}

/// Parse only routing information. Unknown CLI fields are intentionally ignored;
/// full status documents and user metadata must never be logged.
pub fn parse_status(bytes: &[u8]) -> Result<TailscaleSnapshot> {
    ensure!(
        bytes.len() <= MAX_STATUS_BYTES,
        "Tailscale status is too large"
    );
    let raw: CliStatus = serde_json::from_slice(bytes).context("Invalid Tailscale status JSON")?;
    let state = match raw.backend_state.as_str() {
        "Running" => TailscaleState::Ready,
        "NeedsLogin" | "NeedsMachineAuth" => TailscaleState::NeedsLogin,
        "NoState" | "Starting" | "Stopped" | "InUseOtherUser" => TailscaleState::ServiceUnavailable,
        _ => TailscaleState::Invalid,
    };
    if state != TailscaleState::Ready {
        return Ok(TailscaleSnapshot {
            state,
            ..Default::default()
        });
    }
    let self_node = TailscaleNode::try_from(raw.self_node.context("Missing Tailscale self node")?)?;
    ensure!(self_node.online, "Tailscale self node is offline");
    let raw_peers = raw.peer.unwrap_or_default();
    ensure!(raw_peers.len() <= MAX_PEERS, "Too many Tailscale peers");
    let peers = raw_peers
        .into_values()
        .map(TailscaleNode::try_from)
        .collect::<Result<Vec<_>>>()?;
    let snapshot = TailscaleSnapshot {
        state,
        self_node: Some(self_node),
        peers,
    };
    snapshot.validate_peer_map()?;
    Ok(snapshot)
}

impl TailscaleSnapshot {
    /// Resolve the signed node locator against this receiver's local peer map.
    /// Sender-provided literal IPs and DNS lookups are never routing authority.
    pub fn resolve(&self, node_id: &str, dns_name: &str) -> Result<Vec<IpAddr>> {
        self.ready_node()?;
        validate_node_id(node_id)?;
        let dns_name = canonical_dns_name(dns_name)?;
        self.validate_peer_map()?;
        let node = self.peers.iter().find(|node| node.id == node_id).context(
            "Tailscale peer is not visible locally; check sharing or refresh the invitation",
        )?;
        ensure!(
            canonical_dns_name(&node.dns_name)? == dns_name,
            "Tailscale peer name does not match its node ID"
        );
        ensure!(node.online, "Tailscale peer is offline");
        Ok(node.ips.clone())
    }

    pub fn ready_node(&self) -> Result<&TailscaleNode> {
        ensure!(
            self.state == TailscaleState::Ready,
            "Tailscale is not ready"
        );
        let node = self
            .self_node
            .as_ref()
            .context("Missing Tailscale self node")?;
        validate_node(node)?;
        ensure!(node.online, "Tailscale self node is offline");
        Ok(node)
    }

    fn validate_peer_map(&self) -> Result<()> {
        ensure!(self.peers.len() <= MAX_PEERS, "Too many Tailscale peers");
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        let mut addresses = std::collections::HashSet::new();
        for node in self.self_node.iter().chain(self.peers.iter()) {
            validate_node(node)?;
            ensure!(ids.insert(node.id.as_str()), "Ambiguous Tailscale node ID");
            ensure!(
                names.insert(canonical_dns_name(&node.dns_name)?),
                "Ambiguous Tailscale DNS name"
            );
            for ip in &node.ips {
                ensure!(addresses.insert(*ip), "Ambiguous Tailscale IP address");
            }
        }
        Ok(())
    }
}

pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            a == 100
                && (64..=127).contains(&b)
                && !(b == 100 && (c == 0 || c == 100))
                && !(b == 115 && (c == 92 || c == 93))
                && [a, b, c, d] != [100, 101, 102, 103]
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            segments[..3] == [0xfd7a, 0x115c, 0xa1e0]
                && segments != [0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0, 0x53]
        }
    }
}

fn validate_node_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 256
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':')),
        "Invalid Tailscale node ID"
    );
    Ok(())
}

fn canonical_dns_name(name: &str) -> Result<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    ensure!(
        !name.is_empty() && name.len() <= 253 && name.is_ascii(),
        "Invalid Tailscale FQDN"
    );
    ensure!(
        name.parse::<IpAddr>().is_err(),
        "Tailscale FQDN cannot be an IP address"
    );
    let labels: Vec<_> = name.split('.').collect();
    ensure!(labels.len() >= 2, "Tailscale name must be a full DNS name");
    for label in &labels {
        ensure!(
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "Invalid Tailscale FQDN label"
        );
    }
    ensure!(
        labels
            .last()
            .is_some_and(|label| label.bytes().any(|c| c.is_ascii_alphabetic())),
        "Invalid Tailscale FQDN suffix"
    );
    Ok(name.to_ascii_lowercase())
}

fn validate_node(node: &TailscaleNode) -> Result<()> {
    validate_node_id(&node.id)?;
    canonical_dns_name(&node.dns_name)?;
    ensure!(
        !node.ips.is_empty() && node.ips.len() <= MAX_NODE_IPS,
        "Invalid Tailscale address count"
    );
    ensure!(
        node.ips.iter().copied().all(is_tailscale_ip),
        "Unsafe Tailscale node address"
    );
    Ok(())
}

#[cfg(any(windows, test))]
async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Tailscale output limit exceeded",
        ));
    }
    Ok(bytes)
}

/// Only the installed Windows CLI is supported. No shell, PATH lookup, DNS
/// lookup, login or mutation of Tailscale configuration occurs here.
pub async fn inspect() -> TailscaleSnapshot {
    #[cfg(windows)]
    {
        static INSPECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let Ok(_guard) = INSPECT_LOCK.try_lock() else {
            return snapshot_with_state(TailscaleState::ServiceUnavailable);
        };
        let Some(path) = trusted_cli_path() else {
            return snapshot_with_state(TailscaleState::NotInstalled);
        };
        match tokio::time::timeout(std::time::Duration::from_secs(3), read_cli_status(&path)).await
        {
            Ok(Ok((success, stdout))) => match parse_status(&stdout) {
                Ok(snapshot) if success || snapshot.state == TailscaleState::NeedsLogin => snapshot,
                Ok(_) => snapshot_with_state(TailscaleState::ServiceUnavailable),
                Err(_) if !success => snapshot_with_state(TailscaleState::ServiceUnavailable),
                Err(_) => snapshot_with_state(TailscaleState::Invalid),
            },
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => {
                snapshot_with_state(TailscaleState::Invalid)
            }
            _ => snapshot_with_state(TailscaleState::ServiceUnavailable),
        }
    }
    #[cfg(not(windows))]
    snapshot_with_state(TailscaleState::NotInstalled)
}

fn snapshot_with_state(state: TailscaleState) -> TailscaleSnapshot {
    TailscaleSnapshot {
        state,
        ..Default::default()
    }
}

#[cfg(windows)]
fn trusted_cli_path() -> Option<std::path::PathBuf> {
    let program_files = std::path::PathBuf::from(std::env::var_os("ProgramFiles")?);
    if !program_files.is_absolute() {
        return None;
    }
    let path = program_files.join("Tailscale").join("tailscale.exe");
    path.is_file().then_some(path)
}

#[cfg(windows)]
async fn read_cli_status(path: &std::path::Path) -> std::io::Result<(bool, Vec<u8>)> {
    use std::process::Stdio;
    let mut command = tokio::process::Command::new(path);
    command
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("Missing CLI stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("Missing CLI stderr"))?;
    let (stdout, _, status) = tokio::try_join!(
        read_bounded(stdout, MAX_STATUS_BYTES),
        read_bounded(stderr, 64 * 1024),
        child.wait()
    )?;
    Ok((status.success(), stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn status() -> Value {
        json!({
            "BackendState": "Running",
            "Self": {"ID":"self-id", "DNSName":"local.tail123.ts.net.",
                "TailscaleIPs":["100.80.1.1", "fd7a:115c:a1e0::1"], "Online":true},
            "Peer": {"nodekey:peer": {"ID":"peer-id", "DNSName":"remote.tail456.ts.net.",
                "TailscaleIPs":["100.90.2.3", "fd7a:115c:a1e0::2"], "Online":true}}
        })
    }

    fn parse(value: Value) -> Result<TailscaleSnapshot> {
        parse_status(&serde_json::to_vec(&value).unwrap())
    }

    #[test]
    fn running_requires_valid_local_node_and_retains_only_routing_fields() {
        let mut value = status();
        value["FutureSchema"] = json!({"anything":true});
        value["Self"]["UserID"] = json!(42);
        let snapshot = parse(value).unwrap();
        assert_eq!(snapshot.state, TailscaleState::Ready);
        assert_eq!(
            snapshot.ready_node().unwrap().dns_name,
            "local.tail123.ts.net"
        );
        assert_eq!(snapshot.peers.len(), 1);
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("UserID"));
    }

    #[test]
    fn nonrunning_states_never_expose_routes() {
        for (backend, expected) in [
            ("NeedsLogin", TailscaleState::NeedsLogin),
            ("NeedsMachineAuth", TailscaleState::NeedsLogin),
            ("Stopped", TailscaleState::ServiceUnavailable),
            ("Starting", TailscaleState::ServiceUnavailable),
            ("NoState", TailscaleState::ServiceUnavailable),
            ("FutureState", TailscaleState::Invalid),
        ] {
            let mut value = status();
            value["BackendState"] = json!(backend);
            let snapshot = parse(value).unwrap();
            assert_eq!(snapshot.state, expected);
            assert!(snapshot.ready_node().is_err());
            assert!(snapshot.peers.is_empty());
            assert!(snapshot.self_node.is_none());
        }
    }

    #[test]
    fn resolver_uses_receiver_remapped_ip_and_exact_node_fqdn_binding() {
        let snapshot = parse(status()).unwrap();
        assert_eq!(
            snapshot
                .resolve("peer-id", "REMOTE.TAIL456.TS.NET.")
                .unwrap(),
            vec![
                "100.90.2.3".parse::<IpAddr>().unwrap(),
                "fd7a:115c:a1e0::2".parse().unwrap()
            ]
        );
        for (id, name) in [
            ("wrong-id", "remote.tail456.ts.net"),
            ("peer-id", "wrong.tail.ts.net"),
            ("peer-id", "100.90.2.3"),
            ("peer-id", "remote"),
            ("peer-id", "remote.tail456.ts.net.."),
            ("peer-id", " remote.tail456.ts.net"),
        ] {
            assert!(snapshot.resolve(id, name).is_err(), "accepted {id} {name}");
        }
    }

    #[test]
    fn resolver_rejects_offline_duplicate_or_tampered_nodes() {
        let snapshot = parse(status()).unwrap();
        let mut offline = snapshot.clone();
        offline.peers[0].online = false;
        assert!(offline.resolve("peer-id", "remote.tail456.ts.net").is_err());
        let mut duplicate = snapshot.clone();
        duplicate.peers.push(duplicate.peers[0].clone());
        assert!(duplicate
            .resolve("peer-id", "remote.tail456.ts.net")
            .is_err());
        let mut tampered = snapshot.clone();
        tampered.peers[0].ips.push("127.0.0.1".parse().unwrap());
        assert!(tampered
            .resolve("peer-id", "remote.tail456.ts.net")
            .is_err());
        let mut not_ready = snapshot;
        not_ready.state = TailscaleState::NeedsLogin;
        assert!(not_ready
            .resolve("peer-id", "remote.tail456.ts.net")
            .is_err());
    }

    #[test]
    fn rejects_invalid_missing_truncated_and_oversized_status() {
        assert!(parse_status(b"{").is_err());
        assert!(parse_status(&vec![b' '; 2 * 1024 * 1024 + 1]).is_err());
        for invalid in [
            json!({}),
            json!({"BackendState":17}),
            json!({"BackendState":"Running"}),
            json!({"BackendState":"Running", "Self":null}),
            json!([]),
        ] {
            assert!(parse(invalid).is_err());
        }
        for (field, bad) in [
            ("ID", json!("")),
            ("ID", json!("x".repeat(257))),
            ("DNSName", json!("localhost")),
            ("Online", json!("true")),
            ("TailscaleIPs", json!([])),
            ("TailscaleIPs", json!(["not-an-ip"])),
        ] {
            let mut value = status();
            value["Self"][field] = bad;
            assert!(parse(value).is_err(), "accepted invalid {field}");
        }
    }

    #[test]
    fn rejects_peer_map_with_untrusted_ips_or_ambiguous_identity() {
        for address in [
            "127.0.0.1",
            "192.168.1.1",
            "8.8.8.8",
            "::1",
            "fd00::1",
            "100.100.100.100",
            "fd7a:115c:a1e0::53",
        ] {
            let mut value = status();
            value["Peer"]["nodekey:peer"]["TailscaleIPs"] = json!([address]);
            assert!(parse(value).is_err(), "accepted {address}");
        }
        let mut duplicate = status();
        duplicate["Peer"]["nodekey:second"] = duplicate["Peer"]["nodekey:peer"].clone();
        assert!(parse(duplicate).is_err());
        let mut duplicate_name = status();
        duplicate_name["Peer"]["nodekey:second"] = json!({"ID":"other-id",
            "DNSName":"REMOTE.TAIL456.TS.NET", "TailscaleIPs":["100.90.2.4"], "Online":true});
        assert!(parse(duplicate_name).is_err());
    }

    #[test]
    fn tailscale_address_ranges_exclude_internal_services() {
        for address in ["100.64.0.1", "100.127.255.254", "fd7a:115c:a1e0::1"] {
            assert!(
                is_tailscale_ip(address.parse().unwrap()),
                "rejected {address}"
            );
        }
        for address in [
            "100.63.255.255",
            "100.128.0.0",
            "100.100.0.1",
            "100.100.100.100",
            "100.115.92.1",
            "100.115.93.254",
            "100.101.102.103",
            "fd7a:115c:a1e0::53",
            "fd7a:115c:a1e1::1",
            "::ffff:100.80.1.1",
            "0.0.0.0",
        ] {
            assert!(
                !is_tailscale_ip(address.parse().unwrap()),
                "accepted {address}"
            );
        }
    }

    #[tokio::test]
    async fn bounded_output_reader_rejects_extra_byte_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(32);
        use tokio::io::AsyncWriteExt;
        writer.write_all(b"123456789").await.unwrap();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), read_bounded(reader, 8))
                .await
                .unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn bounded_output_reader_accepts_exact_limit() {
        let (mut writer, reader) = tokio::io::duplex(32);
        use tokio::io::AsyncWriteExt;
        writer.write_all(b"12345678").await.unwrap();
        writer.shutdown().await.unwrap();
        assert_eq!(read_bounded(reader, 8).await.unwrap(), b"12345678");
    }
}
