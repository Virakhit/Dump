//! Bounded candidate rounds. Scheduling a dial is never reported as a connection.
use libp2p::{Multiaddr, PeerId};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct DialPlanner {
    cursor: usize,
    peers: BTreeMap<PeerId, (Instant, usize)>,
}
impl DialPlanner {
    pub fn order(
        &mut self,
        peers: &[PeerId],
        priority: Option<PeerId>,
        capacity: usize,
    ) -> Vec<PeerId> {
        if peers.is_empty() || capacity == 0 {
            return Vec::new();
        }
        let mut ordered = peers.to_vec();
        ordered.rotate_left(self.cursor % peers.len());
        self.cursor = (self.cursor + 1) % peers.len();
        if let Some(peer) = priority {
            if let Some(index) = ordered.iter().position(|id| *id == peer) {
                ordered.remove(index);
                ordered.insert(0, peer);
            }
        }
        ordered
    }
    pub fn plan(
        &mut self,
        peer: PeerId,
        candidates: &[Multiaddr],
        connected: bool,
        budget: usize,
        at: Instant,
    ) -> Vec<Multiaddr> {
        if connected {
            self.peers.remove(&peer);
            return Vec::new();
        }
        if candidates.is_empty() || budget == 0 {
            return Vec::new();
        }
        if self
            .peers
            .get(&peer)
            .is_some_and(|(last, _)| at.saturating_duration_since(*last) < Duration::from_secs(6))
        {
            return Vec::new();
        }
        if self.peers.len() >= 129 && !self.peers.contains_key(&peer) {
            return Vec::new();
        }
        let cursor = self.peers.get(&peer).map_or(0, |(_, cursor)| *cursor);
        let count = budget.min(2).min(candidates.len());
        let result = (0..count)
            .map(|index| candidates[(cursor + index) % candidates.len()].clone())
            .collect();
        self.peers
            .insert(peer, (at, (cursor + count) % candidates.len()));
        result
    }
    pub fn retain(&mut self, desired: &[PeerId]) {
        self.peers.retain(|peer, _| desired.contains(peer));
    }
}
