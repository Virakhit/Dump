use libp2p::{multiaddr::Protocol, Multiaddr};
use std::{
    collections::BTreeSet,
    net::IpAddr,
    sync::{Arc, RwLock},
};

pub(crate) type SharedOverlayScope = Arc<RwLock<OverlayScope>>;

#[derive(Default)]
pub(crate) struct OverlayScope {
    pub local: BTreeSet<IpAddr>,
    pub remote: BTreeSet<IpAddr>,
    pub allow_inbound: bool,
    #[cfg(test)]
    pub allow_loopback_sources: bool,
}

impl OverlayScope {
    pub fn owns_listener(&self, address: &Multiaddr) -> bool {
        socket(address).is_some_and(|(ip, port)| self.local.contains(&ip) && port == 42043)
    }

    pub fn contains(&self, local: Option<&Multiaddr>, remote: &Multiaddr) -> bool {
        let Some((remote_ip, remote_port)) = socket(remote) else {
            return false;
        };
        if let Some(local) = local {
            let Some((local_ip, local_port)) = socket(local) else {
                return false;
            };
            // A shared-out machine may not have an outbound route to the recipient.
            // Incoming overlay sockets are still authenticated by libp2p, then by
            // workspace authorization. This grants no membership or catalog access.
            let overlay_source = match remote_ip {
                IpAddr::V4(ip) => ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]),
                IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
            };
            #[cfg(test)]
            let overlay_source =
                overlay_source || (self.allow_loopback_sources && remote_ip.is_loopback());
            self.local.contains(&local_ip)
                && local_port == 42043
                && (self.remote.contains(&remote_ip) || (self.allow_inbound && overlay_source))
        } else {
            !self.local.is_empty() && self.remote.contains(&remote_ip) && remote_port == 42043
        }
    }
}

pub(crate) fn socket(address: &Multiaddr) -> Option<(IpAddr, u16)> {
    if address.iter().any(|p| p == Protocol::P2pCircuit) {
        return None;
    }
    let mut parts = address.iter();
    let ip = match parts.next()? {
        Protocol::Ip4(ip) => IpAddr::V4(ip),
        Protocol::Ip6(ip) => IpAddr::V6(ip),
        _ => return None,
    };
    let port = match parts.next()? {
        Protocol::Tcp(port) => port,
        Protocol::Udp(port) if matches!(parts.next(), Some(Protocol::QuicV1)) => port,
        _ => return None,
    };
    (port != 0).then_some((ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_scope_needs_live_local_and_remote_routes_and_a_dedicated_listener() {
        let mut scope = OverlayScope::default();
        scope.local.insert("100.80.1.1".parse().unwrap());
        scope.remote.insert("100.90.2.3".parse().unwrap());
        let local: Multiaddr = "/ip4/100.80.1.1/tcp/42043".parse().unwrap();
        let incoming: Multiaddr = "/ip4/100.90.2.3/tcp/53123".parse().unwrap();
        assert!(scope.contains(Some(&local), &incoming));
        assert!(scope.contains(None, &"/ip4/100.90.2.3/udp/42043/quic-v1".parse().unwrap()));
        assert!(!scope.contains(Some(&local), &"/ip4/100.95.0.1/tcp/53000".parse().unwrap()));
        assert!(!scope.contains(None, &"/ip4/100.90.2.3/tcp/53123".parse().unwrap()));
        assert!(!scope.contains(
            Some(&"/ip4/192.168.1.3/tcp/42043".parse().unwrap()),
            &incoming
        ));
        scope.remote.clear();
        assert!(!scope.contains(Some(&local), &incoming));
        scope.allow_inbound = true;
        assert!(scope.contains(Some(&local), &incoming));
        assert!(!scope.contains(None, &"/ip4/100.90.2.3/tcp/42043".parse().unwrap()));
        assert!(!scope.contains(Some(&local), &"/ip4/192.168.1.2/tcp/53123".parse().unwrap()));
    }
}
