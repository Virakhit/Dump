//! Bounded in-memory transport tests: real Noise/file serving, no socket claims.
use super::*;
use crate::engine::Engine;
use futures::{channel::mpsc, Sink, Stream as FuturesStream};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
struct Memory {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    buffered: Vec<u8>,
    offset: usize,
}
fn pair() -> (Memory, Memory) {
    let (at, br) = mpsc::channel(1);
    let (bt, ar) = mpsc::channel(1);
    (
        Memory {
            tx: at,
            rx: ar,
            buffered: vec![],
            offset: 0,
        },
        Memory {
            tx: bt,
            rx: br,
            buffered: vec![],
            offset: 0,
        },
    )
}
impl AsyncRead for Memory {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            if this.offset < this.buffered.len() {
                let count = out.len().min(this.buffered.len() - this.offset);
                out[..count].copy_from_slice(&this.buffered[this.offset..this.offset + count]);
                this.offset += count;
                return Poll::Ready(Ok(count));
            }
            match Pin::new(&mut this.rx).poll_next(cx) {
                Poll::Ready(Some(bytes)) => {
                    this.buffered = bytes;
                    this.offset = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
impl AsyncWrite for Memory {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.tx).poll_ready(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(_)) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            Poll::Ready(Ok(())) => {}
        }
        let count = bytes.len().min(8192);
        Pin::new(&mut this.tx)
            .start_send(bytes[..count].to_vec())
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Poll::Ready(Ok(count))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().tx)
            .poll_flush(cx)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().tx)
            .poll_close(cx)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}
#[tokio::test]
async fn bounded_bridge_authenticates_end_peers_and_serves_exact_signed_file() -> Result<()> {
    let root = tempfile::tempdir()?;
    let shared = Engine::open(&root.path().join("source"), Arc::new(|_| {}))?;
    let owner = libp2p::identity::Keypair::generate_ed25519();
    let receiver = libp2p::identity::Keypair::generate_ed25519();
    let key = shared.lock().await.key.clone();
    let mut snapshot = Snapshot {
        version: crate::model::VERSION,
        workspace_id: Uuid::new_v4(),
        name: "Memory bridge".into(),
        owner_peer_id: owner.public().to_peer_id().to_string(),
        owner_public_key: crate::model::public_key(&owner),
        revision: 1,
        members: [&owner, &key, &receiver]
            .into_iter()
            .map(|key| crate::model::Member {
                peer_id: key.public().to_peer_id().to_string(),
                name: "Device".into(),
            })
            .collect(),
        signature: String::new(),
    };
    snapshot.sign(&owner)?;
    {
        let mut e = shared.lock().await;
        let mut next = e.persisted.clone();
        next.active = Some(snapshot.workspace_id);
        next.workspaces.insert(
            snapshot.workspace_id,
            crate::model::Workspace {
                snapshot: snapshot.clone(),
                secret: crate::model::random_secret(),
            },
        );
        e.persist(next)?;
    }
    let payload: Vec<u8> = (0..192 * 1024 + 17).map(|i| (i % 251) as u8).collect();
    let path = root.path().join("payload.bin");
    std::fs::write(&path, &payload)?;
    crate::engine::share_paths(shared.clone(), vec![path]).await?;
    let manifest = shared
        .lock()
        .await
        .persisted
        .shares
        .values()
        .next()
        .unwrap()
        .manifest
        .clone();
    let grant = Grant::new(
        &owner,
        snapshot.clone(),
        receiver.public().to_peer_id().to_string(),
        key.public().to_peer_id().to_string(),
        Purpose::File {
            manifest: manifest.clone(),
        },
        Uuid::new_v4(),
    )?;
    let (client, host_client) = pair();
    let (source, host_source) = pair();
    let cancel = CancellationToken::new();
    let remaining = Arc::new(AtomicU64::new(grant.purpose.budget()?));
    let bandwidth = Arc::new(host_relay::Bandwidth::default());
    let bridge = async {
        let (cr, cw) = host_client.split();
        let (sr, sw) = host_source.split();
        tokio::try_join!(
            host_relay::pump(cr, sw, remaining.clone(), bandwidth.clone(), cancel.clone()),
            host_relay::pump(sr, cw, remaining, bandwidth, cancel)
        )?;
        Ok::<_, anyhow::Error>(())
    };
    let source_task = async {
        let noise = libp2p::noise::Config::new(&key)?;
        let protocol = noise.protocol_info().next().unwrap();
        let (actual, mut stream) = noise.upgrade_inbound(source, protocol).await?;
        ensure!(actual.to_string() == grant.receiver, "Wrong Noise receiver");
        let request = read_frame(&mut stream).await?;
        grant.authorize_request(&request)?;
        serve_file_request(
            shared.clone(),
            Arc::new(Limits::new()),
            actual,
            stream,
            request,
            true,
            CancellationToken::new(),
        )
        .await
    };
    let client_task = async {
        let noise = libp2p::noise::Config::new(&receiver)?;
        let protocol = noise.protocol_info().next().unwrap();
        let (actual, mut stream) = noise.upgrade_outbound(client, protocol).await?;
        ensure!(actual == key.public().to_peer_id(), "Wrong Noise source");
        ensure!(
            actual != owner.public().to_peer_id(),
            "Host impersonated source"
        );
        write_frame(
            &mut stream,
            &Request::File {
                snapshot: snapshot.clone(),
                manifest: manifest.clone(),
            },
        )
        .await?;
        let Response::File { manifest: header } = read_frame(&mut stream).await? else {
            anyhow::bail!("File denied")
        };
        ensure!(header == manifest, "Wrong file identity");
        let mut received = vec![0; manifest.size as usize];
        stream.read_exact(&mut received).await?;
        ensure!(
            matches!(
                read_frame::<_, Response>(&mut stream).await?,
                Response::Done
            ),
            "Incomplete source"
        );
        ensure!(
            crate::model::digest(&received) == manifest.sha256 && received == payload,
            "File integrity mismatch"
        );
        drop(stream);
        Ok::<_, anyhow::Error>(())
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(bridge, source_task, client_task)
    })
    .await??;
    assert!(shared
        .lock()
        .await
        .view()
        .transfers
        .iter()
        .any(|t| t.via_host && t.status == "Completed"));
    Ok(())
}
