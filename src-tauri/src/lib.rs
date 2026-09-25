mod auth;
mod downloader;
mod tidal;
mod transcode;

use std::path::PathBuf;

use auth::{load_session_from_disk, persist_session, AuthSession, AuthState};
use tauri::{Manager, State};
use tidal::TrackInfo;

/// Login pakai kode (device flow) — tidak perlu daftar aplikasi di developer portal.
#[tauri::command]
async fn login_device(
    app: tauri::AppHandle,
    state: State<'_, AuthState>,
) -> Result<AuthSession, String> {
    let session = auth::login_device(&state, app).await?;
    persist_session(&state.data_dir, &session)?;
    *state.session.lock().await = Some(session.clone());
    Ok(session)
}

/// Salin teks ke clipboard (dipakai tombol "Salin" di layar login).
#[tauri::command]
fn copy_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn auth_status(state: State<'_, AuthState>) -> Result<serde_json::Value, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let guard = state.session.lock().await;
    Ok(match guard.as_ref() {
        Some(s) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            serde_json::json!({
                "loggedIn": true,
                "userId": s.user_id,
                "expired": now >= s.expires_at,
            })
        }
        None => serde_json::json!({ "loggedIn": false }),
    })
}

#[tauri::command]
async fn logout(state: State<'_, AuthState>) -> Result<(), String> {
    *state.session.lock().await = None;
    let _ = std::fs::remove_file(state.data_dir.join("auth.json"));
    Ok(())
}

#[tauri::command]
async fn search(state: State<'_, AuthState>, query: String) -> Result<Vec<TrackInfo>, String> {
    tidal::resolve_query(&state, &query).await
}

#[tauri::command]
async fn album_tracks(
    state: State<'_, AuthState>,
    album_id: String,
) -> Result<Vec<TrackInfo>, String> {
    tidal::get_album_tracks(&state, &album_id).await
}

/// Unduh satu track. `format`: `original` (apa adanya) / `mp3_same` / `mp3_vbr0`.
#[tauri::command]
async fn download_track(
    app: tauri::AppHandle,
    state: State<'_, AuthState>,
    track: TrackInfo,
    quality: String,
    format: Option<String>,
    dir: String,
) -> Result<String, String> {
    let path = downloader::download_track(
        app,
        &state,
        &track,
        &quality,
        format.as_deref().unwrap_or("original"),
        PathBuf::from(&dir),
    )
    .await?;
    Ok(path.display().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("gagal resolve app data dir");
            let session = load_session_from_disk(&data_dir);
            app.manage(AuthState {
                client: reqwest::Client::new(),
                data_dir,
                session: tokio::sync::Mutex::new(session),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            login_device,
            copy_text,
            auth_status,
            logout,
            search,
            album_tracks,
            download_track
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

