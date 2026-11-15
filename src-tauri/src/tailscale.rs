//! Read-only local daemon query. Never login, configure, scan peers or change ACLs.
use crate::invite_routes::{overlay_ip, MAX_ROUTES};
use serde_json::Value;
use std::{net::IpAddr, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;
pub fn parse_status(bytes: &[u8]) -> Vec<IpAddr> {
    let Ok(status) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    if status["BackendState"] != "Running" || status["Self"]["Online"] != true {
        return Vec::new();
    }
    let Some(ips) = status["Self"]["TailscaleIPs"].as_array() else {
        return Vec::new();
    };
    let mut result: Vec<IpAddr> = ips
        .iter()
        .filter_map(|value| value.as_str()?.parse().ok())
        .filter(|ip| overlay_ip(*ip))
        .collect();
    result.sort();
    result.dedup();
    result.truncate(MAX_ROUTES);
    result
}
pub async fn local_ips() -> Vec<IpAddr> {
    let Some(program_files) = std::env::var_os("ProgramFiles") else {
        return Vec::new();
    };
    let path = PathBuf::from(program_files)
        .join("Tailscale")
        .join("tailscale.exe");
    let mut command = tokio::process::Command::new(path);
    command
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let Ok(mut child) = command.spawn() else {
        return Vec::new();
    };
    let Some(stdout) = child.stdout.take() else {
        return Vec::new();
    };
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 1024 * 1024 || !child.wait().await.ok()?.success() {
            return None;
        }
        Some(parse_status(&bytes))
    })
    .await;
    result.ok().flatten().unwrap_or_default()
}
