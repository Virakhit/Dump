use crate::{
    connectivity::{peer_address, public_address, MAX_ADDRESSES},
    model::{decode_public, now, public_key},
};
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use libp2p::{identity::Keypair, Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub peer_id: String,
    pub public_key: String,
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub addresses: Vec<String>,
    pub signature: String,
}
impl Contact {
    fn payload(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(
            &(
                "dump/contact/1",
                &self.peer_id,
                &self.public_key,
                self.sequence,
                self.issued_at,
                self.expires_at,
                &self.addresses,
            ),
            &mut bytes,
        )?;
        Ok(bytes)
    }
    pub fn issue(key: &Keypair, sequence: u64, addresses: Vec<String>) -> Result<Self> {
        Self::issue_routes(key, sequence, addresses, false)
    }
    pub(crate) fn issue_routes(
        key: &Keypair,
        sequence: u64,
        addresses: Vec<String>,
        allow_loopback: bool,
    ) -> Result<Self> {
        let mut contact = Self {
            peer_id: key.public().to_peer_id().to_string(),
            public_key: public_key(key),
            sequence,
            issued_at: now(),
            expires_at: now().saturating_add(86400),
            addresses,
            signature: String::new(),
        };
        contact.signature = B64.encode(key.sign(&contact.payload()?)?);
        contact.verify_routes(allow_loopback)?;
        Ok(contact)
    }
    pub fn verify(&self) -> Result<()> {
        self.verify_routes(false)
    }
    pub(crate) fn verify_routes(&self, allow_loopback: bool) -> Result<()> {
        ensure!(
            self.peer_id.len() <= 128 && self.signature.len() <= 128 && self.sequence > 0,
            "Invalid contact"
        );
        ensure!(
            self.issued_at <= now().saturating_add(300)
                && self.expires_at > now()
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= 86400,
            "Contact expired or invalid lifetime"
        );
        ensure!(
            self.addresses.len() <= MAX_ADDRESSES,
            "Too many contact routes"
        );
        let peer: PeerId = self.peer_id.parse()?;
        let key = decode_public(&self.public_key)?;
        ensure!(
            key.to_peer_id() == peer && key.verify(&self.payload()?, &B64.decode(&self.signature)?),
            "Invalid contact signature"
        );
        let mut unique = std::collections::BTreeSet::new();
        for address in &self.addresses {
            ensure!(address.len() <= 512, "Contact address too long");
            let route = peer_address(peer, address.parse()?)?;
            let allowed = public_address(&route);
            // Only internal local simulation callers set this; public entry points use false.
            #[cfg(test)]
            let allowed = allowed
                || (allow_loopback
                    && matches!(route.iter().next(), Some(libp2p::multiaddr::Protocol::Ip4(ip)) if ip.is_loopback()));
            #[cfg(not(test))]
            let _ = allow_loopback;
            ensure!(
                allowed && unique.insert(route),
                "Nonpublic or duplicate contact route"
            );
        }
        Ok(())
    }
    pub fn routes(&self) -> Result<Vec<Multiaddr>> {
        self.routes_with_policy(false)
    }
    pub(crate) fn routes_with_policy(&self, allow_loopback: bool) -> Result<Vec<Multiaddr>> {
        self.verify_routes(allow_loopback)?;
        self.addresses.iter().map(|a| Ok(a.parse()?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_contacts_pin_identity_scope_routes_and_lifetime() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let valid = Contact::issue(&key, 1, vec!["/ip4/8.8.8.8/udp/1234/quic-v1".into()])?;
        let mut tampered = valid.clone();
        tampered.addresses[0] = "/ip4/1.1.1.1/tcp/1234".into();
        assert!(tampered.verify().is_err());
        tampered = valid.clone();
        tampered.peer_id = Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string();
        assert!(tampered.verify().is_err());
        tampered = valid;
        tampered.expires_at = now();
        assert!(tampered.verify().is_err());
        assert!(Contact::issue(&key, 1, vec!["/ip4/127.0.0.1/tcp/1234".into()]).is_err());
        assert!(Contact::issue(&key, 1, vec!["/dns4/example.org/tcp/1234".into()]).is_err());
        Ok(())
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn legacy_state_and_invites_keep_identity_and_contact_cache_is_member_scoped(
    ) -> Result<()> {
        use crate::{
            engine::Engine,
            model::{public_key, Invitation, Member, Persisted},
            network::{handle_control, Request},
        };
        use std::sync::Arc;
        let root = tempfile::tempdir()?;
        let shared = Engine::open(root.path(), Arc::new(|_| {}))?;
        shared.lock().await.create_workspace("Compatible".into())?;
        let invitation = Invitation::parse(&shared.lock().await.create_invite()?)?;
        assert!(invitation.contact.is_none());
        let member_key = Keypair::generate_ed25519();
        let member_peer = member_key.public().to_peer_id();
        handle_control(
            shared.clone(),
            member_peer,
            Request::Join {
                invitation: invitation.clone(),
                name: "Member".into(),
            },
        )
        .await?;
        let mut e = shared.lock().await;
        e.approve(&member_peer.to_string())?;
        let snapshot = e.active_snapshot()?;
        let current = Contact::issue(&member_key, 2, vec!["/ip4/8.8.8.8/tcp/4242".into()])?;
        e.cache_contacts(&snapshot, vec![current.clone()])?;
        e.cache_contacts(&snapshot, vec![Contact::issue(&member_key, 1, vec![])?])?;
        assert_eq!(e.persisted.contacts[&member_peer.to_string()], current);
        assert!(e
            .cache_contacts(
                &snapshot,
                vec![Contact::issue(
                    &member_key,
                    2,
                    vec!["/ip4/1.1.1.1/tcp/4242".into()]
                )?]
            )
            .is_err());
        let other = Keypair::generate_ed25519();
        assert!(e
            .cache_contacts(&snapshot, vec![Contact::issue(&other, 1, vec![])?])
            .is_err());
        e.update_contact(vec!["/ip4/8.8.8.8/tcp/4242".into()])?;
        let sequence = e.persisted.contacts[&e.peer()].sequence;
        let renewed = Invitation::parse(&e.create_invite()?)?.contact.unwrap();
        assert!(renewed.sequence > sequence);
        assert!(renewed.expires_at >= e.persisted.invites.last().unwrap().expires_at);
        let mut legacy = serde_json::to_value(&e.persisted)?;
        legacy.as_object_mut().unwrap().remove("network");
        legacy.as_object_mut().unwrap().remove("contacts");
        let restored: Persisted = serde_json::from_value(legacy)?;
        assert_eq!(restored.peer_id()?, e.peer());
        assert!(!restored.network.help_network && restored.contacts.is_empty());
        restored.workspaces[&snapshot.workspace_id]
            .snapshot
            .verify()?;
        let mut internet = invitation;
        internet.contact = Some(Contact::issue(
            &e.key,
            1,
            vec!["/ip4/8.8.8.8/tcp/4242".into()],
        )?);
        assert!(Invitation::parse(&internet.url()?)?.contact.is_some());
        internet.owner_peer_id = member_peer.to_string();
        assert!(Invitation::parse(&internet.url()?).is_err());
        // Signature domains remain distinct; a contact cannot substitute for a roster.
        let mut forged = snapshot;
        forged.owner_public_key = public_key(&other);
        forged.members.push(Member {
            peer_id: other.public().to_peer_id().to_string(),
            name: "Other".into(),
        });
        assert!(forged.verify().is_err());
        Ok(())
    }
}
