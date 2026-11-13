use crate::model::{now, public_key};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PREFIX: &str = "dump://tailscale/";
pub const PORT: u16 = 42043;
const LINK_VERSION: u8 = 1;
const MAX_LINK_BYTES: usize = 8192;
const CONTACT_LIFETIME: u64 = 86400;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OverlayContact {
    pub workspace_id: Uuid,
    pub peer_id: String,
    pub public_key: String,
    pub node_id: String,
    pub dns_name: String,
    pub udp_port: u16,
    pub tcp_port: u16,
    pub sequence: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub signature: String,
}

impl OverlayContact {
    fn payload(&self) -> impl Serialize + '_ {
        (
            "dump/overlay-contact/1",
            self.workspace_id,
            &self.peer_id,
            &self.public_key,
            &self.node_id,
            &self.dns_name,
            self.udp_port,
            self.tcp_port,
            self.sequence,
            self.issued_at,
            self.expires_at,
        )
    }

    pub fn issue(
        key: &Keypair,
        workspace_id: Uuid,
        node_id: String,
        dns_name: String,
        sequence: u64,
    ) -> Result<Self> {
        let issued_at = now();
        let mut contact = Self {
            workspace_id,
            peer_id: key.public().to_peer_id().to_string(),
            public_key: public_key(key),
            node_id,
            dns_name,
            udp_port: PORT,
            tcp_port: PORT,
            sequence,
            issued_at,
            expires_at: issued_at.saturating_add(CONTACT_LIFETIME),
            signature: String::new(),
        };
        let signature = crate::model::sign(key, &contact.payload())?;
        contact.signature = signature;
        contact.verify()?;
        Ok(contact)
    }

    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.peer_id.len() <= 128
                && self.sequence > 0
                && self.udp_port == PORT
                && self.tcp_port == PORT,
            "Invalid overlay contact"
        );
        let current = now();
        ensure!(
            self.issued_at <= current.saturating_add(300)
                && self.expires_at > current
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= CONTACT_LIFETIME,
            "Overlay contact expired or invalid lifetime"
        );
        ensure!(
            !self.node_id.is_empty()
                && self.node_id.len() <= 128
                && self
                    .node_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "Invalid Tailscale node identifier"
        );
        validate_dns_name(&self.dns_name)?;
        crate::model::verify(
            &self.public_key,
            &self.peer_id,
            &self.signature,
            &self.payload(),
        )
    }

    pub fn cache_key(&self) -> String {
        format!("{}:{}", self.workspace_id, self.peer_id)
    }
}

fn validate_dns_name(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 253 && value.is_ascii(),
        "Invalid Tailscale FQDN"
    );
    let name = value.strip_suffix('.').unwrap_or(value);
    ensure!(
        name.to_ascii_lowercase().ends_with(".ts.net"),
        "Use a Tailscale FQDN ending in .ts.net"
    );
    for label in name.split('.') {
        ensure!(
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid Tailscale FQDN label"
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OverlayLink {
    Invite {
        invitation: crate::model::Invitation,
        contact: OverlayContact,
    },
    Connect {
        contact: OverlayContact,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LinkEnvelope {
    Invite {
        version: u8,
        invitation: crate::model::Invitation,
        contact: OverlayContact,
    },
    Connect {
        version: u8,
        contact: OverlayContact,
    },
}

impl OverlayLink {
    pub fn url(&self) -> Result<String> {
        self.verify()?;
        let envelope = match self {
            Self::Invite {
                invitation,
                contact,
            } => LinkEnvelope::Invite {
                version: LINK_VERSION,
                invitation: invitation.clone(),
                contact: contact.clone(),
            },
            Self::Connect { contact } => LinkEnvelope::Connect {
                version: LINK_VERSION,
                contact: contact.clone(),
            },
        };
        let value = format!("{PREFIX}{}", B64.encode(serde_json::to_vec(&envelope)?));
        ensure!(value.len() <= MAX_LINK_BYTES, "Tailscale link is too long");
        Ok(value)
    }

    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= MAX_LINK_BYTES, "Tailscale link is too long");
        let payload = value
            .trim()
            .strip_prefix(PREFIX)
            .context("Use a dump://tailscale/ link")?;
        let envelope: LinkEnvelope = serde_json::from_slice(&B64.decode(payload)?)?;
        let (version, link) = match envelope {
            LinkEnvelope::Invite {
                version,
                invitation,
                contact,
            } => (
                version,
                Self::Invite {
                    invitation,
                    contact,
                },
            ),
            LinkEnvelope::Connect { version, contact } => (version, Self::Connect { contact }),
        };
        ensure!(
            version == LINK_VERSION,
            "Unsupported Tailscale link version"
        );
        link.verify()?;
        Ok(link)
    }

    fn verify(&self) -> Result<()> {
        match self {
            Self::Invite {
                invitation,
                contact,
            } => {
                contact.verify()?;
                crate::model::Invitation::parse(&invitation.url()?)?;
                ensure!(
                    invitation.workspace_id == contact.workspace_id
                        && invitation.owner_peer_id == contact.peer_id,
                    "Overlay contact does not belong to the invitation workspace and owner"
                );
                Ok(())
            }
            Self::Connect { contact } => contact.verify(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceNetworkPolicy {
    #[default]
    Legacy,
    TailscaleOnly,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{now, random_secret, Invitation, VERSION};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
    use serde_json::{json, Value};

    fn contact(key: &Keypair, workspace: Uuid) -> Result<OverlayContact> {
        OverlayContact::issue(
            key,
            workspace,
            "n0123456789abcdef".into(),
            "desktop.tail123.ts.net".into(),
            1,
        )
    }

    fn invitation(key: &Keypair, workspace: Uuid) -> Invitation {
        Invitation {
            version: VERSION,
            workspace_id: workspace,
            owner_peer_id: key.public().to_peer_id().to_string(),
            token: random_secret(),
            contact: None,
        }
    }

    fn url(value: Value) -> String {
        format!(
            "{PREFIX}{}",
            B64.encode(serde_json::to_vec(&value).unwrap())
        )
    }

    // Removing any signed field from the payload allows the corresponding mutation.
    #[test]
    fn signature_binds_every_contact_field_and_identity() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let valid = contact(&key, Uuid::new_v4())?;
        valid.verify()?;
        crate::model::verify(
            &valid.public_key,
            &valid.peer_id,
            &valid.signature,
            &(
                "dump/overlay-contact/1",
                valid.workspace_id,
                &valid.peer_id,
                &valid.public_key,
                &valid.node_id,
                &valid.dns_name,
                valid.udp_port,
                valid.tcp_port,
                valid.sequence,
                valid.issued_at,
                valid.expires_at,
            ),
        )?;
        let mutations = [
            ("workspace_id", json!(Uuid::new_v4())),
            (
                "peer_id",
                json!(Keypair::generate_ed25519()
                    .public()
                    .to_peer_id()
                    .to_string()),
            ),
            (
                "public_key",
                json!(crate::model::public_key(&Keypair::generate_ed25519())),
            ),
            ("node_id", json!("n9876543210fedcba")),
            ("dns_name", json!("other.tail123.ts.net")),
            ("udp_port", json!(42044)),
            ("tcp_port", json!(42044)),
            ("sequence", json!(2)),
            ("issued_at", json!(valid.issued_at - 1)),
            ("expires_at", json!(valid.expires_at - 1)),
            ("signature", json!(B64.encode([0; 64]))),
        ];
        for (field, value) in mutations {
            let mut altered = serde_json::to_value(&valid)?;
            altered[field] = value;
            let altered: OverlayContact = serde_json::from_value(altered)?;
            assert!(altered.verify().is_err(), "unsigned field: {field}");
        }
        let another = contact(&key, Uuid::new_v4())?;
        assert_ne!(valid.cache_key(), another.cache_key());
        assert_ne!(
            valid.cache_key(),
            contact(&Keypair::generate_ed25519(), valid.workspace_id)?.cache_key()
        );
        Ok(())
    }

    // Signed malformed records must fail structural checks even with a valid signature.
    #[test]
    fn signed_contacts_reject_expiry_lifetime_future_clock_and_zero_sequence() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let valid = contact(&key, Uuid::new_v4())?;
        let timestamp = now();
        for (issued, expires, sequence) in [
            (timestamp - 100, timestamp, 1),
            (timestamp, timestamp + 86401, 1),
            (timestamp + 600, timestamp + 86400, 1),
            (timestamp, timestamp, 1),
            (timestamp, timestamp + 100, 0),
            (u64::MAX, u64::MAX, 1),
        ] {
            let mut bad = valid.clone();
            bad.issued_at = issued;
            bad.expires_at = expires;
            bad.sequence = sequence;
            let signature = crate::model::sign(&key, &bad.payload())?;
            bad.signature = signature;
            assert!(bad.verify().is_err(), "{issued}/{expires}/{sequence}");
        }
        assert!(OverlayContact::issue(
            &key,
            valid.workspace_id,
            valid.node_id.clone(),
            valid.dns_name.clone(),
            0
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn contacts_accept_only_bounded_node_ids_and_tailscale_fqdns() -> Result<()> {
        let key = Keypair::generate_ed25519();
        for dns in [
            "",
            "ts.net",
            ".ts.net",
            "a..ts.net",
            "a.ts.net.evil.org",
            "a/evil.ts.net",
            "-a.ts.net",
            "a-.ts.net",
            "a_b.ts.net",
            "100.64.0.1",
            "fd7a:115c:a1e0::1",
            "a.ts.net:42043",
            "a.ts.net\n",
        ] {
            assert!(
                OverlayContact::issue(&key, Uuid::new_v4(), "n123".into(), dns.into(), 1).is_err(),
                "{dns:?}"
            );
        }
        for node in [
            String::new(),
            "x".repeat(129),
            "n123\n".into(),
            " n123".into(),
            "n/123".into(),
        ] {
            assert!(
                OverlayContact::issue(
                    &key,
                    Uuid::new_v4(),
                    node.clone(),
                    "a.tail.ts.net".into(),
                    1
                )
                .is_err(),
                "{node:?}"
            );
        }
        assert!(OverlayContact::issue(
            &key,
            Uuid::new_v4(),
            "n123".into(),
            format!("{}.ts.net", "x".repeat(64)),
            1
        )
        .is_err());
        for dns in ["a.tail.ts.net", "a.tail.ts.net.", "A.TAIL.TS.NET"] {
            OverlayContact::issue(&key, Uuid::new_v4(), "n123".into(), dns.into(), 1)?.verify()?;
        }
        contact(&key, Uuid::new_v4())?.verify()?;
        Ok(())
    }

    #[test]
    fn invite_and_connect_links_round_trip_without_changing_legacy_invites() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let workspace = Uuid::new_v4();
        let invite = invitation(&key, workspace);
        let legacy = invite.url()?;
        Invitation::parse(&legacy)?;
        let signed = contact(&key, workspace)?;
        for link in [
            OverlayLink::Invite {
                invitation: invite,
                contact: signed.clone(),
            },
            OverlayLink::Connect {
                contact: signed.clone(),
            },
        ] {
            let encoded = link.url()?;
            let wire: Value =
                serde_json::from_slice(&B64.decode(encoded.strip_prefix(PREFIX).unwrap())?)?;
            assert_eq!(wire["version"], 1);
            let decoded = OverlayLink::parse(&encoded)?;
            match decoded {
                OverlayLink::Invite {
                    invitation,
                    contact,
                } => {
                    assert_eq!(wire["kind"], "invite");
                    assert_eq!(invitation.url()?, legacy);
                    assert_eq!(contact, signed);
                }
                OverlayLink::Connect { contact } => {
                    assert_eq!(wire["kind"], "connect");
                    assert!(wire.get("invitation").is_none());
                    assert!(wire.get("token").is_none());
                    assert_eq!(contact, signed);
                }
            }
            assert!(Invitation::parse(&encoded).is_err());
        }
        assert!(OverlayLink::parse(&legacy).is_err());
        Ok(())
    }

    #[test]
    fn invite_rejects_different_workspace_owner_and_invalid_legacy_token() -> Result<()> {
        let key = Keypair::generate_ed25519();
        let workspace = Uuid::new_v4();
        let valid_invite = invitation(&key, workspace);
        let mut invalid = valid_invite.clone();
        invalid.token = "bad".into();
        let mut invalid_version = valid_invite.clone();
        invalid_version.version = 2;
        let mut invalid_owner = valid_invite.clone();
        invalid_owner.owner_peer_id = Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string();
        let mut invalid_workspace = valid_invite.clone();
        invalid_workspace.workspace_id = Uuid::new_v4();
        for invitation in [invalid, invalid_version, invalid_owner, invalid_workspace] {
            let link = OverlayLink::Invite {
                invitation,
                contact: contact(&key, workspace)?,
            };
            assert!(link.url().is_err());
            let mut wire = serde_json::to_value(link)?;
            wire["version"] = json!(1);
            assert!(OverlayLink::parse(&url(wire)).is_err());
        }
        let mut wrong_public = valid_invite;
        wrong_public.contact = Some(crate::contact::Contact::issue(
            &Keypair::generate_ed25519(),
            1,
            vec![],
        )?);
        assert!(OverlayLink::Invite {
            invitation: wrong_public,
            contact: contact(&key, workspace)?
        }
        .url()
        .is_err());
        Ok(())
    }

    #[test]
    fn links_reject_unknown_fields_versions_kinds_missing_signatures_and_oversize() -> Result<()> {
        let signed = contact(&Keypair::generate_ed25519(), Uuid::new_v4())?;
        let valid = json!({"version": 1, "kind": "connect", "contact": signed});
        for (field, value) in [
            ("version", json!(2)),
            ("kind", json!("join")),
            ("token", json!("secret")),
            ("invitation", json!({})),
        ] {
            let mut bad = valid.clone();
            bad[field] = value;
            assert!(OverlayLink::parse(&url(bad)).is_err(), "{field}");
        }
        let mut missing_version = valid.clone();
        missing_version.as_object_mut().unwrap().remove("version");
        assert!(OverlayLink::parse(&url(missing_version)).is_err());
        let mut bad_contact = valid.clone();
        bad_contact["contact"]["ip"] = json!("100.64.0.1");
        assert!(OverlayLink::parse(&url(bad_contact)).is_err());
        let mut bad_signature = valid;
        bad_signature["contact"]["signature"] = json!("");
        assert!(OverlayLink::parse(&url(bad_signature)).is_err());
        for malformed in [
            String::new(),
            PREFIX.into(),
            format!("{PREFIX}%%%"),
            format!("{PREFIX}{}", "x".repeat(8192)),
            format!(" {}", " ".repeat(8192)),
        ] {
            assert!(OverlayLink::parse(&malformed).is_err());
        }
        Ok(())
    }
}
