use crate::{
    engine::{self, Action, Engine, Shared, View},
    network::Node,
};
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use uuid::Uuid;

struct Desktop {
    shared: Shared,
    node: Node,
}

#[derive(Default)]
struct Updates(tokio::sync::Mutex<Option<Update>>);

#[derive(Clone, serde::Serialize)]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
async fn check_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, Updates>,
) -> Result<Option<String>, String> {
    let mut cached = state.0.lock().await;
    *cached = None;
    let mut update = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| {
            format!(
                "Could not check for updates. Check your internet connection and try again. {e}"
            )
        })?;
    if let Some(update) = &mut update {
        // The offline WebView2 installer makes this a large download on slower connections.
        update.timeout = Some(std::time::Duration::from_secs(15 * 60));
    }
    let version = update.as_ref().map(|u| u.version.clone());
    *cached = update;
    Ok(version)
}

fn update_ready(engine: &Engine) -> Result<(), String> {
    if !engine.preparing.is_empty()
        || engine
            .transfers
            .values()
            .any(|t| !matches!(t.status.as_str(), "Completed" | "Cancelled" | "Failed"))
    {
        return Err("Finish or cancel your transfers before updating.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn updates_wait_for_transfers_and_file_preparation() {
        let directory = tempfile::tempdir().unwrap();
        let shared = Engine::open(directory.path(), std::sync::Arc::new(|_| {})).unwrap();
        let mut engine = shared.lock().await;
        assert!(update_ready(&engine).is_ok());
        engine.preparing.push("file".into());
        assert!(update_ready(&engine).is_err());
        engine.preparing.clear();
        let id = Uuid::new_v4();
        engine.transfers.insert(
            id,
            engine::Transfer {
                id,
                file_id: id,
                name: "file".into(),
                peer_id: "peer".into(),
                direction: "Receiving".into(),
                relayed: false,
                bytes: 0,
                total: 1,
                status: "Queued".into(),
                error: None,
            },
        );
        for status in ["Queued", "Connecting", "Receiving", "Sending"] {
            engine.transfers.get_mut(&id).unwrap().status = status.into();
            assert!(update_ready(&engine).is_err());
        }
        for status in ["Completed", "Cancelled", "Failed"] {
            engine.transfers.get_mut(&id).unwrap().status = status.into();
            assert!(update_ready(&engine).is_ok());
        }
    }
}

#[tauri::command]
async fn install_update(
    state: tauri::State<'_, Updates>,
    desktop: tauri::State<'_, Desktop>,
    version: String,
    on_progress: tauri::ipc::Channel<UpdateProgress>,
) -> Result<(), String> {
    update_ready(&*desktop.shared.lock().await)?;
    let mut cached = state.0.lock().await;
    let update = cached
        .as_ref()
        .filter(|u| u.version == version)
        .ok_or("Check for updates again before installing.")?;
    let mut downloaded = 0;
    // The platform updater verifies the pinned signing key before returning these bytes.
    let bytes = update
        .download(
            |length, total| {
                downloaded += length as u64;
                let _ = on_progress.send(UpdateProgress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("Update download or signature verification failed: {e}"))?;
    update_ready(&*desktop.shared.lock().await)?;
    update
        .install(bytes)
        .map_err(|e| format!("Could not start the update installer: {e}"))?;
    *cached = None;
    Ok(())
}

#[tauri::command]
async fn get_state(state: tauri::State<'_, Desktop>) -> Result<View, String> {
    Ok(state.shared.lock().await.view())
}
#[tauri::command]
async fn save_network(
    state: tauri::State<'_, Desktop>,
    settings: crate::relay_host::Settings,
) -> Result<(), String> {
    let mut e = state.shared.lock().await;
    settings
        .validate(
            e.persisted
                .relay_key()
                .map_err(|err| err.to_string())?
                .public()
                .to_peer_id(),
        )
        .map_err(|err| err.to_string())?;
    let mut next = e.persisted.clone();
    next.network = settings;
    next.relay_identity_migrated = false;
    e.persist(next).map_err(|err| err.to_string())?;
    e.emit();
    Ok(())
}
#[tauri::command]
async fn dispatch(state: tauri::State<'_, Desktop>, action: Action) -> Result<(), String> {
    engine::dispatch(&mut *state.shared.lock().await, action).map_err(|e| e.to_string())
}
#[tauri::command]
async fn choose_share_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, Desktop>,
) -> Result<(), String> {
    let workspace = state
        .shared
        .lock()
        .await
        .active_snapshot()
        .map_err(|e| e.to_string())?
        .workspace_id;
    let paths = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose files to share")
            .blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Some(paths) = paths {
        let paths = paths
            .into_iter()
            .map(|p| p.into_path())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let shared = state.shared.clone();
        // Hash off the IPC request so large files do not freeze controls.
        tauri::async_runtime::spawn(async move {
            if let Err(err) = engine::share_paths_to(shared.clone(), paths, workspace).await {
                shared.lock().await.error(err.to_string());
            }
        });
    }
    Ok(())
}
#[tauri::command]
async fn share_dropped_files(
    state: tauri::State<'_, Desktop>,
    paths: Vec<std::path::PathBuf>,
    workspace: Uuid,
) -> Result<(), String> {
    let shared = state.shared.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = engine::share_paths_to(shared.clone(), paths, workspace).await {
            shared.lock().await.error(err.to_string());
        }
    });
    Ok(())
}
#[tauri::command]
async fn receive_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, Desktop>,
    file_id: Uuid,
) -> Result<(), String> {
    let folder = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose a folder for the received file")
            .blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Some(folder) = folder {
        state
            .node
            .receive(file_id, folder.into_path().map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn accept_url(shared: Shared, url: String) {
    tauri::async_runtime::spawn(async move {
        let mut e = shared.lock().await;
        if let Err(err) = e.join(&url) {
            e.error(format!("Could not open invitation: {err}"));
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Updates::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let shared = Engine::open(
                &app.path().app_local_data_dir()?,
                std::sync::Arc::new(move |view| {
                    let _ = handle.emit("dump-state", view);
                }),
            )?;
            let node = tauri::async_runtime::block_on(Node::start(shared.clone(), true))?;
            app.manage(Desktop {
                shared: shared.clone(),
                node,
            });
            if let Some(urls) = app.deep_link().get_current()? {
                for url in urls {
                    accept_url(shared.clone(), url.to_string());
                }
            }
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    accept_url(shared.clone(), url.to_string());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_network,
            dispatch,
            choose_share_files,
            share_dropped_files,
            receive_file,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("Dump could not start; check local data permissions and networking");
}
