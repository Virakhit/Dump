use super::*;
use crate::engine::{share_paths, Engine, View};

#[derive(NetworkBehaviour)]
struct ProbeServer {
    nat: libp2p::autonat::v2::server::Behaviour,
    identify: identify::Behaviour,
}

struct Session(Vec<Node>);
impl Drop for Session {
    fn drop(&mut self) {
        for node in &self.0 {
            node.shutdown.cancel();
        }
    }
}
impl Session {
    async fn close(&self) -> Result<()> {
        for node in &self.0 {
            node.shutdown.cancel();
        }
        for node in &self.0 {
            tokio::time::timeout(Duration::from_secs(10), node.shutdown_and_wait())
                .await
                .context("file/reprobe cleanup")?;
        }
        Ok(())
    }
}

async fn listener(node: &Node) -> Result<Multiaddr> {
    let mut addresses = node.addresses.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(address) = addresses
                .borrow()
                .iter()
                .find(|a| a.iter().any(|p| p == libp2p::multiaddr::Protocol::QuicV1))
                .cloned()
            {
                return Ok(address);
            }
            addresses.changed().await?;
        }
    })
    .await
    .context("file/reprobe listener")?
}

async fn probe(
    node: &Node,
    peer: PeerId,
    address: Option<Multiaddr>,
    revalidate: bool,
) -> Result<BTreeSet<ConnectionId>> {
    let (reply, received) = oneshot::channel();
    node.protocol_reports
        .send(ProtocolReport {
            peer,
            protocol: CONTROL,
            outcome: ProtocolOutcome::Negotiated,
            probe: Some(TestProbe {
                address,
                revalidate,
                reply,
            }),
        })
        .await?;
    Ok(tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .context("file/reprobe actor registration")??)
}

async fn confirmed(node: &Node, count: usize) -> Result<()> {
    let mut events = node.nat_confirmations.clone();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if *events.borrow() >= count {
                return Ok::<_, anyhow::Error>(());
            }
            events.changed().await?;
        }
    })
    .await
    .context("file/reprobe authenticated dial-back")?
}

async fn finished(
    node: &Node,
    events: &mut watch::Receiver<Option<View>>,
    id: Uuid,
    complete: bool,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let Some(transfer) = node
                .shared
                .lock()
                .await
                .view()
                .transfers
                .iter()
                .find(|t| t.id == id)
            {
                match transfer.status.as_str() {
                    "Completed" => {
                        ensure!(complete, "no-overwrite receive unexpectedly completed");
                        return Ok(());
                    }
                    "Failed" | "Cancelled" => {
                        ensure!(
                            !complete,
                            "file/reprobe transfer {} after {}/{} bytes: {}",
                            transfer.status,
                            transfer.bytes,
                            transfer.total,
                            transfer.error.as_deref().unwrap_or("no reason")
                        );
                        ensure!(
                            transfer
                                .error
                                .as_deref()
                                .is_some_and(|e| e.contains("already exists")),
                            "no-overwrite receive failed for a different reason"
                        );
                        return Ok(());
                    }
                    _ => {}
                }
            }
            events.changed().await?;
        }
    })
    .await
    .context("file/reprobe completion")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_file_receive_commits_after_real_reprobe_without_disconnect() -> Result<()> {
    let root = tempfile::tempdir()?;
    let owner = Engine::open(&root.path().join("owner"), Arc::new(|_| {}))?;
    let (updates, mut events) = watch::channel(None);
    let member = Engine::open(
        &root.path().join("member"),
        Arc::new(move |view| {
            updates.send_replace(Some(view));
        }),
    )?;
    owner.lock().await.create_workspace("Revalidation".into())?;
    let member_peer = member.lock().await.peer();
    // A genuine owner-signed roster isolates transfer/revalidation from invitation timing.
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
        workspace.snapshot.verify()?;
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
    let mut session = Session(Vec::new());
    let result = async {
        session.0.push(Node::start(owner.clone(), false).await?);
        session.0.push(Node::start(member.clone(), false).await?);
        let a = &session.0[0];
        let b = &session.0[1];
        let owner_peer = owner.lock().await.key.public().to_peer_id();
        b.connect(owner_peer, listener(a).await?).await?;
        let mut diagnostics = b.diagnostics.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if diagnostics.borrow().get(&owner_peer).is_some_and(|d| d.state == ConnectionState::Direct) { return Ok::<_, anyhow::Error>(()); }
                diagnostics.changed().await?;
            }
        }).await.context("file/reprobe application connection")??;

        let mut server = SwarmBuilder::with_new_identity().with_tokio().with_quic().with_behaviour(|key| ProbeServer {
            nat: Default::default(),
            identify: identify::Behaviour::new(identify::Config::new("/dump/test".into(), key.public()).with_hide_listen_addrs(true).with_cache_size(0)),
        })?.with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60))).build();
        server.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse()?)?;
        let address = tokio::time::timeout(Duration::from_secs(10), async {
            loop { if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await { return address; } }
        }).await.context("file/reprobe test server listener")?;
        let server_peer = *server.local_peer_id();
        let stop = b.shutdown.clone();
        b.tasks.spawn(async move {
            let mut dialbacks = BTreeSet::new();
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    event = server.select_next_some() => match event {
                        SwarmEvent::ConnectionEstablished { connection_id, endpoint, .. } if endpoint.is_dialer() => { dialbacks.insert(connection_id); },
                        SwarmEvent::Behaviour(ProbeServerEvent::Nat(_)) => {
                            // Production probes are five minutes apart and idle probe sockets
                            // expire after one minute. Retire only completed test dial-backs so
                            // advancing the test clock does not exceed Node's two-per-peer cap.
                            for id in std::mem::take(&mut dialbacks) { server.close_connection(id); }
                        },
                        _ => {},
                    }
                }
            }
        });
        b.connect(server_peer, address).await?;
        probe(b, owner_peer, Some(listener(b).await?), false).await?;
        confirmed(b, 1).await?;

        let source = root.path().join("reprobe.bin");
        let payload: Vec<u8> = (0..2 * BLOCK + 17).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&source, &payload).await?;
        share_paths(owner.clone(), vec![source]).await?;
        b.refresh_peer(owner_peer).await.context("file/reprobe authorized signed catalog")?;
        let manifest = member.lock().await.remote.values().find(|m| m.name == "reprobe.bin").context("signed manifest missing")?.clone();
        manifest.verify()?;
        let output = root.path().join("received");
        tokio::fs::create_dir(&output).await?;
        let (started, progress) = oneshot::channel();
        let (resume, resumed) = oneshot::channel();
        *b.limits.receive_pause.lock().await = Some(ReceivePause { started, resume: resumed });
        let id = b.receive(manifest.file_id, output.clone()).await?;
        tokio::time::timeout(Duration::from_secs(10), progress).await.context("file/reprobe active disk-write progress")??;
        {
            let e = member.lock().await;
            let t = &e.transfers[&id];
            ensure!(t.status == "Receiving" && t.bytes > 0 && t.bytes < t.total, "receive was not active before refresh");
            ensure!(e.persisted.partials.len() == 1, "active receive has no recorded partial");
        }
        ensure!(!output.join(&manifest.name).exists(), "destination committed before verification");
        let before = probe(b, owner_peer, None, false).await?;
        ensure!(!before.is_empty(), "application connection missing before re-probe");
        let count = *b.nat_confirmations.borrow();
        probe(b, owner_peer, None, true).await?;
        confirmed(b, count + 1).await?;
        let after = probe(b, owner_peer, None, false).await?;
        ensure!(before.is_subset(&after), "revalidation closed an active application connection");
        ensure!(member.lock().await.transfers[&id].status == "Receiving", "revalidation interrupted signed file receive");
        resume.send(()).map_err(|_| anyhow::anyhow!("receiver stopped while refreshing"))?;
        finished(b, &mut events, id, true).await?;
        let bytes = tokio::fs::read(output.join(&manifest.name)).await?;
        ensure!(bytes == payload && bytes.len() as u64 == manifest.size, "committed file bytes/size mismatch");
        ensure!(hex::encode(Sha256::digest(&bytes)) == manifest.sha256, "committed file SHA-256 mismatch");
        ensure!(member.lock().await.persisted.partials.is_empty(), "completed file left a recorded partial");
        ensure!(std::fs::read_dir(&output)?.count() == 1, "completed file left a partial");
        let collision = b.receive(manifest.file_id, output.clone()).await?;
        finished(b, &mut events, collision, false).await?;
        ensure!(tokio::fs::read(output.join(&manifest.name)).await? == payload, "existing destination was overwritten");
        ensure!(member.lock().await.persisted.partials.is_empty(), "collision left a recorded partial");
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = session.close().await;
    result?;
    cleanup
}
