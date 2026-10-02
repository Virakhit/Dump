#![cfg(feature = "updater-tests")]

use serde_json::json;
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Real HTTP requests through the platform updater; no installer is executed by this check.
#[tokio::test]
async fn signed_updates_reject_tampering_and_version_substitution() -> anyhow::Result<()> {
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))?;
    let mut context = mock_context(noop_assets());
    context.package_info_mut().version = "0.1.1".parse()?;
    let mut updater_config = config["plugins"]["updater"].clone();
    // HTTP is allowed only in this loopback test, never in the shipping config.
    updater_config["dangerousInsecureTransportProtocol"] = json!(true);
    context
        .config_mut()
        .plugins
        .0
        .insert("updater".into(), updater_config);
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)?;
    for (version, tamper, expected_update, expected_download) in [
        ("0.1.0", false, false, false),
        ("0.1.1", false, false, false),
        ("0.2.0", false, true, true),
        ("0.2.0", true, true, false),
        ("0.2.1", false, true, false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).await.unwrap();
                let payload =
                    if String::from_utf8_lossy(&request[..n]).starts_with("GET /installer ") {
                        if tamper {
                            b"tampered installer".to_vec()
                        } else {
                            include_bytes!("fixtures/update.bin").to_vec()
                        }
                    } else {
                        serde_json::to_vec(&json!({
                            "version": version,
                            "platforms": { "windows-x86_64": {
                                "url": format!("http://{address}/installer"),
                                "signature": include_str!("fixtures/update.bin.sig").trim(),
                            } },
                        }))
                        .unwrap()
                    };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(&payload).await.unwrap();
            }
        });
        let updater = app
            .updater_builder()
            .endpoints(vec![format!("http://{address}/manifest").parse()?])?
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let update = updater.check().await?;
        assert_eq!(update.is_some(), expected_update, "version {version}");
        if let Some(update) = update {
            let result = update.download(|_, _| {}, || {}).await;
            assert_eq!(
                result.is_ok(),
                expected_download,
                "version {version}, tamper {tamper}: {result:?}"
            );
            if expected_download {
                assert_eq!(result?, include_bytes!("fixtures/update.bin"));
            }
        }
        server.abort();
    }
    Ok(())
}
