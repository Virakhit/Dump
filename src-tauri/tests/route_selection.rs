use dump_core::{route_selection::DialPlanner, tailscale::parse_status};
use libp2p::{identity::Keypair, Multiaddr};
use std::time::{Duration, Instant};
#[test]
fn full_pending_ticks_do_not_alias_three_peer_rotation_with_six_second_backoff() {
    let peers: Vec<_> = (0..3)
        .map(|_| Keypair::generate_ed25519().public().to_peer_id())
        .collect();
    let candidates = vec![
        "/ip4/100.80.1.2/udp/1/quic-v1".parse().unwrap(),
        "/ip4/100.80.1.2/udp/2/quic-v1".parse().unwrap(),
    ];
    let mut planner = DialPlanner::default();
    let at = Instant::now();
    let mut pending = Vec::<(libp2p::PeerId, u64)>::new();
    let mut seen = std::collections::BTreeSet::new();
    for seconds in (0..=18).step_by(2) {
        pending.retain(|(_, started)| seconds - *started < 5);
        let mut budget = 4usize - pending.len();
        for peer in planner.order(&peers, None, budget) {
            let peer_budget =
                2usize.saturating_sub(pending.iter().filter(|(id, _)| *id == peer).count());
            let scheduled = planner.plan(
                peer,
                &candidates,
                false,
                budget.min(peer_budget),
                at + Duration::from_secs(seconds),
            );
            budget -= scheduled.len();
            for _ in scheduled {
                pending.push((peer, seconds));
                seen.insert(peer);
            }
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "the third target must receive a dial despite continuous failures"
    );
}
#[test]
fn failed_peers_cannot_starve_later_targets_or_an_invitation_owner() {
    let peers: Vec<_> = (0..4)
        .map(|_| Keypair::generate_ed25519().public().to_peer_id())
        .collect();
    let candidates = vec!["/ip4/100.80.1.2/udp/123/quic-v1".parse().unwrap()];
    let mut planner = DialPlanner::default();
    let at = Instant::now();
    let mut attempted = std::collections::BTreeSet::new();
    for round in 0..4 {
        let mut budget = 2;
        for peer in planner.order(&peers, None, budget) {
            let scheduled = planner.plan(
                peer,
                &candidates,
                false,
                budget,
                at + Duration::from_secs(round * 6),
            );
            budget -= scheduled.len();
            if !scheduled.is_empty() {
                attempted.insert(peer);
            }
        }
    }
    assert_eq!(attempted.len(), peers.len());
    assert_eq!(planner.order(&peers, Some(peers[3]), 4)[0], peers[3]);
}
#[test]
fn bounded_rounds_fall_back_without_claiming_or_redialing_an_existing_connection() {
    let peer = Keypair::generate_ed25519().public().to_peer_id();
    let candidates: Vec<Multiaddr> = (1..=4)
        .map(|port| {
            format!("/ip4/100.80.1.2/udp/{port}/quic-v1")
                .parse()
                .unwrap()
        })
        .collect();
    let mut planner = DialPlanner::default();
    let at = Instant::now();
    assert_eq!(
        planner.plan(peer, &candidates, false, 4, at),
        candidates[..2]
    );
    assert!(planner
        .plan(peer, &candidates, false, 4, at + Duration::from_secs(2))
        .is_empty());
    assert_eq!(
        planner.plan(peer, &candidates, false, 4, at + Duration::from_secs(6)),
        candidates[2..]
    );
    assert!(planner
        .plan(peer, &candidates, true, 4, at + Duration::from_secs(12))
        .is_empty());
}
#[test]
fn only_running_local_self_overlay_ips_are_exportable() {
    let ready = br#"{"BackendState":"Running","Self":{"Online":true,"TailscaleIPs":["100.80.1.2","192.168.1.2","8.8.8.8","fd7a:115c:a1e0::2"]},"Peer":{"secret":{"TailscaleIPs":["100.80.9.9"]}}}"#;
    let ips = parse_status(ready);
    assert_eq!(ips.len(), 2);
    assert!(!ips.iter().any(|ip| ip.to_string() == "100.80.9.9"));
    assert!(parse_status(
        br#"{"BackendState":"NeedsLogin","Self":{"Online":true,"TailscaleIPs":["100.80.1.2"]}}"#
    )
    .is_empty());
    assert!(parse_status(
        br#"{"BackendState":"Running","Self":{"Online":false,"TailscaleIPs":["100.80.1.2"]}}"#
    )
    .is_empty());
}
