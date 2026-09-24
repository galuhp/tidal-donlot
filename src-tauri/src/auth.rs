use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tauri_plugin_clipboard_manager::ClipboardExt;

// --- OAuth 2.0 Device Authorization Grant (login pakai kode di link.tidal.com) ---
// Memakai kredensial publik klien TIDAL (klien **TV / AndroidTV**) — kredensial
// yang sama dipakai modul TIDAL OrpheusDL (github.com/bascurtiz/orpheusdl-tidal,
// `interface.py` → `TidalTvSession(settings['tv_atmos_token'], settings['tv_atmos_secret'])`).
// Klien TV inilah yang diizinkan TIDAL mengakses endpoint playback; klien
// web/desktop publik (mis. `zU4XHVVkc2tDPo4t`) hanya dapat metadata.
const TOKEN_URL: &str = "https://auth.tidal.com/v1/oauth2/token";
const DEVICE_AUTH_URL: &str = "https://auth.tidal.com/v1/oauth2/device_authorization";
const DEVICE_CLIENT_ID: &str = "4N3n6Q1x95LL5K7p";
const DEVICE_CLIENT_SECRET: &str = "oKOXfJW371cX6xaZ0PyhgGNBdNLlBZd4AKKYougMjik=";
/// Scope klien TV — OrpheusDL memakai `scope: 'r_usr w_usr'` (tanpa `playback`).
/// Menambah `playback` ditolak TIDAL (`invalid_scope`); izin stream ditentukan
/// oleh **client id** yang dikirim di header `X-Tidal-Token`, bukan scope.
const DEVICE_SCOPE: &str = "r_usr w_usr";
/// User-Agent klien Android TV. Wajib pada panggilan `api.tidal.com/v1` supaya
/// TIDAL mengenali permintaan sebagai klien TV (sama seperti `auth_headers()`
/// di OrpheusDL-TIDAL). Dipakai oleh `tidal.rs` saat resolve stream.
pub const TIDAL_ANDROID_UA: &str = "TIDAL_ANDROID/1039 okhttp/3.14.9";

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
    /// Kode negara akun (dipakai sebagai `countryCode` saat request API).
    #[serde(default)]
    pub country_code: Option<String>,
    /// client_id yang menerbitkan token (dipakai saat refresh)
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

/// Ambil `userId` + `countryCode` dari objek `user` pada respons token.
/// TIDAL memakai kunci `userId` (bukan `id`) — sama seperti tidal-dl (yaronzz).
fn user_fields(user: &Option<serde_json::Value>) -> (Option<u64>, Option<String>) {
    match user {
        Some(u) => (
            u["userId"].as_u64().or_else(|| u["id"].as_u64()),
            u["countryCode"]
                .as_str()
                .filter(|c| !c.is_empty())
                .map(String::from),
        ),
        None => (None, None),
    }
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

/// Lengkapi URL TIDAL dengan skema. Endpoint device TIDAL mengirim nilai tanpa
/// `https://` (`link.tidal.com/ABCD`), sehingga `open_url` gagal dan clipboard
/// berisi teks yang tidak bisa langsung diklik.
fn absolute_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed.trim_start_matches('/'))
    }
}

pub fn persist_session(dir: &Path, session: &AuthSession) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("auth.json"), json).map_err(|e| e.to_string())
}

pub fn load_session_from_disk(dir: &Path) -> Option<AuthSession> {
    let json = std::fs::read_to_string(dir.join("auth.json")).ok()?;
    serde_json::from_str::<AuthSession>(&json)
        .ok()
        .map(migrate_session_client)
}

/// Sesi lama diterbitkan klien device non-TV (versi aplikasi sebelum ini), dan
/// token klien itu tidak bisa dipakai playback. Refresh token tetap sah lintas
/// client id — `auth_session()` di OrpheusDL memakai trik yang sama untuk
/// "switch to any client type from an existing session" — jadi cukup kosongkan
/// `client_id` agar refresh berikutnya memakai klien TV (tanpa login ulang).
pub fn migrate_session_client(mut session: AuthSession) -> AuthSession {
    if session.client_id != DEVICE_CLIENT_ID {
        session.client_id = String::new();
        session.client_secret = None;
    }
    session
}

/// `client_id` pemilik sesi aktif, dipakai `tidal.rs` sebagai header
/// `X-Tidal-Token` pada panggilan `api.tidal.com/v1` (endpoint playback).
pub async fn session_client_id(state: &AuthState) -> String {
    let guard = state.session.lock().await;
    match guard.as_ref() {
        Some(s) if !s.client_id.is_empty() => s.client_id.clone(),
        _ => DEVICE_CLIENT_ID.to_string(),
    }
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
    let (user_id, country_code) = user_fields(&tr.user);
    Ok(AuthSession {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token.unwrap_or_default(),
        expires_at: now + tr.expires_in.max(0) as u64 - 60,
        user_id,
        country_code,
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
    // TIDAL membalas `verificationUri`/`verificationUriComplete` TANPA skema
    // (mis. `link.tidal.com/ABCD`) sehingga URL-nya harus dilengkapi `https://`
    // dulu; kalau `...Complete` tidak ada, rakit sendiri dari user code
    // (pola OrpheusDL: `https://link.tidal.com/<userCode>`).
    let raw_link = device
        .verification_uri_complete
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("{}/{}", device.verification_uri, device.user_code));
    let link = absolute_url(&raw_link);
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
            verification_uri: Some(absolute_url(&device.verification_uri)),
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
            let (user_id, country_code) = user_fields(&tr.user);
            let session = AuthSession {
                access_token: tr.access_token,
                refresh_token: tr.refresh_token.unwrap_or_default(),
                expires_at: now + tr.expires_in.max(0) as u64 - 60,
                user_id,
                country_code,
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
    // OrpheusDL (`TidalTvSession.refresh`) mengirim refresh_token + client_id +
    // client_secret saja — tanpa `scope`; mengirim scope pada grant refresh bisa
    // ditolak TIDAL, jadi form-nya disamakan.
    let refreshed = request_token(
        state,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", &client_id),
            ("client_secret", &client_secret),
        ],
    )
    .await
    .map_err(|e| format!("{e} — sesi tidak bisa diperbarui; silakan Logout lalu login ulang."))?;
    let token = refreshed.access_token.clone();
    // Gabung (bukan timpa): respons refresh boleh tidak memuat `refresh_token`,
    // `user`, atau kredensial klien. Kalau ditimpa mentah-mentah, sesi bisa
    // kehilangan refresh_token dan user terpaksa login ulang.
    {
        let mut guard = state.session.lock().await;
        if let Some(cur) = guard.as_mut() {
            cur.access_token = refreshed.access_token;
            cur.expires_at = refreshed.expires_at;
            if !refreshed.refresh_token.is_empty() {
                cur.refresh_token = refreshed.refresh_token;
            }
            if refreshed.user_id.is_some() {
                cur.user_id = refreshed.user_id;
            }
            if refreshed.country_code.is_some() {
                cur.country_code = refreshed.country_code;
            }
        }
        if let Some(s) = guard.as_ref() {
            persist_session(&state.data_dir, s)?;
        }
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::{absolute_url, migrate_session_client, AuthSession, TokenError, DEVICE_CLIENT_ID};

    #[test]
    fn absolute_url_adds_missing_scheme() {
        // Bentuk asli balasan TIDAL: tanpa skema (lihat probe `device_authorization`).
        assert_eq!(
            absolute_url("link.tidal.com/ABCD"),
            "https://link.tidal.com/ABCD"
        );
        assert_eq!(
            absolute_url("https://link.tidal.com/ABCD"),
            "https://link.tidal.com/ABCD"
        );
        // toleransi spasi & slash di depan
        assert_eq!(
            absolute_url(" /link.tidal.com/ABCD "),
            "https://link.tidal.com/ABCD"
        );
    }

    #[test]
    fn migrates_session_from_old_non_tv_client() {
        // Sesi lama (klien web/desktop) tidak bisa dipakai playback: client_id
        // dikosongkan supaya refresh memakai klien TV, token tetap dibawa.
        let old = AuthSession {
            access_token: "a".into(),
            refresh_token: "r".into(),
            client_id: "zU4XHVVkc2tDPo4t".into(),
            client_secret: Some("old-secret".into()),
            ..Default::default()
        };
        let migrated = migrate_session_client(old);
        assert!(migrated.client_id.is_empty());
        assert!(migrated.client_secret.is_none());
        assert_eq!(migrated.refresh_token, "r");
    }

    #[test]
    fn keeps_tv_client_session_untouched() {
        let tv = AuthSession {
            client_id: DEVICE_CLIENT_ID.to_string(),
            client_secret: Some("secret".into()),
            refresh_token: "r".into(),
            ..Default::default()
        };
        let kept = migrate_session_client(tv);
        assert_eq!(kept.client_id, DEVICE_CLIENT_ID);
        assert_eq!(kept.client_secret.as_deref(), Some("secret"));
    }

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

