use crate::{model::*, storage::Store};
use anyhow::{ensure, Context, Result};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
    sync::Arc,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub type Shared = Arc<Mutex<Engine>>;
pub type Notify = Arc<dyn Fn(View) + Send + Sync>;

#[derive(Clone, Serialize)]
pub struct Pending {
    pub peer_id: String,
    pub name: String,
    pub workspace_id: Uuid,
    pub fingerprint: String,
    #[serde(skip)]
    pub token_hash: String,
    #[serde(skip)]
    pub requested_at: u64,
}
#[derive(Clone, Serialize)]
pub struct FileView {
    pub manifest: Manifest,
    pub mine: bool,
    pub owner_name: String,
}
#[derive(Clone, Serialize)]
pub struct WorkspaceView {
    pub snapshot: Snapshot,
    pub mine: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Transfer {
    pub id: Uuid,
    pub file_id: Uuid,
    pub name: String,
    pub peer_id: String,
    pub direction: String,
    pub bytes: u64,
    pub total: u64,
    pub status: String,
    pub error: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct View {
    pub peer_id: String,
    pub fingerprint: String,
    pub device_name: String,
    pub active: Option<Uuid>,
    pub workspaces: Vec<WorkspaceView>,
    pub online: Vec<String>,
    pub files: Vec<FileView>,
    pub pending: Vec<Pending>,
    pub preparing: Vec<String>,
    pub joining: bool,
    pub network_status: String,
    pub transfers: Vec<Transfer>,
    pub invite_url: Option<String>,
    pub last_error: Option<String>,
    pub network: crate::relay_host::Settings,
}

pub struct Engine {
    pub persisted: Persisted,
    pub key: Keypair,
    pub store: Store,
    pub pending: BTreeMap<String, Pending>,
    pub remote: BTreeMap<Uuid, Manifest>,
    pub online: BTreeMap<String, u64>,
    pub request_budget: BTreeMap<String, (u64, u32)>,
    pub transfers: BTreeMap<Uuid, Transfer>,
    pub cancellations: BTreeMap<Uuid, CancellationToken>,
    pub preparing: Vec<String>,
    pub network_status: String,
    pub invite_url: Option<String>,
    pub last_error: Option<String>,
    pub notify: Notify,
}
impl Engine {
    pub fn open(directory: &Path, notify: Notify) -> Result<Shared> {
        let store = Store::new(directory)?;
        let mut persisted = store.load()?;
        persisted.partials.retain(|partial| {
            // Only recorded, UUID-named Dump partials are eligible for startup cleanup.
            if partial
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".dump-") && n.ends_with(".part"))
            {
                return std::fs::remove_file(partial)
                    .is_err_and(|err| err.kind() != std::io::ErrorKind::NotFound);
            }
            false
        });
        store.save(&persisted)?;
        let key = persisted.key()?;
        Ok(Arc::new(Mutex::new(Self {
            persisted,
            key,
            store,
            pending: BTreeMap::new(),
            remote: BTreeMap::new(),
            online: BTreeMap::new(),
            request_budget: BTreeMap::new(),
            transfers: BTreeMap::new(),
            cancellations: BTreeMap::new(),
            preparing: Vec::new(),
            network_status: "Starting LAN discovery…".into(),
            invite_url: None,
            last_error: None,
            notify,
        })))
    }
    pub fn peer(&self) -> String {
        self.key.public().to_peer_id().to_string()
    }
    pub fn persist(&mut self, next: Persisted) -> Result<()> {
        self.store.save(&next)?;
        self.persisted = next;
        Ok(())
    }
    pub fn emit(&self) {
        (self.notify)(self.view());
    }
    pub fn error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
        self.emit();
    }
    pub fn admit_request(&mut self, peer: &str) -> Result<()> {
        let current = now();
        self.request_budget.retain(|_, (at, _)| *at == current);
        ensure!(
            self.request_budget.len() < 256 || self.request_budget.contains_key(peer),
            "Request limit reached"
        );
        let entry = self
            .request_budget
            .entry(peer.into())
            .or_insert((current, 0));
        ensure!(entry.1 < 10, "Request limit reached");
        entry.1 += 1;
        Ok(())
    }
    pub fn view(&self) -> View {
        let peer = self.peer();
        let owner_name = |id: &str, ws: Uuid| {
            self.persisted
                .workspaces
                .get(&ws)
                .and_then(|w| w.snapshot.members.iter().find(|m| m.peer_id == id))
                .map_or_else(|| "Another device".into(), |m| m.name.clone())
        };
        let mut files: Vec<_> = self
            .persisted
            .shares
            .values()
            .filter(|s| {
                s.available
                    && Some(s.manifest.workspace_id) == self.persisted.active
                    && self.active_snapshot().is_ok_and(|w| w.contains(&peer))
            })
            .map(|s| FileView {
                manifest: s.manifest.clone(),
                mine: true,
                owner_name: self.persisted.device_name.clone(),
            })
            .collect();
        files.extend(
            self.remote
                .values()
                .filter(|m| Some(m.workspace_id) == self.persisted.active)
                .map(|m| FileView {
                    manifest: m.clone(),
                    mine: false,
                    owner_name: owner_name(&m.owner_peer_id, m.workspace_id),
                }),
        );
        files.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        View {
            peer_id: peer.clone(),
            fingerprint: digest(&self.key.public().encode_protobuf()),
            device_name: self.persisted.device_name.clone(),
            active: self.persisted.active,
            workspaces: self
                .persisted
                .workspaces
                .values()
                .map(|w| WorkspaceView {
                    snapshot: w.snapshot.clone(),
                    mine: w.snapshot.owner_peer_id == peer,
                })
                .collect(),
            online: self.online.keys().cloned().collect(),
            files,
            pending: self
                .pending
                .values()
                .filter(|p| Some(p.workspace_id) == self.persisted.active)
                .cloned()
                .collect(),
            preparing: self.preparing.clone(),
            joining: self.persisted.joining.is_some(),
            network_status: self.network_status.clone(),
            transfers: self.transfers.values().cloned().collect(),
            invite_url: self.invite_url.clone(),
            last_error: self.last_error.clone(),
            network: self.persisted.network.clone(),
        }
    }
    pub fn create_workspace(&mut self, name: String) -> Result<()> {
        validate_label(&name)?;
        let peer = self.peer();
        let id = Uuid::new_v4();
        let mut snapshot = Snapshot {
            version: VERSION,
            workspace_id: id,
            name,
            owner_peer_id: peer.clone(),
            owner_public_key: public_key(&self.key),
            revision: 1,
            members: vec![Member {
                peer_id: peer.clone(),
                name: self.persisted.device_name.clone(),
            }],
            signature: String::new(),
        };
        snapshot.sign(&self.key)?;
        let mut next = self.persisted.clone();
        next.workspaces.insert(
            id,
            Workspace {
                snapshot,
                secret: random_secret(),
            },
        );
        next.active = Some(id);
        next.audit("workspace-created", id, &peer);
        self.persist(next)?;
        self.reset_runtime();
        self.emit();
        Ok(())
    }
    pub fn active_snapshot(&self) -> Result<Snapshot> {
        Ok(self
            .persisted
            .workspaces
            .get(&self.persisted.active.context("Choose a workspace first")?)
            .context("Workspace not found")?
            .snapshot
            .clone())
    }
    pub fn activate(&mut self, id: Uuid) -> Result<()> {
        ensure!(
            self.persisted.workspaces.contains_key(&id),
            "Workspace not found"
        );
        let mut next = self.persisted.clone();
        next.active = Some(id);
        self.persist(next)?;
        self.reset_runtime();
        self.emit();
        Ok(())
    }
    pub(crate) fn reset_runtime(&mut self) {
        for token in self.cancellations.values() {
            token.cancel();
        }
        self.remote.clear();
        self.online.clear();
        self.invite_url = None;
    }
    pub fn create_invite(&mut self) -> Result<String> {
        let snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.owner_peer_id == self.peer(),
            "Only the workspace owner can invite"
        );
        let token = random_secret();
        let mut next = self.persisted.clone();
        next.invites.retain(|i| i.expires_at > now());
        ensure!(
            next.invites.len() < 128,
            "Too many invitations; wait for old ones to expire"
        );
        next.invites.push(InviteRecord {
            workspace_id: snapshot.workspace_id,
            token_hash: digest(token.as_bytes()),
            expires_at: now() + 86400,
            approved_peer: None,
        });
        let contact = next
            .contacts
            .get(&self.peer())
            .filter(|c| c.verify().is_ok() && !c.addresses.is_empty())
            .map(|c| {
                crate::contact::Contact::issue(
                    &self.key,
                    c.sequence
                        .checked_add(1)
                        .context("Contact sequence exhausted")?,
                    c.addresses.clone(),
                )
            })
            .transpose()?;
        if let Some(contact) = &contact {
            next.contacts.insert(self.peer(), contact.clone());
        }
        self.persist(next)?;
        let url = Invitation {
            version: VERSION,
            workspace_id: snapshot.workspace_id,
            owner_peer_id: self.peer(),
            token,
            contact,
        }
        .url()?;
        self.invite_url = Some(url.clone());
        self.emit();
        Ok(url)
    }
    pub(crate) fn update_contact(&mut self, addresses: Vec<String>) -> Result<()> {
        let peer = self.peer();
        if self
            .persisted
            .contacts
            .get(&peer)
            .is_some_and(|c| c.addresses == addresses && c.expires_at > now() + 3600)
        {
            return Ok(());
        }
        let sequence = self.persisted.contacts.get(&peer).map_or(Ok(1), |c| {
            c.sequence
                .checked_add(1)
                .context("Contact sequence exhausted")
        })?;
        let contact = crate::contact::Contact::issue(&self.key, sequence, addresses)?;
        let mut next = self.persisted.clone();
        next.contacts
            .retain(|id, c| *id == peer || c.expires_at > now());
        ensure!(
            next.contacts.len() < 256 || next.contacts.contains_key(&peer),
            "Contact cache full"
        );
        next.contacts.insert(peer, contact);
        self.persist(next)
    }
    pub(crate) fn cache_contacts(
        &mut self,
        snapshot: &Snapshot,
        contacts: Vec<crate::contact::Contact>,
    ) -> Result<()> {
        ensure!(
            contacts.len() <= 8 && self.persisted.active == Some(snapshot.workspace_id),
            "Invalid contact page"
        );
        let mut next = self.persisted.clone();
        let own = self.peer();
        next.contacts
            .retain(|id, c| *id == own || c.expires_at > now());
        let mut changed = false;
        for contact in contacts {
            contact.verify()?;
            ensure!(
                snapshot.contains(&contact.peer_id),
                "Contact is not a member"
            );
            if contact.peer_id == own {
                continue;
            }
            if let Some(old) = next.contacts.get(&contact.peer_id) {
                if contact.sequence < old.sequence {
                    continue;
                }
                if contact.sequence == old.sequence {
                    ensure!(old == &contact, "Conflicting contact sequence");
                    continue;
                }
            }
            ensure!(
                next.contacts.len() < 256 || next.contacts.contains_key(&contact.peer_id),
                "Contact cache full"
            );
            next.contacts.insert(contact.peer_id.clone(), contact);
            changed = true;
        }
        if changed {
            self.persist(next)?;
        }
        Ok(())
    }
    pub fn join(&mut self, value: &str) -> Result<()> {
        let invite = Invitation::parse(value)?;
        ensure!(
            invite.owner_peer_id != self.peer(),
            "This is your own invitation"
        );
        if let Some(existing) = self.persisted.workspaces.get(&invite.workspace_id) {
            ensure!(
                existing.snapshot.owner_peer_id == invite.owner_peer_id,
                "Invitation conflicts with the existing workspace owner"
            );
        }
        let mut next = self.persisted.clone();
        next.joining = Some(invite);
        self.persist(next)?;
        self.emit();
        Ok(())
    }
    pub fn cancel_join(&mut self) -> Result<()> {
        let mut next = self.persisted.clone();
        next.joining = None;
        self.persist(next)?;
        self.emit();
        Ok(())
    }
    pub fn set_name(&mut self, name: String) -> Result<()> {
        validate_label(&name)?;
        let mut next = self.persisted.clone();
        next.device_name = name;
        self.persist(next)?;
        self.emit();
        Ok(())
    }
    pub fn approve(&mut self, peer: &str) -> Result<()> {
        let pending = self
            .pending
            .get(peer)
            .context("Join request is no longer available")?
            .clone();
        let mut snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.workspace_id == pending.workspace_id && snapshot.owner_peer_id == self.peer(),
            "Not the workspace owner"
        );
        let mut next = self.persisted.clone();
        let invite = next
            .invites
            .iter_mut()
            .find(|i| {
                i.workspace_id == snapshot.workspace_id
                    && i.token_hash == pending.token_hash
                    && i.expires_at > now()
                    && i.approved_peer.is_none()
            })
            .context("Invitation expired or already used")?;
        invite.approved_peer = Some(peer.into());
        if !snapshot.contains(peer) {
            snapshot.members.push(Member {
                peer_id: peer.into(),
                name: pending.name,
            });
        }
        snapshot.revision = snapshot
            .revision
            .checked_add(1)
            .context("Revision exhausted")?;
        snapshot.sign(&self.key)?;
        next.workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        next.audit("member-approved", snapshot.workspace_id, peer);
        self.persist(next)?;
        self.pending.remove(peer);
        self.emit();
        Ok(())
    }
    pub fn reject(&mut self, peer: &str) -> Result<()> {
        let pending = self
            .pending
            .get(peer)
            .context("Join request not found")?
            .clone();
        ensure!(
            self.active_snapshot()?.owner_peer_id == self.peer(),
            "Only the owner can reject requests"
        );
        let mut next = self.persisted.clone();
        next.invites.retain(|i| i.token_hash != pending.token_hash);
        self.persist(next)?;
        self.pending.remove(peer);
        self.emit();
        Ok(())
    }
    pub fn remove_member(&mut self, peer: &str) -> Result<()> {
        let mut snapshot = self.active_snapshot()?;
        ensure!(
            snapshot.owner_peer_id == self.peer() && peer != self.peer(),
            "Only the owner can remove other members"
        );
        ensure!(snapshot.contains(peer), "Member not found");
        snapshot.members.retain(|m| m.peer_id != peer);
        snapshot.revision = snapshot
            .revision
            .checked_add(1)
            .context("Revision exhausted")?;
        snapshot.sign(&self.key)?;
        let workspace_id = snapshot.workspace_id;
        let mut next = self.persisted.clone();
        next.workspaces.get_mut(&workspace_id).unwrap().snapshot = snapshot;
        next.invites
            .retain(|i| i.approved_peer.as_deref() != Some(peer));
        next.audit("member-removed", next.active.unwrap(), peer);
        self.persist(next)?;
        self.reconcile();
        self.emit();
        Ok(())
    }
    pub fn accept_snapshot(&mut self, snapshot: &Snapshot) -> Result<()> {
        snapshot.verify()?;
        let local = self
            .persisted
            .workspaces
            .get(&snapshot.workspace_id)
            .context("Access denied")?;
        ensure!(
            local.snapshot.owner_peer_id == snapshot.owner_peer_id
                && local.snapshot.owner_public_key == snapshot.owner_public_key,
            "Access denied"
        );
        ensure!(
            snapshot.revision >= local.snapshot.revision,
            "Membership revision is out of date"
        );
        if snapshot.revision == local.snapshot.revision {
            ensure!(
                snapshot == &local.snapshot,
                "Conflicting membership revision"
            );
            return Ok(());
        }
        let mut next = self.persisted.clone();
        next.workspaces
            .get_mut(&snapshot.workspace_id)
            .unwrap()
            .snapshot = snapshot.clone();
        next.audit(
            "membership-updated",
            snapshot.workspace_id,
            &snapshot.owner_peer_id,
        );
        self.persist(next)?;
        self.reconcile();
        self.emit();
        Ok(())
    }
    pub fn authorize(&mut self, peer: &str, incoming: &Snapshot) -> Result<Snapshot> {
        ensure!(
            self.persisted.active == Some(incoming.workspace_id),
            "Access denied"
        );
        incoming.verify()?;
        // New approved peers can present a newer owner-signed roster before authorization.
        if let Some(local) = self.persisted.workspaces.get(&incoming.workspace_id) {
            if incoming.revision >= local.snapshot.revision {
                self.accept_snapshot(incoming)?;
            }
        }
        let current = self.active_snapshot()?;
        ensure!(
            current.contains(peer) && current.contains(&self.peer()),
            "Access denied"
        );
        Ok(current)
    }
    pub fn reconcile(&mut self) {
        let allowed = self
            .active_snapshot()
            .ok()
            .filter(|s| s.contains(&self.peer()));
        self.remote.retain(|_, m| {
            allowed
                .as_ref()
                .is_some_and(|s| s.workspace_id == m.workspace_id && s.contains(&m.owner_peer_id))
        });
        self.online
            .retain(|p, _| allowed.as_ref().is_some_and(|s| s.contains(p)));
        for (id, transfer) in &self.transfers {
            if !allowed
                .as_ref()
                .is_some_and(|s| s.contains(&transfer.peer_id))
            {
                if let Some(token) = self.cancellations.get(id) {
                    token.cancel();
                }
            }
        }
    }
    pub fn unshare(&mut self, id: Uuid) -> Result<()> {
        ensure!(
            self.persisted.shares.contains_key(&id),
            "Shared file not found"
        );
        let mut next = self.persisted.clone();
        next.shares.remove(&id);
        self.persist(next)?;
        self.cancel_sharing(id);
        self.emit();
        Ok(())
    }
    pub fn cancel_sharing(&self, file_id: Uuid) {
        for (id, t) in &self.transfers {
            if t.file_id == file_id && t.direction == "Sharing" {
                if let Some(token) = self.cancellations.get(id) {
                    token.cancel();
                }
            }
        }
    }
    pub fn mark_transfer(&mut self, id: Uuid, status: &str, bytes: u64, error: Option<String>) {
        if let Some(t) = self.transfers.get_mut(&id) {
            t.status = status.into();
            t.bytes = bytes;
            t.error = error;
        }
        if ["Completed", "Cancelled", "Failed"].contains(&status) {
            self.cancellations.remove(&id);
        }
        self.emit();
    }
    pub fn cancel(&mut self, id: Uuid) -> Result<()> {
        self.cancellations
            .get(&id)
            .context("Transfer is no longer active")?
            .cancel();
        Ok(())
    }
    pub fn register_transfer(
        &mut self,
        manifest: &Manifest,
        peer: &str,
        direction: &str,
    ) -> (Uuid, CancellationToken) {
        // ponytail: retain at most 100 completed entries; persistent history only if users need it.
        if self.transfers.len() >= 100 {
            if let Some(id) = self
                .transfers
                .iter()
                .find(|(_, t)| ["Completed", "Failed", "Cancelled"].contains(&t.status.as_str()))
                .map(|(id, _)| *id)
            {
                self.transfers.remove(&id);
            }
        }
        let id = Uuid::new_v4();
        let token = CancellationToken::new();
        self.transfers.insert(
            id,
            Transfer {
                id,
                file_id: manifest.file_id,
                name: manifest.name.clone(),
                peer_id: peer.into(),
                direction: direction.into(),
                bytes: 0,
                total: manifest.size,
                status: "Queued".into(),
                error: None,
            },
        );
        self.cancellations.insert(id, token.clone());
        self.emit();
        (id, token)
    }
}

pub fn open_source(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options
        .open(path)
        .context("File is unavailable or in use")?;
    ensure!(
        file.metadata()?.is_file(),
        "Only regular files can be shared"
    );
    Ok(file)
}
pub fn stamp(file: &File) -> Result<String> {
    let meta = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}",
            meta.len(),
            meta.creation_time(),
            meta.last_write_time()
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(format!(
            "{}:{:?}:{:?}",
            meta.len(),
            meta.created().ok(),
            meta.modified()?
        ))
    }
}
pub fn index_file(path: &Path, workspace_id: Uuid, key: &Keypair) -> Result<LocalShare> {
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Symbolic links are not supported"
    );
    let path = std::fs::canonicalize(path)?;
    ensure!(
        !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "Symbolic links are not supported"
    );
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("Filename must be valid Unicode")?
        .to_string();
    validate_filename(&name)?;
    let mut file = open_source(&path)?;
    let initial = stamp(&file)?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0; BLOCK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        bytes += read as u64;
    }
    ensure!(
        stamp(&file)? == initial && file.metadata()?.len() == bytes,
        "File changed while preparing"
    );
    let mut manifest = Manifest {
        version: VERSION,
        file_id: Uuid::new_v4(),
        workspace_id,
        name,
        size: bytes,
        sha256: hex::encode(hash.finalize()),
        owner_peer_id: key.public().to_peer_id().to_string(),
        owner_public_key: public_key(key),
        signature: String::new(),
    };
    manifest.sign(key)?;
    Ok(LocalShare {
        manifest,
        path,
        stamp: initial,
        available: true,
    })
}
pub async fn share_paths(shared: Shared, paths: Vec<std::path::PathBuf>) -> Result<()> {
    let intended_workspace = shared.lock().await.active_snapshot()?.workspace_id;
    share_paths_to(shared, paths, intended_workspace).await
}
pub async fn share_paths_to(
    shared: Shared,
    paths: Vec<std::path::PathBuf>,
    intended_workspace: Uuid,
) -> Result<()> {
    ensure!(paths.len() <= 1000, "Share at most 1000 files at once");
    for path in paths {
        let (id, key, name) = {
            let mut e = shared.lock().await;
            let snapshot = e.active_snapshot()?;
            ensure!(snapshot.contains(&e.peer()), "You are no longer a member");
            ensure!(
                snapshot.workspace_id == intended_workspace,
                "Workspace changed while preparing; share remaining files again"
            );
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("File")
                .to_string();
            e.preparing.push(name.clone());
            e.emit();
            (snapshot.workspace_id, e.key.clone(), name)
        };
        let indexed = tokio::task::spawn_blocking(move || index_file(&path, id, &key)).await?;
        let mut e = shared.lock().await;
        if let Some(index) = e.preparing.iter().position(|n| n == &name) {
            e.preparing.remove(index);
        }
        match indexed {
            Ok(share)
                if e.persisted.active == Some(id) && e.active_snapshot()?.contains(&e.peer()) =>
            {
                let mut next = e.persisted.clone();
                next.shares
                    .retain(|_, s| s.path != share.path || s.manifest.workspace_id != id);
                next.shares.insert(share.manifest.file_id, share);
                e.persist(next)?;
            }
            Ok(_) => e.error("Workspace changed while preparing; share the file again"),
            Err(err) => e.error(format!("Could not prepare {name}: {err}")),
        }
        e.emit();
    }
    Ok(())
}

pub async fn refresh_share(shared: Shared, old: LocalShare) -> Result<()> {
    let (key, name) = {
        let mut e = shared.lock().await;
        if e.persisted.active != Some(old.manifest.workspace_id)
            || !e
                .persisted
                .shares
                .get(&old.manifest.file_id)
                .is_some_and(|s| s.manifest == old.manifest)
        {
            return Ok(());
        }
        ensure!(
            e.active_snapshot()?.contains(&e.peer()),
            "No longer a member"
        );
        let mut next = e.persisted.clone();
        next.shares
            .get_mut(&old.manifest.file_id)
            .unwrap()
            .available = false;
        e.persist(next)?;
        e.cancel_sharing(old.manifest.file_id);
        let name = old.manifest.name.clone();
        e.preparing.push(name.clone());
        e.emit();
        (e.key.clone(), name)
    };
    let path = old.path.clone();
    let workspace = old.manifest.workspace_id;
    let indexed = tokio::task::spawn_blocking(move || index_file(&path, workspace, &key)).await?;
    let mut e = shared.lock().await;
    if let Some(index) = e.preparing.iter().position(|n| n == &name) {
        e.preparing.remove(index);
    }
    if e.persisted.active == Some(workspace)
        && e.active_snapshot()?.contains(&e.peer())
        && e.persisted
            .shares
            .get(&old.manifest.file_id)
            .is_some_and(|s| s.manifest == old.manifest)
    {
        if let Ok(new) = indexed {
            let mut next = e.persisted.clone();
            next.shares.remove(&old.manifest.file_id);
            next.shares.insert(new.manifest.file_id, new);
            e.persist(next)?;
        }
    }
    e.emit();
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    CreateWorkspace { name: String },
    Activate { id: Uuid },
    SetName { name: String },
    CreateInvite,
    Join { invite: String },
    CancelJoin,
    Approve { peer_id: String },
    Reject { peer_id: String },
    RemoveMember { peer_id: String },
    Unshare { id: Uuid },
    CancelTransfer { id: Uuid },
    DismissError,
}
pub fn dispatch(engine: &mut Engine, action: Action) -> Result<()> {
    match action {
        Action::CreateWorkspace { name } => engine.create_workspace(name),
        Action::Activate { id } => engine.activate(id),
        Action::SetName { name } => engine.set_name(name),
        Action::CreateInvite => engine.create_invite().map(|_| ()),
        Action::Join { invite } => engine.join(&invite),
        Action::CancelJoin => engine.cancel_join(),
        Action::Approve { peer_id } => engine.approve(&peer_id),
        Action::Reject { peer_id } => engine.reject(&peer_id),
        Action::RemoveMember { peer_id } => engine.remove_member(&peer_id),
        Action::Unshare { id } => engine.unshare(id),
        Action::CancelTransfer { id } => engine.cancel(id),
        Action::DismissError => {
            engine.last_error = None;
            engine.emit();
            Ok(())
        }
    }
}
