use super::*;
use crate::invite_routes::{overlay_route, RouteRecord};
const ROUTES: StreamProtocol = StreamProtocol::new("/dump/routes/1");
const PAGE: usize = 8;
pub(super) fn direct_candidate(peer: PeerId, mut address: Multiaddr) -> Option<Multiaddr> {
    if let Some(libp2p::multiaddr::Protocol::P2p(id)) = address.iter().last() {
        if id != peer {
            return None;
        }
        address.pop();
    }
    let mut parts = address.iter();
    let valid_ip = match parts.next()? {
        libp2p::multiaddr::Protocol::Ip4(ip) => {
            !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
        }
        libp2p::multiaddr::Protocol::Ip6(ip) => !ip.is_unspecified() && !ip.is_multicast(),
        _ => false,
    };
    if valid_ip
        && matches!(parts.next(), Some(libp2p::multiaddr::Protocol::Udp(port)) if port != 0)
        && matches!(parts.next(), Some(libp2p::multiaddr::Protocol::QuicV1))
        && parts.next().is_none()
    {
        Some(address)
    } else {
        None
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    snapshot: Snapshot,
    offset: usize,
    own: Option<RouteRecord>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Records {
    snapshot: Snapshot,
    total: usize,
    records: Vec<RouteRecord>,
}
pub(super) fn accept(control: &mut Control) -> Result<libp2p_stream::IncomingStreams> {
    Ok(control.accept(ROUTES)?)
}
fn local_addresses(node: &Node, ips: &[std::net::IpAddr]) -> Vec<String> {
    let mut routes = Vec::new();
    for address in node.addresses.borrow().iter() {
        let first = address.iter().next();
        let local = match first {
            Some(libp2p::multiaddr::Protocol::Ip4(ip)) => ips.contains(&std::net::IpAddr::V4(ip)),
            Some(libp2p::multiaddr::Protocol::Ip6(ip)) => ips.contains(&std::net::IpAddr::V6(ip)),
            _ => false,
        };
        if local {
            routes.push(address.clone());
        }
    }
    routes
        .into_iter()
        .filter_map(|route| {
            let peer = node.own_peer;
            let text = route
                .with(libp2p::multiaddr::Protocol::P2p(peer))
                .to_string();
            overlay_route(peer, &text).ok().map(|_| text)
        })
        .take(crate::invite_routes::MAX_ROUTES)
        .collect()
}
impl Node {
    pub async fn create_shared_invite(&self) -> Result<()> {
        let workspace = {
            let e = self.shared.lock().await;
            let snapshot = e.active_snapshot()?;
            ensure!(
                snapshot.owner_peer_id == e.peer(),
                "Only the owner can invite"
            );
            snapshot.workspace_id
        };
        let ips = if self.discovery {
            crate::tailscale::local_ips().await
        } else {
            Vec::new()
        };
        let addresses = local_addresses(self, &ips);
        ensure!(
            !self.addresses.borrow().is_empty(),
            "No active Dump listener. Wait for startup or restart Dump before inviting."
        );
        let note = if addresses.is_empty() {
            "Tailscale could not be confirmed on this device. This link can still use LAN discovery."
        } else {
            "This link can try LAN and Tailscale. Reachability is checked when each device joins."
        };
        let mut e = self.shared.lock().await;
        ensure!(
            e.persisted.active == Some(workspace),
            "Workspace changed; create the invitation again"
        );
        e.create_shared_invite(addresses)?;
        e.invite_note = Some(note.into());
        e.emit();
        Ok(())
    }
    pub async fn refresh_local_routes(&self) -> Result<()> {
        if !self.discovery {
            return Ok(());
        }
        let workspace = {
            let e = self.shared.lock().await;
            let snapshot = e.active_snapshot()?;
            ensure!(snapshot.contains(&e.peer()), "Not a member");
            snapshot.workspace_id
        };
        let ips = crate::tailscale::local_ips().await;
        let addresses = local_addresses(self, &ips);
        let mut e = self.shared.lock().await;
        ensure!(e.persisted.active == Some(workspace), "Workspace changed");
        e.update_local_routes(addresses)
    }
    pub async fn refresh_routes(&self, peer: PeerId) -> Result<()> {
        let (snapshot, own) = {
            let e = self.shared.lock().await;
            let snapshot = e.active_snapshot()?;
            ensure!(
                snapshot.contains(&e.peer()) && snapshot.contains(&peer.to_string()),
                "Access denied"
            );
            let own = e
                .persisted
                .routes
                .get(&format!("{}:{}", snapshot.workspace_id, e.peer()))
                .filter(|r| r.verify_at(now()).is_ok())
                .cloned();
            (snapshot, own)
        };
        let mut offset = 0;
        let mut total = None;
        loop {
            let current = self.shared.lock().await.active_snapshot()?;
            ensure!(
                current.workspace_id == snapshot.workspace_id
                    && current.contains(&peer.to_string())
                    && current.contains(&self.own_peer.to_string()),
                "Workspace or membership changed"
            );
            let mut control = self.control.clone();
            let mut stream =
                tokio::time::timeout(PREAUTH_TIMEOUT, control.open_stream(peer, ROUTES)).await??;
            write_frame(
                &mut stream,
                &Query {
                    snapshot: current,
                    offset,
                    own: if offset == 0 { own.clone() } else { None },
                },
            )
            .await?;
            let page: Records = read_frame(&mut stream).await?;
            ensure!(
                page.total <= MAX_MEMBERS
                    && page.records.len() <= PAGE
                    && offset + page.records.len() <= page.total,
                "Invalid route page"
            );
            if let Some(expected) = total {
                ensure!(expected == page.total, "Route set changed; retry later");
            } else {
                total = Some(page.total);
            }
            let mut e = self.shared.lock().await;
            ensure!(
                page.snapshot.workspace_id == snapshot.workspace_id,
                "Wrong route workspace"
            );
            let current = e.authorize(&peer.to_string(), &page.snapshot)?;
            let count = page.records.len();
            e.cache_routes(page.records, &current)?;
            offset += count;
            if offset == page.total {
                break;
            }
            ensure!(count > 0, "Incomplete route page");
            drop(e);
            tokio::time::sleep(Duration::from_millis(125)).await;
        }
        Ok(())
    }
}
pub(super) async fn serve(shared: Shared, peer: PeerId, mut stream: Stream) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let query: Query = read_frame(&mut stream).await?;
    let response = {
        let mut e = shared.lock().await;
        let snapshot = e.authorize(&peer.to_string(), &query.snapshot)?;
        ensure!(query.offset <= MAX_MEMBERS, "Invalid route offset");
        if let Some(record) = query.own {
            ensure!(record.peer_id == peer.to_string(), "Route sender mismatch");
            e.cache_routes(vec![record], &snapshot)?;
        }
        let all: Vec<_> = e
            .persisted
            .routes
            .values()
            .filter(|record| {
                record.workspace_id == snapshot.workspace_id
                    && snapshot.contains(&record.peer_id)
                    && record.verify_at(now()).is_ok()
            })
            .cloned()
            .collect();
        ensure!(query.offset <= all.len(), "Route set changed");
        Records {
            snapshot,
            total: all.len(),
            records: all.into_iter().skip(query.offset).take(PAGE).collect(),
        }
    };
    write_frame(&mut stream, &response).await?;
    stream.close().await?;
    Ok(())
}
