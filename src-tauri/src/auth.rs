use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tauri_plugin_clipboard_manager::ClipboardExt;

// --- OAuth 2.0 Device Authorization Grant (login pakai kode di link.tidal.com) ---
// Memakai kredensial publik klien TIDAL (dipublikasikan proyek open-source
// seperti TidalLib) sehingga pengguna TIDAK perlu mendaftar aplikasi sendiri.
const TOKEN_URL: &str = "https://auth.tidal.com/v1/oauth2/token";
const DEVICE_AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2/device_authorization";
const DEVICE_CLIENT_ID: &str = "zU4XHVVkc2tDPo4t";
const DEVICE_CLIENT_SECRET: &str = "VJKhDFqJPqvsPVNBV6ukXTJmwlvbttP7wlMlrc72se4=";
/// Scope yang diminta saat login device — sama seperti tidal-dl yaronzz
/// yang terbukti berhasil; tanpa scope ini endpoint v1 (metadata & playback)
/// membalas 403 "Token is missing required scope".
const DEVICE_SCOPE: &str = "r_usr+w_usr+w_sub";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Serialize, Clone)]
struct DeviceLoginEvent {
    stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_uri: Option<String>,
    #[serde(rename = "verificationUriComplete", skip_serializing_if = "Option::is_none")]
    verification_uri_complete: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    copied: Option<bool>,
    #[serde(rename = "browserOpened", skip_serializing_if = "Option::is_none")]
    browser_opened: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub user_id: Option<u64>,
    /// client_id yang menerbitkan token (dipakai saat refresh)
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

pub struct AuthState {
    pub session: tokio::sync::Mutex<Option<AuthSession>>,
    pub client: reqwest::Client,
    pub data_dir: PathBuf,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    user: Option<serde_json::Value>,
}

pub fn persist_session(dir: &Path, session: &AuthSession) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("auth.json"), json).map_err(|e| e.to_string())
}

pub fn load_session_from_disk(dir: &Path) -> Option<AuthSession> {
    let json = std::fs::read_to_string(dir.join("auth.json")).ok()?;
    serde_json::from_str(&json).ok()
}

pub async fn request_token(state: &AuthState, form: &[(&str, &str)]) -> Result<AuthSession, String> {
    let resp = state
        .client
        .post(TOKEN_URL)
        .form(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token request gagal: {status} - {body}"));
    }
    let tr: TokenResponse = resp.json().await.map_err(|e| e.to_string())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    Ok(AuthSession {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token.unwrap_or_default(),
        expires_at: now + tr.expires_in.max(0) as u64 - 60,
        user_id: tr.user.and_then(|u| u["id"].as_u64()),
        ..Default::default()
    })
}

/// Login pakai kode: OAuth 2.0 Device Authorization Grant (tanpa registrasi aplikasi).
pub async fn login_device(state: &AuthState, app: tauri::AppHandle) -> Result<AuthSession, String> {
    let resp = state
        .client
        .post(DEVICE_AUTH_URL)
        .form(&[("client_id", DEVICE_CLIENT_ID), ("scope", DEVICE_SCOPE)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("gagal meminta device code: {status} - {body}"));
    }
    let device: DeviceAuthResponse = resp.json().await.map_err(|e| e.to_string())?;

    // Link login (sudah memuat kode) disalin ke clipboard sebagai jalur utama,
    // karena auto-open browser bisa gagal di sebagian lingkungan Windows.
    let link = device
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| device.verification_uri.clone());
    let copied = app.clipboard().write_text(link.clone()).is_ok();

    // Best-effort: coba buka browser default, tapi jangan bergantung padanya.
    let browser_opened = tauri_plugin_opener::open_url(link.clone(), None::<&str>).is_ok();
    let message = if copied {
        Some(format!("Link login disalin ke clipboard: {link}"))
    } else {
        Some(format!("Salin manual link ini: {link}"))
    };

    let _ = app.emit(
        "device-login",
        DeviceLoginEvent {
            stage: "code".into(),
            user_code: Some(device.user_code.clone()),
            verification_uri: Some(device.verification_uri.clone()),
            verification_uri_complete: Some(link),
            copied: Some(copied),
            browser_opened: Some(browser_opened),
            message,
        },
    );

    let interval = device.interval.unwrap_or(5).max(1);
    let expires_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
        + device.expires_in;
    // Server TIDAL bisa meminta client_secret pada token request; mulai dengan
    // secret dan ulangi tanpa secret bila server menolaknya.
    let mut with_secret = true;

    loop {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        if now >= expires_at {
            return device_error(&app, "kode login kadaluarsa, silakan ulangi");
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        // RFC 8628: token endpoint memakai `device_code` — BUKAN `code`.
        // (Salah nama parameter = 400 invalid_request "Missing parameters: device_code"
        //  sehingga polling berhenti sebelum user sempat menyetujui.)
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device.device_code.as_str()),
            ("client_id", DEVICE_CLIENT_ID),
            ("scope", DEVICE_SCOPE),
        ];
        if with_secret {
            form.push(("client_secret", DEVICE_CLIENT_SECRET));
        }
        let resp = state
            .client
            .post(TOKEN_URL)
            .form(&form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if resp.status().is_success() {
            let tr: TokenResponse = resp.json().await.map_err(|e| e.to_string())?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let session = AuthSession {
                access_token: tr.access_token,
                refresh_token: tr.refresh_token.unwrap_or_default(),
                expires_at: now + tr.expires_in.max(0) as u64 - 60,
                user_id: tr.user.and_then(|u| u["id"].as_u64()),
                client_id: DEVICE_CLIENT_ID.to_string(),
                client_secret: Some(DEVICE_CLIENT_SECRET.to_string()),
            };
            let _ = app.emit(
                "device-login",
                DeviceLoginEvent {
                    stage: "done".into(),
                    user_code: None,
                    verification_uri: None,
                    verification_uri_complete: None,
                    copied: None,
                    browser_opened: None,
                    message: None,
                },
            );
            return Ok(session);
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<TokenError>(&body).ok();
        let code = parsed
            .as_ref()
            .map(|e| e.error.clone())
            .unwrap_or_default();
        match code.as_str() {
            // user belum menyetujui — terus polling sampai batas expires_at
            "authorization_pending" => continue,
            "slow_down" => {
                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                continue;
            }
            "expired_token" => {
                return device_error(&app, "kode login kadaluarsa, silakan ulangi");
            }
            "access_denied" => {
                return device_error(&app, "login dibatalkan di browser");
            }
            _ => {
                // Hanya retry-tanpa-secret bila server memang menolak client_secret;
                // error lain dilaporkan apa adanya (jangan menyembunyikan error nyata).
                let detail = parsed
                    .and_then(|e| e.error_description)
                    .unwrap_or(body);
                let secret_rejected = with_secret
                    && (code == "invalid_client"
                        || code == "unauthorized_client"
                        || detail.contains("client_secret"));
                if secret_rejected {
                    with_secret = false;
                    continue;
                }
                return device_error(
                    &app,
                    &format!("device login gagal ({status}): {detail}"),
                );
            }
        }
    }
}

fn device_error(app: &tauri::AppHandle, message: &str) -> Result<AuthSession, String> {
    let _ = app.emit(
        "device-login",
        DeviceLoginEvent {
            stage: "error".into(),
            user_code: None,
            verification_uri: None,
            verification_uri_complete: None,
            copied: None,
            browser_opened: None,
            message: Some(message.to_string()),
        },
    );
    Err(message.to_string())
}

pub async fn ensure_access_token(state: &AuthState) -> Result<String, String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    {
        let guard = state.session.lock().await;
        match guard.as_ref() {
            None => return Err("belum login - silakan login dulu".into()),
            Some(s) if now < s.expires_at => return Ok(s.access_token.clone()),
            Some(s) => {
                if s.refresh_token.is_empty() {
                    return Err("session kadaluarsa, silakan login ulang".into());
                }
            }
        }
    }
    let (refresh_token, client_id, client_secret) = {
        let guard = state.session.lock().await;
        let s = guard.as_ref().unwrap();
        let client_id = if s.client_id.is_empty() {
            DEVICE_CLIENT_ID.to_string()
        } else {
            s.client_id.clone()
        };
        let client_secret = match &s.client_secret {
            Some(secret) => secret.clone(),
            None => DEVICE_CLIENT_SECRET.to_string(),
        };
        (s.refresh_token.clone(), client_id, client_secret)
    };
    let refreshed = request_token(
        state,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", &client_id),
            ("client_secret", &client_secret),
            ("scope", DEVICE_SCOPE),
        ],
    )
    .await?;
    let token = refreshed.access_token.clone();
    *state.session.lock().await = Some(refreshed.clone());
    persist_session(&state.data_dir, &refreshed)?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::TokenError;

    #[test]
    fn parses_tidal_token_error_response() {
        // Error asli yang pernah muncul saat nama parameter salah (`code` vs `device_code`)
        let e: TokenError = serde_json::from_str(
            r#"{"error":"invalid_request","error_description":"Missing parameters: device_code"}"#,
        )
        .unwrap();
        assert_eq!(e.error, "invalid_request");
        assert_eq!(
            e.error_description.as_deref(),
            Some("Missing parameters: device_code")
        );

        // Bentuk standar saat user belum menyetujui
        let pending: TokenError =
            serde_json::from_str(r#"{"error":"authorization_pending"}"#).unwrap();
        assert_eq!(pending.error, "authorization_pending");
        assert!(pending.error_description.is_none());
    }
}

