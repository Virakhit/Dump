use super::*;
use crate::overlay::{OverlayContact, OverlayLink, WorkspaceNetworkPolicy};

impl Engine {
    pub fn overlay_policy(&self) -> WorkspaceNetworkPolicy {
        if let Some(invitation) = &self.persisted.joining {
            return if self
                .persisted
                .overlay_joining
                .as_ref()
                .is_some_and(|contact| {
                    contact.workspace_id == invitation.workspace_id
                        && contact.peer_id == invitation.owner_peer_id
                }) {
                WorkspaceNetworkPolicy::TailscaleOnly
            } else {
                WorkspaceNetworkPolicy::Legacy
            };
        }
        if self.lan_joining.is_some() {
            return WorkspaceNetworkPolicy::Legacy;
        }
        self.persisted
            .active
            .and_then(|workspace| self.persisted.workspace_network.get(&workspace).copied())
            .unwrap_or_default()
    }
    pub fn set_network_policy(&mut self, policy: WorkspaceNetworkPolicy) -> Result<()> {
        self.ensure_network_idle()?;
        let snapshot = self.active_snapshot()?;
        self.check_overlay_snapshot(&snapshot)?;
        if policy == WorkspaceNetworkPolicy::TailscaleOnly {
            self.ensure_overlay_ready()?;
        }
        if self
            .persisted
            .workspace_network
            .get(&snapshot.workspace_id)
            .copied()
            .unwrap_or_default()
            == policy
        {
            return Ok(());
        }
        let mut next = self.persisted.clone();
        next.workspace_network.insert(snapshot.workspace_id, policy);
        self.persist(next)?;
        self.reset_runtime();
        self.emit();
        Ok(())
    }
    pub fn refresh_overlay_contact(&mut self, workspace: Uuid) -> Result<OverlayContact> {
        self.ensure_overlay_ready()?;
        let snapshot = &self
            .persisted
            .workspaces
            .get(&workspace)
            .context("Workspace not found")?
            .snapshot;
        self.check_overlay_snapshot(snapshot)?;
        let node = self.tailscale.ready_node()?.clone();
        let own = self.peer();
        let cache_key = format!("{workspace}:{own}");
        let current = now();
        let old = self.persisted.overlay_contacts.get(&cache_key);
        if let Some(old) = old.filter(|old| {
            old.node_id == node.id
                && old.dns_name == node.dns_name
                && old.expires_at > current.saturating_add(3600)
                && old.verify().is_ok()
        }) {
            return Ok(old.clone());
        }
        let sequence = old.map_or(Ok(1), |old| {
            old.sequence
                .checked_add(1)
                .context("Overlay contact sequence exhausted")
        })?;
        let contact =
            OverlayContact::issue(&self.key, workspace, node.id, node.dns_name, sequence)?;
        let mut next = self.persisted.clone();
        // Keep our own sequence watermarks even after expiry, as with legacy contacts.
        next.overlay_contacts
            .retain(|_, contact| contact.peer_id == own || contact.expires_at > current);
        ensure!(
            next.overlay_contacts.len() < 256 || next.overlay_contacts.contains_key(&cache_key),
            "Overlay contact cache full"
        );
        next.overlay_contacts.insert(cache_key, contact.clone());
        self.persist(next)?;
        Ok(contact)
    }
    pub fn create_overlay_invite(&mut self) -> Result<String> {
        let snapshot = self.active_snapshot()?;
        self.check_overlay_snapshot(&snapshot)?;
        ensure!(
            snapshot.owner_peer_id == self.peer(),
            "Only the workspace owner can invite"
        );
        self.set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        let contact = self.refresh_overlay_contact(snapshot.workspace_id)?;
        let mut invitation = Invitation::parse(&self.create_invite()?)?;
        invitation.contact = None;
        let url = OverlayLink::Invite {
            invitation,
            contact,
        }
        .url()?;
        self.invite_url = Some(url.clone());
        self.emit();
        Ok(url)
    }
    pub fn create_overlay_connect(&mut self) -> Result<String> {
        let snapshot = self.active_snapshot()?;
        self.check_overlay_snapshot(&snapshot)?;
        self.set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        let contact = self.refresh_overlay_contact(snapshot.workspace_id)?;
        let url = OverlayLink::Connect { contact }.url()?;
        self.invite_url = Some(url.clone());
        self.emit();
        Ok(url)
    }
    pub fn join_overlay(&mut self, value: &str) -> Result<()> {
        let link = OverlayLink::parse(value)?;
        self.ensure_network_idle()?;
        let mut next = self.persisted.clone();
        match link {
            OverlayLink::Invite {
                invitation,
                contact,
            } => {
                ensure!(
                    invitation.owner_peer_id != self.peer(),
                    "This is your own invitation"
                );
                if let Some(existing) = next.workspaces.get(&invitation.workspace_id) {
                    ensure!(
                        existing.snapshot.owner_peer_id == invitation.owner_peer_id,
                        "Invitation conflicts with the existing workspace owner"
                    );
                }
                next.joining = Some(invitation);
                next.overlay_joining = Some(contact);
            }
            OverlayLink::Connect { contact } => {
                let snapshot = &next
                    .workspaces
                    .get(&contact.workspace_id)
                    .context("Join this workspace before opening a connect link")?
                    .snapshot;
                let contacts = self.overlay_cache(snapshot, vec![contact.clone()])?;
                next.active = Some(contact.workspace_id);
                next.workspace_network
                    .insert(contact.workspace_id, WorkspaceNetworkPolicy::TailscaleOnly);
                next.overlay_contacts = contacts;
                next.joining = None;
                next.overlay_joining = None;
            }
        }
        self.persist(next)?;
        self.reset_runtime();
        self.last_error = None;
        self.emit();
        Ok(())
    }
    pub fn cache_overlay_contacts(
        &mut self,
        snapshot: &Snapshot,
        contacts: Vec<OverlayContact>,
    ) -> Result<()> {
        ensure!(
            self.persisted.active == Some(snapshot.workspace_id),
            "Invalid overlay contact workspace"
        );
        let contacts = self.overlay_cache(snapshot, contacts)?;
        if contacts != self.persisted.overlay_contacts {
            let mut next = self.persisted.clone();
            next.overlay_contacts = contacts;
            self.persist(next)?;
        }
        Ok(())
    }
    pub fn check_transport(&self, overlay: bool) -> Result<()> {
        let active_policy = self
            .persisted
            .active
            .and_then(|workspace| self.persisted.workspace_network.get(&workspace).copied())
            .unwrap_or_default();
        let strict = active_policy == WorkspaceNetworkPolicy::TailscaleOnly
            || self.overlay_policy() == WorkspaceNetworkPolicy::TailscaleOnly;
        ensure!(
            overlay == strict,
            "Transport does not match the workspace network policy"
        );
        if overlay {
            self.ensure_overlay_ready()?;
        }
        Ok(())
    }
    pub fn ensure_network_idle(&self) -> Result<()> {
        ensure!(
            self.preparing.is_empty()
                && self.transfers.values().all(|transfer| matches!(
                    transfer.status.as_str(),
                    "Completed" | "Cancelled" | "Failed"
                )),
            "Finish current file preparation and transfers before changing networks"
        );
        ensure!(
            !self.persisted.chat_messages.iter().any(|entry| {
                Some(entry.message.workspace_id) == self.persisted.active
                    && entry
                        .delivery
                        .as_ref()
                        .is_some_and(|delivery| delivery.status == ChatDeliveryStatus::Sending)
            }),
            "Finish the current chat delivery before changing networks"
        );
        Ok(())
    }

    fn ensure_overlay_ready(&self) -> Result<()> {
        ensure!(
            self.persisted.network.use_tailscale,
            "Enable installed Tailscale first"
        );
        self.tailscale.ready_node()?;
        ensure!(
            self.overlay_listener_ready,
            "Tailscale listener is not ready"
        );
        Ok(())
    }

    fn check_overlay_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        snapshot.verify()?;
        ensure!(
            snapshot.contains(&self.peer()),
            "This device is not an approved workspace member"
        );
        ensure!(
            self.persisted
                .workspaces
                .get(&snapshot.workspace_id)
                .is_some_and(|workspace| workspace.snapshot == *snapshot),
            "Overlay contacts require the current workspace roster"
        );
        Ok(())
    }

    fn overlay_cache(
        &self,
        snapshot: &Snapshot,
        contacts: Vec<OverlayContact>,
    ) -> Result<BTreeMap<String, OverlayContact>> {
        ensure!(contacts.len() <= 8, "Too many overlay contacts in one page");
        self.check_overlay_snapshot(snapshot)?;
        let current = now();
        let own = self.peer();
        let mut cache = self.persisted.overlay_contacts.clone();
        cache.retain(|_, contact| contact.peer_id == own || contact.expires_at > current);
        for contact in contacts {
            contact.verify()?;
            ensure!(
                contact.workspace_id == snapshot.workspace_id
                    && snapshot.contains(&contact.peer_id),
                "Overlay contact is not a member of this workspace"
            );
            let key = contact.cache_key();
            if let Some(old) = cache.get(&key) {
                ensure!(
                    contact.sequence >= old.sequence,
                    "Stale overlay contact sequence"
                );
                if contact.sequence == old.sequence {
                    ensure!(old == &contact, "Conflicting overlay contact sequence");
                    continue;
                }
            }
            ensure!(
                cache.len() < 256 || cache.contains_key(&key),
                "Overlay contact cache full"
            );
            cache.insert(key, contact);
        }
        Ok(cache)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::tailscale::{TailscaleNode, TailscaleSnapshot, TailscaleState};

    fn ready(e: &mut Engine, id: &str) {
        e.persisted.network.use_tailscale = true;
        e.tailscale = TailscaleSnapshot {
            state: TailscaleState::Ready,
            self_node: Some(TailscaleNode {
                id: id.into(),
                dns_name: format!("{id}.test-tail.ts.net"),
                ips: vec!["100.80.1.1".parse().unwrap()],
                online: true,
            }),
            peers: vec![],
        };
        e.overlay_listener_ready = true;
    }

    #[tokio::test]
    async fn cold_start_invite_is_persisted_until_the_adapter_becomes_ready() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let owner = Keypair::generate_ed25519();
        let workspace = Uuid::new_v4();
        let invitation = Invitation {
            version: VERSION,
            workspace_id: workspace,
            owner_peer_id: owner.public().to_peer_id().to_string(),
            token: random_secret(),
            contact: None,
        };
        let contact = OverlayContact::issue(
            &owner,
            workspace,
            "owner".into(),
            "owner.tail.ts.net".into(),
            1,
        )?;
        let link = OverlayLink::Invite {
            invitation,
            contact: contact.clone(),
        }
        .url()?;
        let original_peer;
        {
            let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
            let mut e = shared.lock().await;
            e.persisted.network.use_tailscale = true;
            assert_eq!(e.tailscale.state, TailscaleState::NotInstalled);
            assert!(!e.overlay_listener_ready);
            original_peer = e.peer();
            e.join_overlay(&link)?;
            assert!(e.persisted.workspaces.is_empty());
            assert_eq!(e.persisted.overlay_joining.as_ref(), Some(&contact));
            assert_eq!(
                e.persisted
                    .joining
                    .as_ref()
                    .map(|invite| invite.workspace_id),
                Some(workspace)
            );
            assert!(!e.join_request_submitted);
            assert!(e.check_transport(true).is_err());
            assert!(e.check_transport(false).is_err());
        }
        let reopened = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = reopened.lock().await;
        assert_eq!(e.peer(), original_peer);
        assert_eq!(e.persisted.overlay_joining.as_ref(), Some(&contact));
        assert!(e.persisted.joining.is_some());
        assert!(e.persisted.workspaces.is_empty());
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        ready(&mut e, "receiver");
        e.check_transport(true)?;
        assert!(!e.join_request_submitted);
        Ok(())
    }

    #[tokio::test]
    async fn cold_start_connect_preserves_roster_and_disabled_adapter_preference() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Offline approved members".into())?;
        let member = Keypair::generate_ed25519();
        let mut snapshot = e.active_snapshot()?;
        snapshot.members.push(Member {
            peer_id: member.public().to_peer_id().to_string(),
            name: "Friend".into(),
        });
        snapshot.revision += 1;
        snapshot.sign(&e.key)?;
        e.persisted
            .workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        let original_peer = e.peer();
        let contact = OverlayContact::issue(
            &member,
            snapshot.workspace_id,
            "remote".into(),
            "remote.tail.ts.net".into(),
            1,
        )?;
        let link = OverlayLink::Connect {
            contact: contact.clone(),
        }
        .url()?;
        assert!(!e.persisted.network.use_tailscale);
        assert_eq!(e.tailscale.state, TailscaleState::NotInstalled);
        assert!(!e.overlay_listener_ready);
        e.join_overlay(&link)?;
        assert_eq!(e.active_snapshot()?, snapshot);
        assert_eq!(e.peer(), original_peer);
        assert!(!e.persisted.network.use_tailscale);
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        assert!(e.persisted.joining.is_none());
        assert!(e.persisted.overlay_joining.is_none());
        assert_eq!(e.persisted.overlay_contacts[&contact.cache_key()], contact);
        assert!(e.check_transport(true).is_err());
        assert!(e.check_transport(false).is_err());
        drop(e);
        drop(shared);
        let reopened = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let e = reopened.lock().await;
        assert_eq!(e.active_snapshot()?, snapshot);
        assert_eq!(e.peer(), original_peer);
        assert!(!e.persisted.network.use_tailscale);
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        Ok(())
    }

    #[tokio::test]
    async fn policy_changes_wait_for_in_flight_chat_delivery() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Chat".into())?;
        ready(&mut e, "local");
        let workspace = e.active_snapshot()?.workspace_id;
        let message = ChatMessage::new(workspace, "Me".into(), "In flight".into(), &e.key)?;
        e.persisted.chat_messages.push(StoredChat {
            message,
            delivery: Some(ChatDelivery {
                attempted: 1,
                delivered: 0,
                status: ChatDeliveryStatus::Sending,
            }),
        });
        assert!(e.ensure_network_idle().is_err());
        assert!(e
            .set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)
            .is_err());
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::Legacy);
        e.persisted.chat_messages[0]
            .delivery
            .as_mut()
            .unwrap()
            .status = ChatDeliveryStatus::Complete;
        e.ensure_network_idle()?;
        e.set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        Ok(())
    }

    #[tokio::test]
    async fn overlay_invite_keeps_identity_and_waits_for_explicit_approval() -> Result<()> {
        let a_dir = tempfile::tempdir()?;
        let b_dir = tempfile::tempdir()?;
        let a = Engine::open(a_dir.path(), Arc::new(|_| {}))?;
        let b = Engine::open(b_dir.path(), Arc::new(|_| {}))?;
        let mut a = a.lock().await;
        let mut b = b.lock().await;
        a.create_workspace("Overlay".into())?;
        ready(&mut a, "owner");
        ready(&mut b, "member");
        let original = b.peer();
        let link = a.create_overlay_invite()?;
        assert!(link.starts_with(crate::overlay::PREFIX));
        assert_eq!(a.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        b.join(&link)?;
        assert_eq!(b.peer(), original);
        assert_eq!(b.overlay_policy(), WorkspaceNetworkPolicy::TailscaleOnly);
        assert!(b.persisted.workspaces.is_empty());
        assert!(!b.join_request_submitted);
        b.cancel_join()?;
        assert!(b.persisted.overlay_joining.is_none());
        assert_eq!(b.overlay_policy(), WorkspaceNetworkPolicy::Legacy);
        Ok(())
    }

    #[tokio::test]
    async fn existing_members_connect_without_owner_and_reject_outsiders_or_replays() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Existing".into())?;
        ready(&mut e, "local");
        let workspace = e.active_snapshot()?.workspace_id;
        let member = Keypair::generate_ed25519();
        let mut snapshot = e.active_snapshot()?;
        snapshot.members.push(Member {
            peer_id: member.public().to_peer_id().to_string(),
            name: "Friend".into(),
        });
        snapshot.revision += 1;
        snapshot.sign(&e.key)?;
        e.persisted.workspaces.get_mut(&workspace).unwrap().snapshot = snapshot.clone();
        let contact = OverlayContact::issue(
            &member,
            workspace,
            "remote".into(),
            "remote.other-tail.ts.net".into(),
            2,
        )?;
        let link = OverlayLink::Connect {
            contact: contact.clone(),
        }
        .url()?;
        let before = e.active_snapshot()?;
        e.join(&link)?;
        assert_eq!(e.active_snapshot()?, before);
        assert!(e.persisted.joining.is_none());
        assert_eq!(e.persisted.overlay_contacts[&contact.cache_key()], contact);
        let old = OverlayContact::issue(
            &member,
            workspace,
            "remote".into(),
            "remote.other-tail.ts.net".into(),
            1,
        )?;
        assert!(e.cache_overlay_contacts(&snapshot, vec![old]).is_err());
        let outsider = OverlayContact::issue(
            &Keypair::generate_ed25519(),
            workspace,
            "outsider".into(),
            "outsider.other.ts.net".into(),
            1,
        )?;
        assert!(e
            .join(&OverlayLink::Connect { contact: outsider }.url()?)
            .is_err());
        e.check_transport(true)?;
        assert!(e.check_transport(false).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn policy_changes_wait_for_transfers_and_legacy_state_defaults_are_preserved(
    ) -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Legacy".into())?;
        let mut old = serde_json::to_value(&e.persisted)?;
        for field in ["overlay_contacts", "workspace_network", "overlay_joining"] {
            old.as_object_mut().unwrap().remove(field);
        }
        old["network"]
            .as_object_mut()
            .unwrap()
            .remove("use_tailscale");
        let loaded: Persisted = serde_json::from_value(old)?;
        assert!(loaded.workspace_network.is_empty());
        assert!(!loaded.network.use_tailscale);
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::Legacy);
        ready(&mut e, "local");
        e.preparing.push("file".into());
        assert!(e
            .set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)
            .is_err());
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::Legacy);
        e.preparing.clear();
        e.set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        assert!(e.check_transport(false).is_err());
        e.check_transport(true)?;
        Ok(())
    }

    #[tokio::test]
    async fn contact_refresh_reuses_fresh_binding_and_increments_sequence_after_node_change(
    ) -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Refresh".into())?;
        ready(&mut e, "local");
        let workspace = e.active_snapshot()?.workspace_id;
        let first = e.refresh_overlay_contact(workspace)?;
        assert_eq!(e.refresh_overlay_contact(workspace)?, first);
        let node = e.tailscale.self_node.as_mut().unwrap();
        node.id = "replacement".into();
        node.dns_name = "replacement.test-tail.ts.net".into();
        let replacement = e.refresh_overlay_contact(workspace)?;
        replacement.verify()?;
        assert_eq!(replacement.sequence, first.sequence + 1);
        assert_eq!(replacement.node_id, "replacement");
        assert_eq!(
            e.persisted.overlay_contacts[&replacement.cache_key()],
            replacement
        );
        Ok(())
    }

    #[tokio::test]
    async fn contact_pages_reject_outsiders_scope_changes_and_conflicts_atomically() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Cache".into())?;
        let member = Keypair::generate_ed25519();
        let mut snapshot = e.active_snapshot()?;
        snapshot.members.push(Member {
            peer_id: member.public().to_peer_id().to_string(),
            name: "Member".into(),
        });
        snapshot.revision += 1;
        snapshot.sign(&e.key)?;
        e.persisted
            .workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        let signed = OverlayContact::issue(
            &member,
            snapshot.workspace_id,
            "remote".into(),
            "remote.tail.ts.net".into(),
            1,
        )?;
        let outsider = OverlayContact::issue(
            &Keypair::generate_ed25519(),
            snapshot.workspace_id,
            "outsider".into(),
            "outsider.tail.ts.net".into(),
            1,
        )?;
        assert!(e
            .cache_overlay_contacts(&snapshot, vec![signed.clone(), outsider])
            .is_err());
        assert!(e.persisted.overlay_contacts.is_empty());
        let wrong_workspace = OverlayContact::issue(
            &member,
            Uuid::new_v4(),
            "remote".into(),
            "remote.tail.ts.net".into(),
            1,
        )?;
        assert!(e
            .cache_overlay_contacts(&snapshot, vec![wrong_workspace])
            .is_err());
        assert!(e
            .cache_overlay_contacts(&snapshot, vec![signed.clone(); 9])
            .is_err());
        e.cache_overlay_contacts(&snapshot, vec![signed.clone()])?;
        e.cache_overlay_contacts(&snapshot, vec![signed.clone()])?;
        let conflicting = OverlayContact::issue(
            &member,
            snapshot.workspace_id,
            "other".into(),
            "other.tail.ts.net".into(),
            1,
        )?;
        assert!(e
            .cache_overlay_contacts(&snapshot, vec![conflicting])
            .is_err());
        assert_eq!(e.persisted.overlay_contacts[&signed.cache_key()], signed);
        let mut stale = snapshot.clone();
        stale.revision -= 1;
        stale.sign(&e.key)?;
        assert!(e.cache_overlay_contacts(&stale, vec![signed]).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn active_tailscale_policy_stays_closed_during_legacy_join_and_adapter_loss() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Protected".into())?;
        ready(&mut e, "local");
        e.set_network_policy(WorkspaceNetworkPolicy::TailscaleOnly)?;
        e.persisted.joining = Some(Invitation {
            version: VERSION,
            workspace_id: Uuid::new_v4(),
            owner_peer_id: Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string(),
            token: random_secret(),
            contact: None,
        });
        assert_eq!(e.overlay_policy(), WorkspaceNetworkPolicy::Legacy);
        assert!(e.check_transport(false).is_err());
        e.check_transport(true)?;
        e.persisted.network.use_tailscale = false;
        assert!(e.check_transport(true).is_err());
        assert!(e.check_transport(false).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn ordinary_member_exports_connect_without_owner_or_invitation_token() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let shared = Engine::open(dir.path(), Arc::new(|_| {}))?;
        let mut e = shared.lock().await;
        e.create_workspace("Members".into())?;
        let owner = Keypair::generate_ed25519();
        let mut snapshot = e.active_snapshot()?;
        snapshot.owner_peer_id = owner.public().to_peer_id().to_string();
        snapshot.owner_public_key = public_key(&owner);
        snapshot.members.push(Member {
            peer_id: snapshot.owner_peer_id.clone(),
            name: "Offline owner".into(),
        });
        snapshot.sign(&owner)?;
        e.persisted
            .workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        ready(&mut e, "local");
        assert!(e.create_overlay_invite().is_err());
        let link = e.create_overlay_connect()?;
        match OverlayLink::parse(&link)? {
            OverlayLink::Connect { contact } => assert_eq!(contact.peer_id, e.peer()),
            _ => panic!("member link must not contain an invitation"),
        }
        assert!(e.persisted.invites.is_empty());
        assert_eq!(e.active_snapshot()?, snapshot);
        Ok(())
    }
}
