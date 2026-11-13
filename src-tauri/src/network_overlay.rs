use super::*;
use crate::{
    overlay::WorkspaceNetworkPolicy,
    overlay_routes::{OverlayScope, SharedOverlayScope},
};
use libp2p::{
    core::{transport::ListenerId, ConnectedPoint},
    Swarm,
};
use std::net::IpAddr;
#[path = "overlay_exchange.rs"]
mod exchange;
use exchange::{
    read as read_overlay_frame, write as write_overlay_frame, OverlayRequest, OverlayResponse,
};

const OVERLAY_CONTACT: StreamProtocol = StreamProtocol::new("/dump/overlay-contacts/1");
const OVERLAY_LISTENER_ERROR:&str="Tailscale listener could not open TCP and UDP port 42043. Check port availability, then check Tailscale again.";

pub(super) fn active_overlay(e: &crate::engine::Engine) -> bool {
    e.persisted
        .active
        .and_then(|id| e.persisted.workspace_network.get(&id))
        .is_some_and(|policy| *policy == WorkspaceNetworkPolicy::TailscaleOnly)
}

#[derive(Default)]
pub(super) struct OverlayListeners {
    ids: BTreeMap<Multiaddr, ListenerId>,
    bound_ips: BTreeSet<IpAddr>,
    failed: bool,
    generation: u64,
    context_epoch: u64,
    retry_at: Option<Instant>,
}
pub(super) type OverlayConnections = BTreeMap<ConnectionId, (PeerId, ConnectedPoint)>;

pub(super) fn endpoint(scope: &SharedOverlayScope, endpoint: &ConnectedPoint) -> bool {
    if endpoint.is_relayed() {
        return false;
    }
    let local = match endpoint {
        ConnectedPoint::Listener { local_addr, .. } => Some(local_addr),
        _ => None,
    };
    scope
        .read()
        .is_ok_and(|scope| scope.contains(local, endpoint.get_remote_address()))
}

pub(super) fn route_state(
    connections: &BTreeMap<ConnectionId, (PeerId, bool)>,
    overlay: &OverlayConnections,
    peer: PeerId,
    strict: bool,
) -> ConnectionState {
    if strict {
        if overlay.values().any(|(id, _)| *id == peer) {
            ConnectionState::Direct
        } else {
            ConnectionState::Offline
        }
    } else {
        connection_state(
            &connections
                .iter()
                .filter(|(id, _)| !overlay.contains_key(id))
                .map(|(id, value)| (*id, *value))
                .collect(),
            peer,
        )
    }
}

pub(super) fn listener(ip: IpAddr, tcp: bool) -> Multiaddr {
    let first = match ip {
        IpAddr::V4(ip) => libp2p::multiaddr::Protocol::Ip4(ip),
        IpAddr::V6(ip) => libp2p::multiaddr::Protocol::Ip6(ip),
    };
    let route = Multiaddr::empty().with(first);
    if tcp {
        route.with(libp2p::multiaddr::Protocol::Tcp(42043))
    } else {
        route
            .with(libp2p::multiaddr::Protocol::Udp(42043))
            .with(libp2p::multiaddr::Protocol::QuicV1)
    }
}

impl Node {
    pub async fn refresh_tailscale(&self) -> Result<()> {
        let Ok(_guard) = self.tailscale_refresh.try_lock() else {
            return Ok(());
        };
        let enabled = self.shared.lock().await.persisted.network.use_tailscale;
        let snapshot = crate::tailscale::inspect().await;
        let mut e = self.shared.lock().await;
        // A user may disable the adapter while inspection was in progress.
        if enabled == e.persisted.network.use_tailscale {
            e.tailscale = snapshot;
            e.tailscale_generation = e.tailscale_generation.wrapping_add(1);
            e.emit();
        }
        Ok(())
    }

    pub(super) fn overlay_routes(&self, e: &crate::engine::Engine, peer: PeerId) -> Vec<Multiaddr> {
        if !e.persisted.network.use_tailscale || !e.overlay_listener_ready {
            return Vec::new();
        }
        let contact = if let Some(contact) = e
            .persisted
            .overlay_joining
            .as_ref()
            .filter(|c| c.peer_id == peer.to_string())
        {
            Some(contact)
        } else {
            e.persisted.active.and_then(|workspace| {
                e.persisted
                    .overlay_contacts
                    .get(&format!("{workspace}:{peer}"))
            })
        };
        let Some(contact) = contact.filter(|c| c.verify().is_ok()) else {
            return Vec::new();
        };
        let Ok(ips) = e.tailscale.resolve(&contact.node_id, &contact.dns_name) else {
            return Vec::new();
        };
        #[cfg(test)]
        let ips = if let Some(config) = &self.overlay_test {
            config
                .remotes
                .get(&contact.node_id)
                .copied()
                .into_iter()
                .collect()
        } else {
            ips
        };
        ips.into_iter()
            .flat_map(|ip| [listener(ip, false), listener(ip, true)])
            .take(MAX_ADDRESSES)
            .collect()
    }

    pub(super) fn sync_overlay(
        &self,
        e: &mut crate::engine::Engine,
        swarm: &mut Swarm<Behaviour>,
        listeners: &[Multiaddr],
        state: &mut OverlayListeners,
        connections: &mut OverlayConnections,
        all_connections: &BTreeMap<ConnectionId, (PeerId, bool)>,
    ) {
        if e.persisted
            .overlay_joining
            .as_ref()
            .is_some_and(|contact| contact.verify().is_err())
        {
            let _ = e.cancel_join();
            e.error("Tailscale invitation expired or is no longer valid. Ask for a fresh link.");
        }
        let own = e
            .tailscale
            .ready_node()
            .ok()
            .filter(|_| e.persisted.network.use_tailscale);
        let ready = own.is_some();
        let ips: BTreeSet<_> = own
            .into_iter()
            .flat_map(|node| node.ips.iter().copied())
            .collect();
        #[cfg(test)]
        let ips = if let Some(config) = &self.overlay_test {
            if ips.is_empty() {
                BTreeSet::new()
            } else {
                BTreeSet::from([config.listen])
            }
        } else {
            ips
        };
        // A temporary status failure suspends all application traffic below,
        // but keeps the owned listener sockets. Quinn connections may retain
        // their UDP socket while draining, so removing/rebinding it on login
        // races that drain. Disable or a newly validated IP change retires it.
        if ready || !e.persisted.network.use_tailscale {
            state.bound_ips = ips;
        }
        let ips = state.bound_ips.clone();
        let remote: BTreeSet<_> = e
            .tailscale
            .peers
            .iter()
            .filter(|node| node.online)
            .flat_map(|node| node.ips.iter().copied())
            .collect();
        #[cfg(test)]
        let remote = if let Some(config) = &self.overlay_test {
            if !ready {
                BTreeSet::new()
            } else {
                config
                    .remotes
                    .values()
                    .copied()
                    .chain(["127.0.0.1".parse().unwrap()])
                    .collect()
            }
        } else {
            remote
        };
        if let Ok(mut scope) = self.overlay_scope.write() {
            // A handshake started before status loss must keep its transport
            // classification. Readiness checks still forbid every operation.
            let remote = if !ready && e.persisted.network.use_tailscale {
                scope.remote.clone()
            } else {
                remote
            };
            *scope = OverlayScope {
                local: ips.clone(),
                remote,
                allow_inbound: true,
                #[cfg(test)]
                allow_loopback_sources: self.overlay_test.is_some(),
            };
        }
        let changed_context = state.context_epoch != e.network_epoch;
        state.context_epoch = e.network_epoch;
        let strict = e.overlay_policy() == WorkspaceNetworkPolicy::TailscaleOnly;
        let close_overlay = changed_context && !strict && !active_overlay(e);
        if changed_context && strict {
            for id in all_connections
                .keys()
                .filter(|id| !connections.contains_key(id))
            {
                swarm.close_connection(*id);
            }
        }
        let stale: Vec<_> = connections
            .iter()
            .filter(|(_, (_, point))| {
                !ready || close_overlay || !endpoint(&self.overlay_scope, point)
            })
            .map(|(id, (peer, _))| (*id, *peer))
            .collect();
        for (id, peer) in stale {
            swarm.close_connection(id);
            for (transfer_id, transfer) in &mut e.transfers {
                if transfer.tailscale && transfer.peer_id == peer.to_string() {
                    if let Some(cancel) = e.cancellations.get(transfer_id) {
                        if cancel.is_cancelled() {
                            continue;
                        }
                        cancel.cancel();
                        if !close_overlay {
                            transfer.error = Some("Tailscale connection is no longer available. Start a new receive after reconnecting.".into());
                        }
                    }
                }
            }
        }
        let wanted: BTreeSet<_> = ips
            .iter()
            .flat_map(|ip| [listener(*ip, false), listener(*ip, true)])
            .collect();
        let removed: Vec<_> = state
            .ids
            .keys()
            .filter(|a| !wanted.contains(*a))
            .cloned()
            .collect();
        for address in removed {
            if let Some(id) = state.ids.remove(&address) {
                swarm.remove_listener(id);
            }
        }
        if state.generation != e.tailscale_generation
            || (state.failed && state.retry_at.is_some_and(|at| Instant::now() >= at))
        {
            state.failed = false;
            state.generation = e.tailscale_generation;
            state.retry_at = None;
        }
        if ready && !state.failed {
            for address in &wanted {
                if !state.ids.contains_key(address) {
                    match swarm.listen_on(address.clone()) {
                        Ok(id) => {
                            state.ids.insert(address.clone(), id);
                        }
                        Err(_) => {
                            state.failed = true;
                            break;
                        }
                    }
                }
            }
        }
        if state.failed {
            state
                .retry_at
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(4));
            for (_, id) in std::mem::take(&mut state.ids) {
                swarm.remove_listener(id);
            }
            if e.last_error.as_deref() != Some(OVERLAY_LISTENER_ERROR) {
                e.error(OVERLAY_LISTENER_ERROR);
            }
        }
        e.overlay_listener_ready = ready
            && !state.failed
            && ips.iter().any(|ip| {
                listeners.contains(&listener(*ip, false))
                    && listeners.contains(&listener(*ip, true))
            });
        if e.overlay_listener_ready {
            if e.last_error.as_deref() == Some(OVERLAY_LISTENER_ERROR) {
                e.last_error = None;
            }
            if let Some(workspace) = e.persisted.active {
                let _ = e.refresh_overlay_contact(workspace);
            }
        }
    }

    pub(super) fn overlay_listener_error(
        &self,
        state: &mut OverlayListeners,
        id: ListenerId,
    ) -> bool {
        if state.ids.values().any(|candidate| *candidate == id) {
            state.failed = true;
            state
                .retry_at
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(4));
            true
        } else {
            false
        }
    }

    pub(super) async fn exchange_overlay_contacts(&self, peer: PeerId) -> Result<()> {
        let (snapshot, contact) = {
            let mut e = self.shared.lock().await;
            if !e.persisted.network.use_tailscale || !e.overlay_listener_ready {
                return Ok(());
            }
            let snapshot = e.active_snapshot()?;
            let own = e.refresh_overlay_contact(snapshot.workspace_id)?;
            (snapshot, own)
        };
        let mut offset = 0;
        let mut expected = None;
        loop {
            let mut stream = self.open_stream(peer, OVERLAY_CONTACT).await?;
            write_overlay_frame(
                &mut stream,
                &OverlayRequest::new(
                    &snapshot,
                    if offset == 0 {
                        Some(contact.clone())
                    } else {
                        None
                    },
                    offset,
                ),
            )
            .await?;
            let reply: OverlayResponse = read_overlay_frame(&mut stream).await?;
            ensure!(
                reply.total <= 256
                    && reply.contacts.len() <= 8
                    && offset <= reply.total
                    && reply.contacts.len() as u32 <= reply.total - offset
                    && (offset == reply.total || !reply.contacts.is_empty()),
                "Invalid overlay contact page"
            );
            ensure!(
                expected.is_none_or(|total| total == reply.total),
                "Overlay contact list changed; retry"
            );
            expected = Some(reply.total);
            offset += reply.contacts.len() as u32;
            let mut e = self.shared.lock().await;
            let current = e.authorize(&peer.to_string(), &snapshot)?;
            e.cache_overlay_contacts(&current, reply.contacts)?;
            drop(e);
            if offset >= reply.total {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(())
    }
}

pub(super) async fn serve_overlay_contacts(
    shared: Shared,
    peer: PeerId,
    mut stream: Stream,
    overlay: bool,
) -> Result<()> {
    shared.lock().await.admit_request(&peer.to_string())?;
    let request: OverlayRequest = read_overlay_frame(&mut stream).await?;
    let reply = {
        let mut e = shared.lock().await;
        e.check_transport(overlay)?;
        ensure!(
            e.persisted.network.use_tailscale && e.overlay_listener_ready,
            "Tailscale is not ready"
        );
        let snapshot = exchange::authorize_reference(&mut e, peer, &request)?;
        if let Some(contact) = request.contact {
            ensure!(
                contact.peer_id == peer.to_string(),
                "Overlay contact sender mismatch"
            );
            e.cache_overlay_contacts(&snapshot, vec![contact])?;
        }
        let contacts: Vec<_> = e
            .persisted
            .overlay_contacts
            .values()
            .filter(|c| {
                c.workspace_id == snapshot.workspace_id
                    && snapshot.contains(&c.peer_id)
                    && c.verify().is_ok()
            })
            .cloned()
            .collect();
        OverlayResponse {
            total: contacts.len() as u32,
            contacts: contacts
                .into_iter()
                .skip(request.offset as usize)
                .take(8)
                .collect(),
        }
    };
    write_overlay_frame(&mut stream, &reply).await?;
    stream.close().await?;
    Ok(())
}

pub(super) const fn overlay_protocol() -> StreamProtocol {
    OVERLAY_CONTACT
}

#[cfg(all(test, windows))]
mod roster_frame_tests {
    use super::*;
    #[tokio::test]
    async fn maximum_valid_roster_overlay_request_fits_bounded_frame() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let e = crate::lan_invite_tests::engine(dir.path())?;
        let mut snapshot = e.active_snapshot()?;
        for i in 1..MAX_MEMBERS {
            snapshot.members.push(Member {
                peer_id: libp2p::identity::Keypair::generate_ed25519()
                    .public()
                    .to_peer_id()
                    .to_string(),
                name: format!("Member {i}"),
            });
        }
        snapshot.sign(&e.key)?;
        assert_eq!(snapshot.members.len(), MAX_MEMBERS);
        let request = OverlayRequest::new(&snapshot, None, 0);
        let mut stream = futures::io::Cursor::new(Vec::new());
        assert!(
            write_overlay_frame(&mut stream, &request).await.is_ok(),
            "A valid maximum-size workspace cannot exchange overlay contacts"
        );
        Ok(())
    }
}
