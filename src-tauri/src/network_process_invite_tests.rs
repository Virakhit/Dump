//! Separate OS processes, identities and protected stores with real loopback sockets.
//! LAN discovery is injected through the private test harness, not real mDNS.
//! Nothing here is compiled into the desktop application.
use super::*;
use crate::engine::Engine;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

const REPLY: &str = "DUMP_TEST_REPLY ";
const WORKER: &str = "network::process_invite_tests::peer_worker";
const DEADLINE: Duration = Duration::from_secs(25);
// Bound simultaneous child processes even in the normal parallel lib test suite.
static PROCESS_TEST: Mutex<()> = Mutex::const_new(());

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Command {
    Create {
        name: String,
    },
    Activate {
        workspace: Uuid,
    },
    Invite {
        lan: bool,
    },
    Join {
        url: String,
    },
    Connect {
        peer: String,
        address: Multiaddr,
        lan: bool,
    },
    Approve {
        peer: String,
    },
    Reject {
        peer: String,
    },
    PollCancelled {
        url: String,
    },
    Cancel,
    Trace,
    View,
    Stop,
}

async fn summary(shared: &Shared) -> Result<Value> {
    let e = shared.lock().await;
    let view = e.view();
    let saved = e.store.load()?;
    for workspace in saved.workspaces.values() {
        workspace.snapshot.verify()?;
    }
    Ok(json!({
        "peer": view.peer_id,
        "active": view.active,
        "joining": view.joining,
        "stage": view.join_stage,
        "stored_invites": saved.invites.len(),
        "pending": view.pending.iter().map(|p| json!({
            "peer": p.peer_id, "workspace": p.workspace_id,
        })).collect::<Vec<_>>(),
        "saved_active": saved.active,
        "workspaces": saved.workspaces.values().map(|w| json!({
            "id": w.snapshot.workspace_id,
            "members": w.snapshot.members.iter().map(|m| &m.peer_id).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "error": view.last_error,
    }))
}

async fn execute(node: &Node, command: Command) -> Result<Value> {
    match command {
        Command::Create { name } => node.shared.lock().await.create_workspace(name)?,
        Command::Activate { workspace } => node.shared.lock().await.activate(workspace)?,
        Command::Invite { lan } => {
            let mut e = node.shared.lock().await;
            let url = if lan {
                e.create_lan_invite()?
            } else {
                e.create_invite()?
            };
            return Ok(json!({"url": url}));
        }
        Command::Join { url } => node.shared.lock().await.join(&url)?,
        Command::Connect { peer, address, lan } => {
            let peer = peer.parse()?;
            if lan {
                node.test_mdns(peer, address, true).await?;
            } else {
                node.connect(peer, address).await?;
            }
        }
        Command::Approve { peer } => node.shared.lock().await.approve(&peer)?,
        Command::Reject { peer } => node.shared.lock().await.reject(&peer)?,
        Command::PollCancelled { url } => {
            let error = node
                .poll_join(Invitation::parse(&url)?)
                .await
                .expect_err("A cancelled join accepted a late grant");
            ensure!(
                error.to_string() == "Join was cancelled",
                "Expected a verified late grant to be discarded, got: {error}"
            );
        }
        Command::Cancel => node.shared.lock().await.cancel_join()?,
        Command::Trace => {
            let diagnostics = node.diagnostics.borrow().clone();
            return Ok(json!({
                "view": summary(&node.shared).await?,
                "connections": diagnostics.iter().map(|(peer, d)| json!({
                    "peer": peer.to_string(), "state": format!("{:?}", d.state),
                    "last_failure": format!("{:?}", d.last_failure),
                    "protocol": d.last_protocol, "outcome": format!("{:?}", d.protocol_outcome),
                })).collect::<Vec<_>>(),
            }));
        }
        Command::View | Command::Stop => {}
    }
    summary(&node.shared).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Private child-process entry point; run the multiprocess_* parent tests"]
async fn peer_worker() -> Result<()> {
    let directory = std::env::var_os("DUMP_INVITE_TEST_STORE")
        .context("This entry point requires a parent-created temporary test store")?;
    let shared = Engine::open(Path::new(&directory), Arc::new(|_| {}))?;
    shared
        .lock()
        .await
        .set_name("Isolated invitation test".into())?;
    let node = Node::start_inner(shared.clone(), false, true).await?;
    let mut addresses = node.addresses.clone();
    let listeners = tokio::time::timeout(DEADLINE, async {
        loop {
            let current = addresses.borrow().clone();
            if current.len() >= 2 {
                return Ok::<_, anyhow::Error>(current);
            }
            addresses.changed().await?;
        }
    })
    .await??;
    println!(
        "{REPLY}{}",
        json!({
            "peer": shared.lock().await.peer(),
            "addresses": listeners.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "pid": std::process::id(),
        })
    );
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = input.next_line().await? {
        let command: Command = serde_json::from_str(&line)?;
        let stop = matches!(command, Command::Stop);
        let response = match execute(&node, command).await {
            Ok(value) => json!({"ok": value}),
            Err(error) => json!({"error": error.to_string()}),
        };
        println!("{REPLY}{response}");
        std::io::Write::flush(&mut std::io::stdout())?;
        if stop {
            break;
        }
    }
    node.shutdown_and_wait().await;
    Ok(())
}

struct PeerProcess {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    ready: Value,
}

impl PeerProcess {
    async fn start(directory: &Path) -> Result<Self> {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .args(["--exact", WORKER, "--ignored", "--nocapture"])
            .env("DUMP_INVITE_TEST_STORE", directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .creation_flags(0x08000000);
        let mut child = command.spawn()?;
        let input = child.stdin.take().context("No worker stdin")?;
        let output = BufReader::new(child.stdout.take().context("No worker stdout")?).lines();
        let mut worker = Self {
            child,
            input,
            output,
            ready: Value::Null,
        };
        worker.ready = worker.read().await.context("Worker startup")?;
        Ok(worker)
    }

    async fn read(&mut self) -> Result<Value> {
        tokio::time::timeout(DEADLINE, async {
            while let Some(line) = self.output.next_line().await? {
                if let Some(payload) = line.strip_prefix(REPLY) {
                    return Ok(serde_json::from_str(payload)?);
                }
            }
            bail!("Worker exited before replying")
        })
        .await
        .context("Worker reply deadline")?
    }

    async fn ask(&mut self, command: Command) -> Result<Value> {
        let mut bytes = serde_json::to_vec(&command)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        let response = self.read().await?;
        ensure!(
            response.get("error").is_none(),
            "Worker operation: {}",
            response["error"]
        );
        response.get("ok").cloned().context("Missing worker result")
    }

    fn peer(&self) -> Result<PeerId> {
        self.ready["peer"]
            .as_str()
            .context("Missing peer ID")?
            .parse()
            .map_err(Into::into)
    }

    fn address(&self, tcp: bool) -> Result<Multiaddr> {
        self.ready["addresses"]
            .as_array()
            .context("Missing listeners")?
            .iter()
            .filter_map(Value::as_str)
            .find(|a| {
                if tcp {
                    a.contains("/tcp/")
                } else {
                    a.contains("/quic-v1")
                }
            })
            .context("Missing selected transport")?
            .parse()
            .map_err(Into::into)
    }

    async fn wait(&mut self, label: &str, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
        tokio::time::timeout(DEADLINE, async {
            loop {
                let view = self.ask(Command::View).await?;
                if predicate(&view) {
                    return Ok(view);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .with_context(|| format!("Timed out: {label}"))?
    }

    async fn stop(&mut self) -> Result<()> {
        self.ask(Command::Stop).await?;
        let status = tokio::time::timeout(DEADLINE, self.child.wait()).await??;
        ensure!(status.success(), "Worker failed at shutdown: {status}");
        Ok(())
    }
}

async fn approve_flow(lan: bool, tcp: bool) -> Result<()> {
    let _serial = PROCESS_TEST.lock().await;
    let root = tempfile::tempdir()?;
    let mut owner = PeerProcess::start(&root.path().join("owner")).await?;
    let mut member = PeerProcess::start(&root.path().join("member")).await?;
    ensure!(
        owner.ready["pid"] != member.ready["pid"],
        "Peers must use separate OS processes"
    );
    ensure!(
        owner.peer()? != member.peer()?,
        "Peers must use separate identities"
    );
    let target = owner
        .ask(Command::Create {
            name: "Target workspace".into(),
        })
        .await?["active"]
        .clone();
    let old = member
        .ask(Command::Create {
            name: "Existing workspace".into(),
        })
        .await?["active"]
        .clone();
    let url = owner.ask(Command::Invite { lan }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = member.ask(Command::Join { url }).await?;
    ensure!(
        before["active"] == old,
        "Pending join replaced the existing workspace"
    );
    ensure!(
        before["joining"] == true && before["stage"] != "awaiting_approval",
        "Premature approval waiting state"
    );
    ensure!(owner.ask(Command::View).await?["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    member
        .ask(Command::Connect {
            peer: owner.peer()?.to_string(),
            address: owner.address(tcp)?,
            lan,
        })
        .await?;
    if lan {
        owner
            .ask(Command::Connect {
                peer: member.peer()?.to_string(),
                address: member.address(tcp)?,
                lan: true,
            })
            .await?;
    }
    let peer = member.peer()?.to_string();
    owner
        .wait("owner sees the target-workspace request", |v| {
            v["pending"].as_array().is_some_and(|p| {
                p.iter()
                    .any(|p| p["peer"] == peer && p["workspace"] == target)
            })
        })
        .await?;
    let pending = member
        .wait("authenticated request acknowledgment", |v| {
            v["stage"] == "awaiting_approval"
        })
        .await?;
    ensure!(
        pending["active"] == old && pending["saved_active"] == old,
        "Joined before explicit owner approval"
    );
    ensure!(
        !pending["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == target),
        "Granted membership before approval"
    );
    owner.ask(Command::Approve { peer: peer.clone() }).await?;
    let joined = member
        .wait("approved workspace selected and saved", |v| {
            v["active"] == target && v["saved_active"] == target && v["joining"] == false
        })
        .await?;
    ensure!(joined["stage"].is_null());
    ensure!(
        joined["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == old),
        "Lost the existing workspace"
    );
    ensure!(
        joined["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == target
                && w["members"].as_array().unwrap().iter().any(|m| m == &peer)),
        "Missing signed membership"
    );
    member.stop().await?;
    owner.stop().await?;
    // Verify persistence after process exit and a fresh Engine load, not shared memory.
    let mut reopened = PeerProcess::start(&root.path().join("member")).await?;
    let saved = reopened.ask(Command::View).await?;
    ensure!(
        saved["active"] == target && saved["saved_active"] == target && saved["joining"] == false
    );
    reopened.stop().await?;
    eprintln!("PASS separate processes: {} / {}; explicit approval, signed membership, old workspace preserved, restart persisted", if lan { "LAN invite (injected discovery)" } else { "full invite" }, if tcp { "TCP + Noise" } else { "QUIC" });
    Ok(())
}

#[tokio::test]
async fn multiprocess_full_invite_quic_approval_and_restart() -> Result<()> {
    approve_flow(false, false).await
}

#[tokio::test]
async fn multiprocess_full_invite_tcp_approval_and_restart() -> Result<()> {
    approve_flow(false, true).await
}

#[tokio::test]
async fn multiprocess_lan_invite_approval_and_restart() -> Result<()> {
    approve_flow(true, false).await
}

#[tokio::test]
async fn multiprocess_wrong_owner_workspace_does_not_grant_membership() -> Result<()> {
    let _serial = PROCESS_TEST.lock().await;
    let root = tempfile::tempdir()?;
    let mut owner = PeerProcess::start(&root.path().join("owner")).await?;
    let mut member = PeerProcess::start(&root.path().join("member")).await?;
    let target = owner
        .ask(Command::Create {
            name: "Invited workspace".into(),
        })
        .await?["active"]
        .clone();
    let url = owner.ask(Command::Invite { lan: false }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    owner
        .ask(Command::Create {
            name: "Wrong selected workspace".into(),
        })
        .await?;
    member.ask(Command::Join { url }).await?;
    member
        .ask(Command::Connect {
            peer: owner.peer()?.to_string(),
            address: owner.address(false)?,
            lan: false,
        })
        .await?;
    let denied = member
        .wait("wrong workspace rejected", |v| {
            v["joining"] == false && v["error"].is_string()
        })
        .await?;
    ensure!(denied["active"].is_null() && denied["workspaces"].as_array().unwrap().is_empty());
    ensure!(owner.ask(Command::View).await?["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    // A newly issued invite after selecting the correct target must work again.
    owner
        .ask(Command::Activate {
            workspace: serde_json::from_value(target.clone())?,
        })
        .await?;
    let url = owner.ask(Command::Invite { lan: false }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    member.ask(Command::Join { url }).await?;
    let peer = member.peer()?.to_string();
    owner
        .wait("request after selecting the target workspace", |v| {
            !v["pending"].as_array().unwrap().is_empty()
        })
        .await?;
    owner.ask(Command::Approve { peer }).await?;
    member
        .wait("correct workspace approved", |v| {
            v["active"] == target && v["joining"] == false
        })
        .await?;
    member.stop().await?;
    owner.stop().await?;
    eprintln!("PASS separate processes: wrong owner-selected workspace denies join; correct target and fresh invite recover");
    Ok(())
}

#[tokio::test]
async fn multiprocess_cancel_before_late_approval_does_not_join() -> Result<()> {
    let _serial = PROCESS_TEST.lock().await;
    let root = tempfile::tempdir()?;
    let mut owner = PeerProcess::start(&root.path().join("owner")).await?;
    let mut member = PeerProcess::start(&root.path().join("member")).await?;
    owner
        .ask(Command::Create {
            name: "Cancellation".into(),
        })
        .await?;
    let url = owner.ask(Command::Invite { lan: false }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    member.ask(Command::Join { url: url.clone() }).await?;
    member
        .ask(Command::Connect {
            peer: owner.peer()?.to_string(),
            address: owner.address(false)?,
            lan: false,
        })
        .await?;
    owner
        .wait("request before cancellation", |v| {
            !v["pending"].as_array().unwrap().is_empty()
        })
        .await?;
    member
        .wait("request acknowledged before cancellation", |v| {
            v["stage"] == "awaiting_approval"
        })
        .await?;
    member.ask(Command::Cancel).await?;
    // Current cancellation is local: owner may still hold the old pending card.
    owner
        .ask(Command::Approve {
            peer: member.peer()?.to_string(),
        })
        .await?;
    // Read the real authenticated Approved response even after cancellation;
    // this must hit the cancellation guard, not pass because no response arrived.
    member.ask(Command::PollCancelled { url }).await?;
    let cancelled = member.ask(Command::View).await?;
    ensure!(
        cancelled["joining"] == false
            && cancelled["active"].is_null()
            && cancelled["stage"].is_null()
    );
    ensure!(cancelled["workspaces"].as_array().unwrap().is_empty());
    member.stop().await?;
    owner.stop().await?;
    let mut reopened = PeerProcess::start(&root.path().join("member")).await?;
    let saved = reopened.ask(Command::View).await?;
    ensure!(
        saved["active"].is_null()
            && saved["workspaces"].as_array().unwrap().is_empty()
            && saved["joining"] == false
    );
    reopened.stop().await?;
    eprintln!("PASS separate processes: cancellation before owner approval leaves client unjoined after restart; owner-side withdrawal is not implemented");
    Ok(())
}

#[tokio::test]
async fn multiprocess_rejection_clears_join_without_granting_membership() -> Result<()> {
    let _serial = PROCESS_TEST.lock().await;
    let root = tempfile::tempdir()?;
    let mut owner = PeerProcess::start(&root.path().join("owner")).await?;
    let mut member = PeerProcess::start(&root.path().join("member")).await?;
    let target = owner
        .ask(Command::Create {
            name: "Rejected invite".into(),
        })
        .await?["active"]
        .clone();
    let previous = member
        .ask(Command::Create {
            name: "Keep this workspace".into(),
        })
        .await?["active"]
        .clone();
    let url = owner.ask(Command::Invite { lan: false }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    member.ask(Command::Join { url }).await?;
    member
        .ask(Command::Connect {
            peer: owner.peer()?.to_string(),
            address: owner.address(false)?,
            lan: false,
        })
        .await?;
    let peer = member.peer()?.to_string();
    owner
        .wait("request before rejection", |v| {
            v["pending"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["peer"] == peer && p["workspace"] == target)
        })
        .await?;
    member
        .wait("acknowledged request before rejection", |v| {
            v["stage"] == "awaiting_approval"
        })
        .await?;
    let rejected = owner.ask(Command::Reject { peer: peer.clone() }).await?;
    ensure!(rejected["pending"].as_array().unwrap().is_empty());
    ensure!(
        !rejected["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["members"].as_array().unwrap().iter().any(|m| m == &peer)),
        "Rejected peer was added to the signed roster"
    );
    let cleared = member
        .wait("owner rejection delivered over the network", |v| {
            v["joining"] == false && v["error"].as_str().is_some_and(|e| e.contains("rejected"))
        })
        .await?;
    ensure!(
        cleared["active"] == previous
            && cleared["saved_active"] == previous
            && cleared["stage"].is_null()
    );
    ensure!(!cleared["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == target));
    member.stop().await?;
    owner.stop().await?;
    let mut reopened = PeerProcess::start(&root.path().join("member")).await?;
    let saved = reopened.ask(Command::View).await?;
    ensure!(saved["active"] == previous && saved["joining"] == false);
    ensure!(!saved["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == target));
    reopened.stop().await?;
    eprintln!("PASS separate processes: explicit owner rejection reaches client, retains its old workspace and grants no membership after restart");
    Ok(())
}

#[tokio::test]
async fn multiprocess_full_invite_recovers_when_owner_restarts_before_approval() -> Result<()> {
    let _serial = PROCESS_TEST.lock().await;
    let root = tempfile::tempdir()?;
    let owner_store = root.path().join("owner");
    let mut owner = PeerProcess::start(&owner_store).await?;
    let mut member = PeerProcess::start(&root.path().join("member")).await?;
    let target = owner
        .ask(Command::Create {
            name: "Owner restart".into(),
        })
        .await?["active"]
        .clone();
    let url = owner.ask(Command::Invite { lan: false }).await?["url"]
        .as_str()
        .unwrap()
        .to_owned();
    member.ask(Command::Join { url }).await?;
    member
        .ask(Command::Connect {
            peer: owner.peer()?.to_string(),
            address: owner.address(false)?,
            lan: false,
        })
        .await?;
    let peer = member.peer()?.to_string();
    owner
        .wait("request before owner restart", |v| {
            v["pending"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["peer"] == peer)
        })
        .await?;
    member
        .wait("request acknowledged before owner restart", |v| {
            v["stage"] == "awaiting_approval"
        })
        .await?;
    let original_peer = owner.peer()?;
    let original_pid = owner.ready["pid"].clone();
    owner.stop().await?;
    let still_pending = member.ask(Command::View).await?;
    ensure!(still_pending["joining"] == true && still_pending["active"].is_null());
    ensure!(!still_pending["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == target));
    let mut restarted = PeerProcess::start(&owner_store).await?;
    ensure!(restarted.peer()? == original_peer && restarted.ready["pid"] != original_pid);
    ensure!(restarted.ask(Command::View).await?["active"] == target);
    ensure!(restarted.ask(Command::View).await?["stored_invites"] == 1);
    eprintln!(
        "Owner restart routes: previous={}, current={}",
        owner.ready["addresses"], restarted.ready["addresses"]
    );
    // This supplies the newly allocated listener address, not real mDNS discovery.
    // The persisted full invite and the application's normal retry loop do the rest.
    member
        .ask(Command::Connect {
            peer: restarted.peer()?.to_string(),
            address: restarted.address(false)?,
            lan: false,
        })
        .await?;
    // Observe reconnect timing separately from the short command/view deadline.
    // QUIC expiry plus the existing per-route retry interval can exceed 25 s.
    // Retain state transitions in the log instead of silently retrying the test.
    let reconnect_started = Instant::now();
    let mut last_trace = Value::Null;
    tokio::time::timeout(Duration::from_secs(75), async {
        loop {
            let view = restarted.ask(Command::View).await?;
            if view["pending"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["peer"] == peer && p["workspace"] == target)
            {
                return Ok::<_, anyhow::Error>(());
            }
            let trace = member.ask(Command::Trace).await?;
            if trace != last_trace {
                eprintln!(
                    "Owner restart at {:?}: member={trace}, owner={view}",
                    reconnect_started.elapsed()
                );
                last_trace = trace;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .context("Owner restart observation deadline (75 s)")??;
    eprintln!(
        "Owner restart request reconstructed after {:?}",
        reconnect_started.elapsed()
    );
    restarted.ask(Command::Approve { peer }).await?;
    member
        .wait("approval from restarted owner", |v| {
            v["joining"] == false && v["active"] == target && v["saved_active"] == target
        })
        .await?;
    member.stop().await?;
    restarted.stop().await?;
    let mut reopened = PeerProcess::start(&root.path().join("member")).await?;
    ensure!(reopened.ask(Command::View).await?["active"] == target);
    reopened.stop().await?;
    eprintln!("PASS separate processes: full invitation survives owner restart, normal polling reconstructs the request after a supplied new route, explicit approval persists");
    Ok(())
}
