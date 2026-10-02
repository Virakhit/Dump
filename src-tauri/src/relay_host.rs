//! The opt-in infrastructure role has no Engine, workspace, file or disk API.
use crate::connectivity::{direct_address, public_address};
use anyhow::{ensure, Result};
use futures::{AsyncRead, AsyncWrite, StreamExt};
use libp2p::{
    connection_limits,
    core::{
        muxing::{StreamMuxerBox, StreamMuxerEvent},
        upgrade, StreamMuxer, Transport,
    },
    identity::Keypair,
    noise, quic, relay,
    swarm::{
        dummy, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub const PORT: u16 = 42042;
pub const BANDWIDTH: usize = 2 * 1024 * 1024;
const BURST: usize = 64 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub help_network: bool,
    pub public_addresses: Vec<String>,
    pub relays: Vec<String>,
}
impl Settings {
    pub fn validate(&self, peer: PeerId) -> Result<()> {
        ensure!(
            self.public_addresses.len() <= 2 && self.relays.len() <= 3,
            "Too many network addresses"
        );
        for address in &self.public_addresses {
            ensure!(address.len() <= 512, "Address too long");
            let address = direct_address(peer, address.parse()?)?;
            ensure!(
                public_address(&address),
                "Use a public literal IP address with a nonzero port"
            );
        }
        for address in &self.relays {
            relay_address(address)?;
        }
        Ok(())
    }
}
pub fn relay_address(value: &str) -> Result<(PeerId, Multiaddr)> {
    ensure!(value.len() <= 512, "Address too long");
    let address: Multiaddr = value.parse()?;
    let Some(libp2p::multiaddr::Protocol::P2p(peer)) = address.iter().last() else {
        anyhow::bail!("Relay address must end with its Peer ID");
    };
    let address = direct_address(peer, address)?;
    ensure!(
        public_address(&address),
        "Configured relay must use a public literal IP address"
    );
    Ok((peer, address))
}
pub fn config() -> relay::Config {
    relay::Config {
        max_reservations: 16,
        max_reservations_per_peer: 1,
        reservation_duration: Duration::from_secs(600),
        max_circuits: 8,
        max_circuits_per_peer: 2,
        max_circuit_duration: Duration::from_secs(600),
        max_circuit_bytes: 256 * 1024 * 1024,
        // Keep the library's per-IP and per-Peer ID request token buckets.
        ..Default::default()
    }
}

/// A shared read + write application-byte bucket across all host-role connections.
struct Bucket {
    bytes: usize,
    at: Instant,
}
type Budget = Arc<Mutex<Bucket>>;
struct Throttled<S> {
    inner: S,
    budget: Budget,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl<S> Throttled<S> {
    fn grant(&mut self, cx: &mut Context<'_>, wanted: usize) -> Poll<usize> {
        if wanted == 0 {
            return Poll::Ready(0);
        }
        let mut bucket = self.budget.lock().unwrap_or_else(|e| e.into_inner());
        let refill = (bucket.at.elapsed().as_nanos() * BANDWIDTH as u128 / 1_000_000_000)
            .min(BURST as u128) as usize;
        if refill != 0 {
            bucket.bytes = (bucket.bytes + refill).min(BURST);
            bucket.at = Instant::now();
        }
        if bucket.bytes != 0 {
            let grant = wanted.min(bucket.bytes);
            bucket.bytes -= grant;
            self.sleep = None;
            return Poll::Ready(grant);
        }
        drop(bucket);
        let sleep = self
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_millis(1))));
        if sleep.as_mut().poll(cx).is_ready() {
            self.sleep = None;
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
    fn refund(&self, grant: usize, result: &Poll<io::Result<usize>>) {
        let used = if let Poll::Ready(Ok(n)) = result {
            *n
        } else {
            0
        };
        let mut bucket = self.budget.lock().unwrap_or_else(|e| e.into_inner());
        bucket.bytes = (bucket.bytes + grant.saturating_sub(used)).min(BURST);
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for Throttled<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let grant = futures::ready!(this.grant(cx, bytes.len()));
        let result = Pin::new(&mut this.inner).poll_read(cx, &mut bytes[..grant]);
        this.refund(grant, &result);
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for Throttled<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let grant = futures::ready!(this.grant(cx, bytes.len()));
        let result = Pin::new(&mut this.inner).poll_write(cx, &bytes[..grant]);
        this.refund(grant, &result);
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}
struct Mux {
    inner: StreamMuxerBox,
    budget: Budget,
}
impl StreamMuxer for Mux {
    type Substream = Throttled<libp2p::core::muxing::SubstreamBox>;
    type Error = io::Error;
    fn poll_inbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Self::Substream>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner)
            .poll_inbound(cx)
            .map_ok(|inner| Throttled {
                inner,
                budget: this.budget.clone(),
                sleep: None,
            })
    }
    fn poll_outbound(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Self::Substream>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner)
            .poll_outbound(cx)
            .map_ok(|inner| Throttled {
                inner,
                budget: this.budget.clone(),
                sleep: None,
            })
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<StreamMuxerEvent>> {
        Pin::new(&mut self.get_mut().inner).poll(cx)
    }
}

#[derive(Default)]
pub(crate) struct Admission(VecDeque<Instant>);
impl NetworkBehaviour for Admission {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = std::convert::Infallible;
    fn handle_pending_inbound_connection(
        &mut self,
        _: ConnectionId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        let now = Instant::now();
        self.0
            .retain(|at| now.duration_since(*at) < Duration::from_secs(60));
        if self.0.len() >= 60 {
            return Err(ConnectionDenied::new(io::Error::other(
                "Host connection rate limit",
            )));
        }
        self.0.push_back(now);
        Ok(())
    }
    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }
    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: libp2p::core::Endpoint,
        _: libp2p::core::transport::PortUse,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }
    fn on_swarm_event(&mut self, _: FromSwarm) {}
    fn on_connection_handler_event(
        &mut self,
        _: PeerId,
        _: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        libp2p::core::util::unreachable(event);
    }
    fn poll(&mut self, _: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}
#[derive(NetworkBehaviour)]
pub(crate) struct Host {
    admission: Admission,
    limits: connection_limits::Behaviour,
    relay: relay::Behaviour,
    identify: crate::identify::Identify,
}
pub(crate) fn swarm(key: Keypair, cfg: relay::Config) -> Result<Swarm<Host>> {
    let peer = key.public().to_peer_id();
    let tcp = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true))
        .upgrade(upgrade::Version::V1Lazy)
        .authenticate(noise::Config::new(&key)?)
        .multiplex(yamux::Config::default())
        .map(|(p, mux), _| (p, StreamMuxerBox::new(mux)));
    let quic = quic::tokio::Transport::new(quic::Config::new(&key))
        .map(|(p, mux), _| (p, StreamMuxerBox::new(mux)));
    let budget = Arc::new(Mutex::new(Bucket {
        bytes: BURST,
        at: Instant::now(),
    }));
    let transport = quic
        .or_transport(tcp)
        .map(|either, _| either.into_inner())
        .map(move |(p, inner), _| {
            (
                p,
                StreamMuxerBox::new(Mux {
                    inner,
                    budget: budget.clone(),
                }),
            )
        })
        .boxed();
    Ok(Swarm::new(
        transport,
        Host {
            admission: Admission::default(),
            limits: connection_limits::Behaviour::new(
                connection_limits::ConnectionLimits::default()
                    .with_max_pending_incoming(Some(8))
                    .with_max_pending_outgoing(Some(8))
                    .with_max_established(Some(32))
                    .with_max_established_per_peer(Some(2)),
            ),
            relay: relay::Behaviour::new(peer, cfg),
            identify: crate::identify::Identify::new(
                libp2p::identify::Config::new("/dump/relay/1".into(), key.public())
                    .with_hide_listen_addrs(true)
                    .with_cache_size(0),
            ),
        },
        peer,
        libp2p::swarm::Config::with_tokio_executor()
            .with_idle_connection_timeout(Duration::from_secs(60)),
    ))
}
pub(crate) fn start(key: Keypair, settings: &Settings, shutdown: CancellationToken) -> Result<()> {
    if !settings.help_network {
        return Ok(());
    }
    let peer = key.public().to_peer_id();
    settings.validate(peer)?;
    let mut swarm = swarm(key, config())?;
    swarm.listen_on(format!("/ip4/0.0.0.0/udp/{PORT}/quic-v1").parse()?)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{PORT}").parse()?)?;
    for address in &settings.public_addresses {
        swarm.add_external_address(direct_address(peer, address.parse()?)?);
    }
    tokio::spawn(async move {
        loop {
            tokio::select! { _ = shutdown.cancelled() => break, _ = swarm.select_next_some() => {} }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncWriteExt;
    #[tokio::test]
    async fn shared_bandwidth_and_connection_rate_are_bounded() -> Result<()> {
        let budget = Arc::new(Mutex::new(Bucket {
            bytes: BURST,
            at: Instant::now(),
        }));
        let mut a = Throttled {
            inner: futures::io::sink(),
            budget: budget.clone(),
            sleep: None,
        };
        let mut b = Throttled {
            inner: futures::io::sink(),
            budget,
            sleep: None,
        };
        let bytes = vec![0; BANDWIDTH / 2];
        let at = Instant::now();
        futures::try_join!(a.write_all(&bytes), b.write_all(&bytes))?;
        assert!(
            at.elapsed() >= Duration::from_millis(900),
            "Aggregate bucket must charge both streams"
        );
        let mut admission = Admission::default();
        let address = "/ip4/127.0.0.1/tcp/42".parse()?;
        for _ in 0..60 {
            admission
                .handle_pending_inbound_connection(
                    ConnectionId::new_unchecked(1),
                    &address,
                    &address,
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        assert!(admission
            .handle_pending_inbound_connection(ConnectionId::new_unchecked(2), &address, &address)
            .is_err());
        assert_eq!(admission.0.len(), 60);
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_rejects_excess_reservations_and_stops_over_budget_payload() -> Result<()> {
        use crate::{
            engine::{share_paths, Engine},
            model::BLOCK,
            network::Node,
        };
        use libp2p::swarm::SwarmEvent;
        let key = Keypair::generate_ed25519();
        let relay_peer = key.public().to_peer_id();
        let mut cfg = config();
        cfg.max_reservations = 1;
        cfg.max_circuits = 1;
        cfg.max_circuit_bytes = 128 * 1024;
        let mut relay = swarm(key, cfg)?;
        relay.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = relay.select_next_some().await {
                break address;
            }
        };
        relay.add_external_address(address.clone()); // Isolated test, never shipping advertisement.
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let (denied_tx, mut denied) = tokio::sync::watch::channel(false);
        let host = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_stop.cancelled() => break,
                    event = relay.select_next_some() => {
                        if matches!(event, SwarmEvent::Behaviour(HostEvent::Relay(relay::Event::ReservationReqDenied { .. }))) { denied_tx.send_replace(true); }
                    }
                }
            }
        });
        let root = tempfile::tempdir()?;
        let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
        let member = Engine::open(&root.path().join("member"), Arc::new(|_| {}))?;
        let outsider = Engine::open(&root.path().join("outsider"), Arc::new(|_| {}))?;
        owner.lock().await.create_workspace("Limits".into())?;
        let invite = owner.lock().await.create_invite()?;
        member.lock().await.join(&invite)?;
        let a = Node::start(owner.clone(), false).await?;
        let b = Node::start(member.clone(), false).await?;
        let c = Node::start(outsider, false).await?;
        a.reserve_relay(relay_peer, address.clone()).await?;
        let mut routes = a.relay_addresses.clone();
        let route = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(address) = routes.borrow().first().cloned() {
                    break Ok::<_, anyhow::Error>(address);
                }
                routes.changed().await?;
            }
        })
        .await??;
        c.reserve_relay(relay_peer, address).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !*denied.borrow() {
                denied.changed().await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        assert!(c.relay_addresses.borrow().is_empty());
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        let member_peer = member.lock().await.peer();
        b.connect(owner_peer, route).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if owner.lock().await.pending.contains_key(&member_peer) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        owner.lock().await.approve(&member_peer)?;
        tokio::time::timeout(Duration::from_secs(15), async {
            while member.lock().await.persisted.joining.is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let source = root.path().join("limited.bin");
        tokio::fs::write(&source, vec![7; BLOCK]).await?;
        share_paths(owner.clone(), vec![source]).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            while member.lock().await.remote.is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let file = member.lock().await.remote.values().next().unwrap().file_id;
        let output = root.path().join("output");
        tokio::fs::create_dir(&output).await?;
        let id = b.receive(file, output.clone()).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let e = member.lock().await;
                if e.transfers
                    .get(&id)
                    .is_some_and(|t| ["Failed", "Cancelled"].contains(&t.status.as_str()))
                {
                    break;
                }
                drop(e);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        assert!(tokio::fs::read_dir(&output)
            .await?
            .next_entry()
            .await?
            .is_none());
        assert!(member.lock().await.persisted.partials.is_empty());
        a.shutdown.cancel();
        b.shutdown.cancel();
        c.shutdown.cancel();
        stop.cancel();
        host.await?;
        Ok(())
    }
}
