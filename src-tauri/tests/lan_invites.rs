use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use dump_core::{
    contact::Contact,
    lan_invite,
    model::{random_secret, Invitation, VERSION},
};
use libp2p::{identity::Keypair, PeerId};
use uuid::Uuid;

fn invitation() -> Invitation {
    Invitation {
        version: VERSION,
        workspace_id: Uuid::new_v4(),
        owner_peer_id: Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string(),
        token: random_secret(),
        contact: None,
    }
}

#[test]
fn lan_commitment_matches_a_stable_wire_vector() -> Result<()> {
    let peer_bytes = [&[0x12, 0x20][..], &[0x33; 32]].concat();
    let original = Invitation {
        version: VERSION,
        workspace_id: Uuid::from_bytes([0x11; 16]),
        owner_peer_id: PeerId::from_bytes(&peer_bytes)?.to_string(),
        token: B64.encode([0x22; 32]),
        contact: None,
    };
    assert_eq!(lan_invite::code(&original)?, "BUIkx78PoDQCy4rh");
    Ok(())
}

#[test]
fn lan_link_is_short_and_binds_all_join_fields() -> Result<()> {
    let original = invitation();
    let code = lan_invite::code(&original)?;
    let url = format!("{}{code}", lan_invite::PREFIX);
    assert_eq!(url.len(), 27);
    assert_eq!(B64.decode(&code)?.len(), 12);
    assert_eq!(lan_invite::parse_url(&format!(" \n{url}\t"))?, code);
    let peer: PeerId = original.owner_peer_id.parse()?;
    lan_invite::verify(&original, &code, peer)?;

    let mut changed = original.clone();
    changed.workspace_id = Uuid::new_v4();
    assert!(lan_invite::verify(&changed, &code, peer).is_err());
    changed = original.clone();
    changed.token = random_secret();
    assert!(lan_invite::verify(&changed, &code, peer).is_err());
    changed = original.clone();
    changed.owner_peer_id = invitation().owner_peer_id;
    assert!(lan_invite::verify(&changed, &code, changed.owner_peer_id.parse()?).is_err());
    assert!(lan_invite::verify(&original, &code, changed.owner_peer_id.parse()?).is_err());
    Ok(())
}

#[test]
fn every_token_byte_is_bound_by_the_link() -> Result<()> {
    let original = invitation();
    let code = lan_invite::code(&original)?;
    let token = B64.decode(&original.token)?;
    for index in 0..32 {
        let mut modified = token.clone();
        modified[index] ^= 1;
        let mut changed = original.clone();
        changed.token = B64.encode(modified);
        assert_ne!(lan_invite::code(&changed)?, code, "token byte {index}");
    }
    Ok(())
}

#[test]
fn lan_link_rejects_malformed_input() -> Result<()> {
    let valid = "dump://lan/AbCdEf123456GhIj";
    assert_eq!(lan_invite::parse_url(valid)?, "AbCdEf123456GhIj");
    for invalid in [
        "dump://lan/",
        "dump://lan/AbCdEf123456GhI",
        "dump://lan/AbCdEf123456GhIjj",
        "dump://lan/AbCdEf123456GhI=",
        "dump://lan/AbCdEf12/456GhIj",
        "dump://lan/AbCdEf12+456GhIj",
        "dump://lan/AbCdEf123456GhIj?x=1",
        "dump://lan/AbCdEf123456GhIj#x",
        "dump://join/AbCdEf123456GhIj",
        "https://example.com/AbCdEf123456GhIj",
        "dump://lan/กขคงจฉชซฌญฎฏฐฑฒณ",
    ] {
        assert!(lan_invite::parse_url(invalid).is_err(), "{invalid}");
    }
    assert!(lan_invite::parse_url(&format!("dump://lan/{}", "A".repeat(8192))).is_err());
    Ok(())
}

#[test]
fn lan_commitment_rejects_invalid_or_internet_invitations() -> Result<()> {
    let original = invitation();
    let mut invalid = original.clone();
    invalid.version = VERSION + 1;
    assert!(lan_invite::code(&invalid).is_err());
    invalid = original.clone();
    invalid.owner_peer_id = "not-a-peer-id".into();
    assert!(lan_invite::code(&invalid).is_err());
    for token in [B64.encode([0; 31]), B64.encode([0; 33]), "invalid!".into()] {
        invalid = original.clone();
        invalid.token = token;
        assert!(lan_invite::code(&invalid).is_err());
    }
    invalid = original;
    let key = Keypair::generate_ed25519();
    invalid.owner_peer_id = key.public().to_peer_id().to_string();
    invalid.contact = Some(Contact::issue(&key, 1, vec![])?);
    assert!(lan_invite::code(&invalid).is_err());
    Ok(())
}

#[test]
fn existing_full_invites_stay_compatible() -> Result<()> {
    let mut original = invitation();
    let url = original.url()?;
    assert!(url.starts_with("dump://join/"));
    assert_eq!(
        serde_json::to_value(Invitation::parse(&url)?)?,
        serde_json::to_value(&original)?
    );

    let key = Keypair::generate_ed25519();
    original.owner_peer_id = key.public().to_peer_id().to_string();
    original.contact = Some(Contact::issue(
        &key,
        1,
        vec!["/ip4/8.8.8.8/udp/4242/quic-v1".into()],
    )?);
    assert_eq!(
        serde_json::to_value(Invitation::parse(&original.url()?)?)?,
        serde_json::to_value(&original)?
    );
    original.owner_peer_id = invitation().owner_peer_id;
    assert!(Invitation::parse(&original.url()?).is_err());
    Ok(())
}
