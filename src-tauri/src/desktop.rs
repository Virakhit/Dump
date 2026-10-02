use crate::{
    engine::{self, Action, Engine, Shared, View},
    network::Node,
};
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

struct Desktop {
    shared: Shared,
    node: Node,
}

#[tauri::command]
async fn get_state(state: tauri::State<'_, Desktop>) -> Result<View, String> {
    Ok(state.shared.lock().await.view())
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
            dispatch,
            choose_share_files,
            share_dropped_files,
            receive_file
        ])
        .run(tauri::generate_context!())
        .expect("Dump could not start; check local data permissions and networking");
}
