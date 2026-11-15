use super::*;
use crate::invite_routes::{self, Envelope, JoinInput, RouteRecord, SharedInvite};
use std::collections::BTreeSet;
impl Engine {
    pub fn set_host_relay(&mut self, enabled: bool) -> Result<()> {
        let snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.owner_peer_id == self.peer(),
            "Only the workspace owner can host relays"
        );
        let mut next = self.persisted.clone();
        if enabled {
            next.host_relay.insert(snapshot.workspace_id);
        } else {
            next.host_relay.remove(&snapshot.workspace_id);
        }
        self.persist(next)?;
        self.relay_cancel.cancel();
        self.relay_cancel = CancellationToken::new();
        self.emit();
        Ok(())
    }
    pub fn create_shared_invite(&mut self, addresses: Vec<String>) -> Result<String> {
        let snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.owner_peer_id == self.peer(),
            "Only the owner can invite"
        );
        let mut next = self.persisted.clone();
        next.invites.retain(|i| i.expires_at > now());
        next.shared_invites.retain(|_, i| i.expires_at > now());
        ensure!(
            next.invites.len() + next.shared_invites.len() < 128,
            "Too many invitations"
        );
        next.route_sequence = next.route_sequence.max(
            next.routes
                .values()
                .filter(|record| record.peer_id == self.peer())
                .map(|record| record.sequence)
                .max()
                .unwrap_or(0),
        );
        next.route_sequence = next
            .route_sequence
            .checked_add(1)
            .context("Route sequence exhausted")?;
        let record = RouteRecord::new(
            &self.key,
            snapshot.workspace_id,
            next.route_sequence,
            addresses,
        )?;
        let envelope = Envelope::new(&self.key, snapshot.workspace_id, record.clone())?;
        let url = envelope.url()?;
        next.shared_invites.insert(
            digest(envelope.invitation.token.as_bytes()),
            SharedInvite {
                workspace_id: snapshot.workspace_id,
                expires_at: record.expires_at,
                approved: BTreeSet::new(),
                rejected: BTreeSet::new(),
            },
        );
        invite_routes::merge_record(&mut next.routes, record, &snapshot)?;
        self.persist(next)?;
        self.invite_url = Some(url.clone());
        self.emit();
        Ok(url)
    }
    pub fn revoke_current_invite(&mut self) -> Result<()> {
        let input = JoinInput::parse(
            self.invite_url
                .as_deref()
                .context("No current invitation")?,
        )?;
        let snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.workspace_id == input.invitation.workspace_id
                && snapshot.owner_peer_id == self.peer(),
            "Choose the invited workspace"
        );
        let hash = digest(input.invitation.token.as_bytes());
        let mut next = self.persisted.clone();
        next.shared_invites.remove(&hash);
        next.invites.retain(|invite| invite.token_hash != hash);
        self.persist(next)?;
        self.pending.retain(|_, p| p.token_hash != hash);
        self.invite_url = None;
        self.emit();
        Ok(())
    }
    pub fn cache_routes(&mut self, records: Vec<RouteRecord>, snapshot: &Snapshot) -> Result<()> {
        let current = self.active_snapshot()?;
        ensure!(
            current.workspace_id == snapshot.workspace_id
                && current.revision == snapshot.revision
                && current.contains(&self.peer()),
            "Route workspace changed"
        );
        ensure!(records.len() <= MAX_MEMBERS, "Too many route records");
        let mut next = self.persisted.clone();
        let mut changed = false;
        for record in records {
            if record.peer_id == self.peer() && record.sequence > next.route_sequence {
                next.route_sequence = record.sequence;
                changed = true;
            }
            changed |= invite_routes::merge_record(&mut next.routes, record, &current)?;
        }
        if changed {
            self.persist(next)?;
        }
        Ok(())
    }
    pub fn update_local_routes(&mut self, addresses: Vec<String>) -> Result<()> {
        let snapshot = self.active_snapshot()?;
        ensure!(snapshot.contains(&self.peer()), "Not a member");
        let key = format!("{}:{}", snapshot.workspace_id, self.peer());
        let mut addresses = addresses;
        addresses.sort();
        addresses.dedup();
        if self.persisted.routes.get(&key).is_some_and(|old| {
            old.addresses == addresses && old.expires_at > now().saturating_add(3600)
        }) {
            return Ok(());
        }
        let mut next = self.persisted.clone();
        next.route_sequence = next
            .route_sequence
            .checked_add(1)
            .context("Route sequence exhausted")?;
        let record = RouteRecord::new(
            &self.key,
            snapshot.workspace_id,
            next.route_sequence,
            addresses,
        )?;
        invite_routes::merge_record(&mut next.routes, record, &snapshot)?;
        self.persist(next)?;
        Ok(())
    }
}
