use libp2p::{core::transport::ListenerId, Multiaddr};
use std::collections::{BTreeSet, HashMap};

#[derive(Default)]
pub(crate) struct LiveListeners {
    by_id: HashMap<ListenerId, BTreeSet<Multiaddr>>,
}

impl LiveListeners {
    pub(crate) fn add(&mut self, id: ListenerId, address: Multiaddr) {
        self.by_id.entry(id).or_default().insert(address);
    }

    pub(crate) fn expire(&mut self, id: ListenerId, address: &Multiaddr) {
        if let Some(addresses) = self.by_id.get_mut(&id) {
            addresses.remove(address);
            if addresses.is_empty() {
                self.by_id.remove(&id);
            }
        }
    }

    pub(crate) fn close(&mut self, id: ListenerId) {
        self.by_id.remove(&id);
    }

    pub(crate) fn addresses(&self) -> Vec<Multiaddr> {
        self.by_id
            .values()
            .flat_map(|addresses| addresses.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp() -> Multiaddr {
        "/ip4/100.80.1.1/tcp/42043".parse().unwrap()
    }

    fn quic() -> Multiaddr {
        "/ip4/100.80.1.1/udp/42043/quic-v1".parse().unwrap()
    }

    fn assert_both_transports_live(listeners: &LiveListeners) {
        let addresses = listeners.addresses();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&tcp()));
        assert!(addresses.contains(&quic()));
    }

    #[test]
    fn delayed_old_listener_events_preserve_replacement_tcp_and_quic_readiness() {
        let old_tcp = ListenerId::next();
        let old_quic = ListenerId::next();
        let new_tcp = ListenerId::next();
        let new_quic = ListenerId::next();
        let mut listeners = LiveListeners::default();
        listeners.add(old_tcp, tcp());
        listeners.add(old_quic, quic());
        listeners.add(new_tcp, tcp());
        listeners.add(new_quic, quic());
        assert_both_transports_live(&listeners);

        listeners.expire(old_tcp, &tcp());
        listeners.close(old_quic);
        assert_both_transports_live(&listeners);
        listeners.close(old_tcp);
        listeners.expire(old_quic, &quic());
        assert_both_transports_live(&listeners);

        listeners.close(new_tcp);
        assert_eq!(listeners.addresses(), vec![quic()]);
        listeners.close(new_quic);
        assert!(listeners.addresses().is_empty());
    }

    #[test]
    fn late_expiry_after_replacement_snapshot_cannot_erase_new_owner_or_duplicate_addresses() {
        let old = ListenerId::next();
        let replacement = ListenerId::next();
        let mut listeners = LiveListeners::default();
        listeners.add(old, tcp());
        assert_eq!(listeners.addresses(), vec![tcp()]);
        listeners.close(old);
        listeners.add(replacement, tcp());
        listeners.add(replacement, tcp());
        let current_snapshot = listeners.addresses();
        assert_eq!(current_snapshot, vec![tcp()]);
        listeners.expire(old, &tcp());
        listeners.close(old);
        assert_eq!(listeners.addresses(), current_snapshot);
        listeners.expire(replacement, &tcp());
        assert!(listeners.addresses().is_empty());
    }

    #[test]
    fn expiring_one_address_preserves_other_addresses_of_the_same_listener() {
        let id = ListenerId::next();
        let unknown = ListenerId::next();
        let mut listeners = LiveListeners::default();
        listeners.add(id, tcp());
        listeners.add(id, quic());
        listeners.add(id, tcp());
        let addresses = listeners.addresses();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.windows(2).all(|pair| pair[0] < pair[1]));
        listeners.expire(unknown, &tcp());
        listeners.expire(id, &"/ip4/100.95.1.1/tcp/42043".parse().unwrap());
        assert_eq!(listeners.addresses(), addresses);
        listeners.expire(id, &tcp());
        assert_eq!(listeners.addresses(), vec![quic()]);
        listeners.close(id);
        assert!(listeners.addresses().is_empty());
    }
}
