use anyhow::{ensure, Result};
use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};
use std::collections::BTreeMap;

pub const MAX_PEERS: usize = 256;
pub const MAX_ADDRESSES: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReachabilityStatus {
    #[default]
    Unknown,
    Public,
    Unreachable,
}

#[derive(Debug, Clone, Default)]
pub struct Reachability {
    pub status: ReachabilityStatus,
    pub public_addresses: Vec<Multiaddr>,
}

/// Local diagnostics only. A transport connection never implies workspace admission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Offline,
    Connecting,
    Direct,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialFailure {
    WrongPeer,
    Unreachable,
}

#[derive(Debug, Clone, Default)]
pub struct PeerDiagnostics {
    pub state: ConnectionState,
    pub identified: bool,
    pub last_failure: Option<DialFailure>,
    /// Reported by this peer, unverified; never advertised as a reachable address.
    pub observed_address: Option<Multiaddr>,
    pub advertised_addresses: Vec<Multiaddr>,
    pub hole_punch_succeeded: Option<bool>,
}

pub type Diagnostics = BTreeMap<PeerId, PeerDiagnostics>;

pub(crate) fn diagnostic(
    diagnostics: &mut Diagnostics,
    peer: PeerId,
) -> Option<&mut PeerDiagnostics> {
    if diagnostics.len() >= MAX_PEERS && !diagnostics.contains_key(&peer) {
        return None;
    }
    Some(diagnostics.entry(peer).or_default())
}

/// Literal-IP QUIC or authenticated TCP. Explicit callers may use LAN/loopback IPs.
/// Require a matching terminal Peer ID, or attach it at dial time for authentication.
pub fn direct_address(peer: PeerId, mut address: Multiaddr) -> Result<Multiaddr> {
    ensure!(address.to_vec().len() <= 256, "Address is too long");
    if let Some(Protocol::P2p(id)) = address.iter().last() {
        ensure!(id == peer, "Address belongs to another peer");
        address.pop();
    }
    let mut parts = address.iter();
    let valid_ip = match parts.next() {
        Some(Protocol::Ip4(ip)) => !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast(),
        Some(Protocol::Ip6(ip)) => !ip.is_unspecified() && !ip.is_multicast(),
        _ => false,
    };
    let transport = match parts.next() {
        Some(Protocol::Udp(port)) if port != 0 => matches!(parts.next(), Some(Protocol::QuicV1)),
        Some(Protocol::Tcp(port)) => port != 0,
        _ => false,
    };
    ensure!(
        valid_ip && transport && parts.next().is_none(),
        "Use a literal IP address with a nonzero QUIC UDP or TCP port"
    );
    Ok(address)
}

pub fn peer_address(peer: PeerId, mut address: Multiaddr) -> Result<Multiaddr> {
    ensure!(address.to_vec().len() <= 512, "Address is too long");
    if !is_relayed(&address) {
        return direct_address(peer, address);
    }
    if let Some(Protocol::P2p(id)) = address.iter().last() {
        ensure!(id == peer, "Address belongs to another peer");
        address.pop();
    }
    ensure!(
        matches!(address.pop(), Some(Protocol::P2pCircuit)),
        "Invalid circuit address"
    );
    let Some(Protocol::P2p(relay)) = address.pop() else {
        anyhow::bail!("Circuit needs a relay identity");
    };
    ensure!(relay != peer, "Relay cannot impersonate the destination");
    Ok(direct_address(relay, address)?
        .with(Protocol::P2p(relay))
        .with(Protocol::P2pCircuit))
}

pub fn is_relayed(address: &Multiaddr) -> bool {
    address.iter().any(|p| p == Protocol::P2pCircuit)
}

/// Conservative candidate filter, not proof of Internet reachability.
pub fn public_address(address: &Multiaddr) -> bool {
    match address.iter().next() {
        Some(Protocol::Ip4(ip)) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && a != 0
                && a < 240
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0)
        }
        Some(Protocol::Ip6(ip)) => {
            let segments = ip.segments();
            // Only global unicast; excludes mapped IPv4, ULA, link-local and documentation.
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && !(segments[0] == 0x2002)
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
        _ => false,
    }
}

#[derive(Default)]
pub(crate) struct AddressBook(BTreeMap<PeerId, Vec<Multiaddr>>);

impl AddressBook {
    pub fn contains(&self, peer: &PeerId) -> bool {
        self.0.contains_key(peer)
    }

    pub fn remember(&mut self, peer: PeerId, address: Multiaddr) -> Result<bool> {
        let address = peer_address(peer, address)?;
        ensure!(
            self.0.len() < MAX_PEERS || self.0.contains_key(&peer),
            "Peer address limit reached"
        );
        let addresses = self.0.entry(peer).or_default();
        if addresses.contains(&address) {
            return Ok(false);
        }
        ensure!(
            addresses.len() < MAX_ADDRESSES,
            "Peer address limit reached"
        );
        addresses.push(address);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> PeerId {
        libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id()
    }

    #[test]
    fn addresses_are_bounded_and_identity_scoped() -> Result<()> {
        let id = peer();
        let address: Multiaddr = "/ip4/127.0.0.1/udp/9000/quic-v1".parse()?;
        assert_eq!(direct_address(id, address.clone())?, address);
        assert_eq!(
            direct_address(id, address.clone().with(Protocol::P2p(id)))?,
            address
        );
        assert!(direct_address(id, address.clone().with(Protocol::P2p(peer()))).is_err());
        for value in [
            "/ip4/0.0.0.0/udp/9000/quic-v1",
            "/ip4/224.0.0.1/udp/9000/quic-v1",
            "/ip4/255.255.255.255/udp/9000/quic-v1",
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "/ip4/127.0.0.1/tcp/0",
            "/dns4/example.com/udp/9000/quic-v1",
            "/ip4/127.0.0.1/udp/9000/quic-v1/p2p-circuit",
        ] {
            assert!(direct_address(id, value.parse()?).is_err(), "{value}");
        }
        let mut book = AddressBook::default();
        assert!(book.remember(id, address.clone())?);
        assert!(!book.remember(id, address)?);
        for port in 1..MAX_ADDRESSES {
            book.remember(id, format!("/ip4/127.0.0.1/udp/{port}/quic-v1").parse()?)?;
        }
        assert!(book
            .remember(id, "/ip4/127.0.0.1/udp/8000/quic-v1".parse()?)
            .is_err());
        for _ in 1..MAX_PEERS {
            book.remember(peer(), "/ip4/127.0.0.1/udp/9000/quic-v1".parse()?)?;
        }
        assert!(book
            .remember(peer(), "/ip4/127.0.0.1/udp/9000/quic-v1".parse()?)
            .is_err());
        Ok(())
    }

    #[test]
    fn public_candidates_exclude_local_and_reserved_ranges() -> Result<()> {
        for ip in [
            "0.1.2.3",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "192.0.0.9",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.1.2.3",
            "255.255.255.255",
        ] {
            assert!(
                !public_address(&format!("/ip4/{ip}/udp/9000/quic-v1").parse()?),
                "{ip}"
            );
        }
        for ip in [
            "::",
            "::1",
            "::ffff:8.8.8.8",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(
                !public_address(&format!("/ip6/{ip}/udp/9000/quic-v1").parse()?),
                "{ip}"
            );
        }
        assert!(public_address(&"/ip4/8.8.8.8/udp/9000/quic-v1".parse()?));
        assert!(public_address(
            &"/ip6/2606:4700::1/udp/9000/quic-v1".parse()?
        ));
        Ok(())
    }

    #[test]
    fn circuit_addresses_bind_both_relay_and_destination() -> Result<()> {
        let owner = peer();
        let relay = peer();
        let direct: Multiaddr = "/ip4/127.0.0.1/tcp/9000".parse()?;
        let route = direct
            .clone()
            .with(Protocol::P2p(relay))
            .with(Protocol::P2pCircuit);
        assert_eq!(peer_address(owner, route.clone())?, route);
        assert_eq!(
            peer_address(owner, route.clone().with(Protocol::P2p(owner)))?,
            route
        );
        assert!(peer_address(owner, route.clone().with(Protocol::P2p(peer()))).is_err());
        assert!(peer_address(relay, route.clone()).is_err());
        assert!(peer_address(owner, direct.clone().with(Protocol::P2pCircuit)).is_err());
        assert!(peer_address(owner, route.with(Protocol::P2pCircuit)).is_err());
        assert_eq!(direct_address(owner, direct.clone())?, direct);
        Ok(())
    }
}
