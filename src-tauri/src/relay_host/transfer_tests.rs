use super::*;
use crate::{
    connectivity::ConnectionState,
    engine::{share_paths, Engine, Shared, View},
    model::Member,
    network::Node,
};
use anyhow::{bail, Context};
use libp2p::swarm::SwarmEvent;
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Default)]
struct HostState {
    circuits: usize,
    reservation_denied: bool,
    closed: Vec<(io::ErrorKind, String)>,
}

fn engine(path: &std::path::Path) -> Result<(Shared, watch::Receiver<Option<View>>)> {
    let (sender, receiver) = watch::channel(None);
    Ok((
        Engine::open(
            path,
            Arc::new(move |view| {
                sender.send_replace(Some(view));
            }),
        )?,
        receiver,
    ))
}

struct Session {
    root: tempfile::TempDir,
    owner: Shared,
    member: Shared,
    updates: watch::Receiver<Option<View>>,
    nodes: Vec<Node>,
    stop: CancellationToken,
    host: Option<tokio::task::JoinHandle<()>>,
    host_state: watch::Receiver<HostState>,
    relay: Option<(PeerId, Multiaddr)>,
}

impl Drop for Session {
    fn drop(&mut self) {
        for node in &self.nodes {
            node.shutdown.cancel();
        }
        self.stop.cancel();
        if let Some(host) = &self.host {
            host.abort();
        }
    }
}

impl Session {
    async fn start(cfg: relay::Config, direct: bool) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let (owner, _) = engine(&root.path().join("owner"))?;
        let (member, updates) = engine(&root.path().join("member"))?;
        owner
            .lock()
            .await
            .create_workspace("Circuit limits".into())?;
        // Seed an owner-signed approved roster to isolate circuit limits from the
        // invitation timing covered by the Internet invitation regression.
        let member_peer = member.lock().await.peer();
        let workspace = {
            let mut e = owner.lock().await;
            let mut next = e.persisted.clone();
            let workspace = next.workspaces.get_mut(&next.active.unwrap()).unwrap();
            workspace.snapshot.members.push(Member {
                peer_id: member_peer,
                name: "Member".into(),
            });
            workspace.snapshot.revision += 1;
            workspace.snapshot.sign(&e.key)?;
            let workspace = workspace.clone();
            e.persist(next)?;
            workspace
        };
        {
            let mut e = member.lock().await;
            let mut next = e.persisted.clone();
            next.active = Some(workspace.snapshot.workspace_id);
            next.workspaces
                .insert(workspace.snapshot.workspace_id, workspace);
            e.persist(next)?;
        }
        let (host_updates, host_state) = watch::channel(HostState::default());
        let mut session = Self {
            root,
            owner,
            member,
            updates,
            nodes: Vec::new(),
            stop: CancellationToken::new(),
            host: None,
            host_state,
            relay: None,
        };
        let setup = async {
            let key = Keypair::generate_ed25519();
            let relay_peer = key.public().to_peer_id();
            let mut relay = swarm(key, cfg)?;
            relay.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
            let address = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let SwarmEvent::NewListenAddr { address, .. } = relay.select_next_some().await { break address; }
                }
            }).await.context("quota test relay listen stage")?;
            relay.add_external_address(address.clone()); // Test-controlled loopback only.
            session.relay = Some((relay_peer, address.clone()));
            let stop = session.stop.clone();
            session.host = Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        event = relay.select_next_some() => match event {
                            SwarmEvent::Behaviour(HostEvent::Relay(relay::Event::ReservationReqDenied { .. })) => {
                                host_updates.send_modify(|state| state.reservation_denied = true);
                            }
                            SwarmEvent::Behaviour(HostEvent::Relay(relay::Event::CircuitReqAccepted { .. })) => {
                                host_updates.send_modify(|state| state.circuits += 1);
                            }
                            SwarmEvent::Behaviour(HostEvent::Relay(relay::Event::CircuitClosed { error: Some(error), .. })) => {
                                host_updates.send_modify(|state| {
                                    if state.closed.len() < 8 { state.closed.push((error.kind(), error.to_string())); }
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }));
            session.nodes.push(Node::start(session.owner.clone(), false).await?);
            session.nodes.push(Node::start(session.member.clone(), false).await?);
            let owner_peer = session.owner.lock().await.key.public().to_peer_id();
            let route = if direct {
                let mut addresses = session.nodes[0].addresses.clone();
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(address) = addresses.borrow().first().cloned() { break Ok::<_, anyhow::Error>(address); }
                        addresses.changed().await?;
                    }
                }).await.context("quota test direct listener stage")??
            } else {
                let a = &session.nodes[0];
                a.reserve_relay(relay_peer, address).await?;
                let mut addresses = a.relay_addresses.clone();
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(address) = addresses.borrow().first().cloned() { break Ok::<_, anyhow::Error>(address); }
                        addresses.changed().await?;
                    }
                }).await.context("quota test reservation stage")??
            };
            session.nodes[1].connect(owner_peer, route).await?;
            let mut diagnostics = session.nodes[1].diagnostics.clone();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let expected = if direct { ConnectionState::Direct } else { ConnectionState::Relay };
                    if diagnostics.borrow().get(&owner_peer).is_some_and(|d| d.state == expected) { break Ok::<_, anyhow::Error>(()); }
                    diagnostics.changed().await?;
                }
            }).await.context("quota test connection stage")??;
            Ok::<_, anyhow::Error>(())
        }.await;
        if let Err(error) = setup {
            session.close().await?;
            return Err(error);
        }
        Ok(session)
    }

    async fn close(&mut self) -> Result<()> {
        self.stop.cancel();
        for node in &self.nodes {
            node.shutdown.cancel();
        }
        for node in &self.nodes {
            tokio::time::timeout(Duration::from_secs(10), node.shutdown_and_wait())
                .await
                .context("quota test node cleanup stage")?;
        }
        if let Some(host) = self.host.take() {
            tokio::time::timeout(Duration::from_secs(10), host)
                .await
                .context("quota test relay cleanup stage")??;
        }
        Ok(())
    }

    async fn download(&mut self, name: &str, size: usize) -> Result<(Uuid, std::path::PathBuf)> {
        let source = self.root.path().join(name);
        tokio::fs::write(&source, vec![37; size]).await?;
        share_paths(self.owner.clone(), vec![source]).await?;
        let peer = self.owner.lock().await.key.public().to_peer_id();
        self.nodes[1]
            .refresh_peer(peer)
            .await
            .context("quota test authorized catalog stage")?;
        let file = self
            .member
            .lock()
            .await
            .remote
            .values()
            .find(|m| m.name == name)
            .context("quota test catalog missing signed manifest")?
            .file_id;
        let output = self.root.path().join(format!("out-{name}"));
        tokio::fs::create_dir(&output).await?;
        let id = self.nodes[1].receive(file, output.clone()).await?;
        Ok((id, output))
    }

    async fn transfer(&mut self, id: Uuid, complete: bool) -> Result<()> {
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let view = self.member.lock().await.view();
                if let Some(t) = view.transfers.iter().find(|t| t.id == id) {
                    match t.status.as_str() {
                        "Completed" => {
                            ensure!(complete, "Circuit-limited transfer unexpectedly completed");
                            break Ok::<_, anyhow::Error>(());
                        }
                        "Failed" | "Cancelled" => {
                            ensure!(
                                !complete,
                                "Transfer {}: {}",
                                t.status,
                                t.error.as_deref().unwrap_or("no reason")
                            );
                            break Ok(());
                        }
                        _ => {}
                    }
                }
                self.updates.changed().await?;
            }
        })
        .await;
        if result.is_err() {
            let view = self.member.lock().await.view();
            let state = view
                .transfers
                .iter()
                .find(|t| t.id == id)
                .map(|t| (&t.status, t.bytes, &t.error));
            bail!("quota transfer stage timed out after {:?}: transfer={state:?}, relay circuits={}, closed={:?}", started.elapsed(), self.host_state.borrow().circuits, self.host_state.borrow().closed);
        }
        result??;
        Ok(())
    }

    async fn failed_safely(&mut self, id: Uuid, output: &std::path::Path) -> Result<()> {
        self.transfer(id, false).await?;
        ensure!(
            tokio::fs::read_dir(output)
                .await?
                .next_entry()
                .await?
                .is_none(),
            "Failed transfer left a completed file or partial"
        );
        let e = self.member.lock().await;
        ensure!(
            e.persisted.partials.is_empty(),
            "Owned partial cleanup was not recorded"
        );
        ensure!(
            !e.cancellations.contains_key(&id),
            "Failed transfer remained active"
        );
        let t = e.transfers.get(&id).unwrap();
        ensure!(
            t.error
                .as_ref()
                .is_some_and(|e| e.contains("Relay connection")),
            "Relay error must give truthful recovery guidance: {:?}",
            t.error
        );
        Ok(())
    }

    async fn host_error(
        &mut self,
        predicate: impl Fn(&(io::ErrorKind, String)) -> bool,
    ) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self.host_state.borrow().closed.iter().any(&predicate) {
                    break Ok::<_, anyhow::Error>(());
                }
                self.host_state.changed().await?;
            }
        })
        .await
        .context("quota test host circuit closure stage")??;
        Ok(())
    }
}

pub(super) async fn excess_reservations_and_payload_limit() -> Result<()> {
    let mut cfg = config();
    cfg.max_reservations = 1;
    cfg.max_circuits = 1;
    cfg.max_circuit_bytes = 128 * 1024;
    let mut session = Session::start(cfg, false).await?;
    let result = async {
        let outsider = Engine::open(&session.root.path().join("outsider"), Arc::new(|_| {}))?;
        let c = Node::start(outsider, false).await?;
        session.nodes.push(c.clone());
        let (peer, address) = session.relay.clone().unwrap();
        c.reserve_relay(peer, address).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if session.host_state.borrow().reservation_denied {
                    break Ok::<_, anyhow::Error>(());
                }
                session.host_state.changed().await?;
            }
        })
        .await
        .context("quota test excess reservation denial stage")??;
        ensure!(
            c.relay_addresses.borrow().is_empty(),
            "Denied reservation produced a circuit locator"
        );
        let (id, output) = session.download("limited.bin", 1024 * 1024).await?;
        session.failed_safely(id, &output).await?;
        session
            .host_error(|(_, reason)| reason == "Max circuit bytes reached.")
            .await?;
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_inside_circuit_budget_completes() -> Result<()> {
    let mut cfg = config();
    cfg.max_circuit_bytes = 512 * 1024;
    let mut session = Session::start(cfg, false).await?;
    let result = async {
        let (id, output) = session.download("small.bin", 32 * 1024).await?;
        session.transfer(id, true).await?;
        ensure!(
            tokio::fs::read(output.join("small.bin")).await? == vec![37; 32 * 1024],
            "received bytes differ"
        );
        assert_eq!(session.host_state.borrow().circuits, 1);
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn byte_budget_closure_fails_without_committing_or_retrying() -> Result<()> {
    let mut cfg = config();
    cfg.max_circuit_bytes = 128 * 1024;
    let mut session = Session::start(cfg, false).await?;
    let result = async {
        let (id, output) = session.download("too-large.bin", 1024 * 1024).await?;
        session.failed_safely(id, &output).await?;
        session
            .host_error(|(_, reason)| reason == "Max circuit bytes reached.")
            .await?;
        // Observe subsequent state ticks: routing may reconnect, downloads never restart themselves.
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(5), session.updates.changed()).await??;
            assert_eq!(session.member.lock().await.transfers.len(), 1);
        }
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn earlier_transfer_and_protocol_overhead_consume_the_same_circuit_budget() -> Result<()> {
    let mut cfg = config();
    cfg.max_circuit_bytes = 256 * 1024;
    let mut session = Session::start(cfg, false).await?;
    let result = async {
        let (first, output) = session.download("first.bin", 64 * 1024).await?;
        session.transfer(first, true).await?;
        ensure!(
            tokio::fs::read(output.join("first.bin")).await? == vec![37; 64 * 1024],
            "received bytes differ"
        );
        let (second, output) = session.download("second.bin", 192 * 1024).await?;
        session.failed_safely(second, &output).await?;
        session
            .host_error(|(_, reason)| reason == "Max circuit bytes reached.")
            .await?;
        assert_eq!(
            session.host_state.borrow().circuits,
            1,
            "No new circuit may bypass the consumed budget"
        );
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn circuit_lifetime_closure_fails_without_committing() -> Result<()> {
    let mut cfg = config();
    cfg.max_circuit_duration = Duration::from_secs(2);
    cfg.max_circuit_bytes = 16 * 1024 * 1024;
    let mut session = Session::start(cfg, false).await?;
    let result = async {
        let (id, output) = session.download("slow.bin", 4 * 1024 * 1024).await?;
        session.failed_safely(id, &output).await?;
        session
            .host_error(|(kind, _)| *kind == io::ErrorKind::TimedOut)
            .await?;
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_shutdown_during_transfer_never_leaves_a_completed_file() -> Result<()> {
    let mut session = Session::start(config(), false).await?;
    let result = async {
        let (id, output) = session.download("interrupted.bin", 4 * 1024 * 1024).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let e = session.member.lock().await;
                if e.transfers.get(&id).is_some_and(|t| t.bytes > 0) {
                    break Ok::<_, anyhow::Error>(());
                }
                drop(e);
                session.updates.changed().await?;
            }
        })
        .await
        .context("quota test active payload stage")??;
        session.stop.cancel();
        session.failed_safely(id, &output).await?;
        Ok(())
    }
    .await;
    session.close().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_file_larger_than_relay_budget_is_not_blocked() -> Result<()> {
    let mut cfg = config();
    cfg.max_circuit_bytes = 1;
    cfg.max_circuit_duration = Duration::from_secs(1);
    let mut session = Session::start(cfg, true).await?;
    let result = async {
        let (id, output) = session.download("direct.bin", 256 * 1024).await?;
        session.transfer(id, true).await?;
        ensure!(
            tokio::fs::read(output.join("direct.bin")).await? == vec![37; 256 * 1024],
            "received bytes differ"
        );
        assert!(
            !session
                .member
                .lock()
                .await
                .transfers
                .get(&id)
                .unwrap()
                .relayed
        );
        assert_eq!(session.host_state.borrow().circuits, 0);
        Ok(())
    }
    .await;
    session.close().await?;
    result
}
