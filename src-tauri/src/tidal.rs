use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{ensure_access_token, session_client_id, AuthState, TIDAL_ANDROID_UA};

const API_BASE: &str = "https://openapi.tidal.com/v2";
const LEGACY_API: &str = "https://api.tidal.com/v1";
const COUNTRY: &str = "US";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub album_id: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<f64>,
    /// Jenis resource: "song" | "album" | "artist" — dipakai UI untuk menandai
    /// tiap hasil pencarian. Payload lama tanpa field ini dianggap "song".
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "song".to_string()
}

/// Gabungkan `attributes.name` dari relationship multi (mis. `artists`) lewat `included`.
fn names_of(res: &Value, rel: &str, by_id: &std::collections::HashMap<&str, &Value>) -> String {
    res["relationships"][rel]["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    by_id
                        .get(a["id"].as_str()?)
                        .map(|r| r["attributes"]["name"].as_str().unwrap_or("").to_string())
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Ambil URL cover lewat relationship `rel`: `coverArt` (album/track) atau
/// `profileArt` (artis). Relationship bisa berupa array (coverArt) atau objek
/// tunggal (profileArt), jadi keduanya ditangani.
fn cover_from(
    res: &Value,
    rel: &str,
    by_id: &std::collections::HashMap<&str, &Value>,
) -> Option<String> {
    let attrs = &res["attributes"];
    // Bentuk lama: imageLinks langsung di atribut album.
    if let Some(links) = attrs["imageLinks"].as_array() {
        if let Some(best) = links
            .iter()
            .max_by_key(|l| l["meta"]["width"].as_u64().unwrap_or(0))
        {
            if let Some(href) = best["href"].as_str() {
                return Some(href.to_string());
            }
        }
    }
    if let Some(url) = attrs["coverArt"]["original"]["url"].as_str() {
        return Some(url.to_string());
    }
    // Bentuk OpenAPI v2: relationships.<rel>.data -> resource `artworks` di
    // `included`, URL gambar di attributes.files[].href (pilih yang terlebar).
    let data = &res["relationships"][rel]["data"];
    let art_id = data[0]["id"].as_str().or_else(|| data["id"].as_str())?;
    let art = by_id.get(art_id)?;
    let best = art["attributes"]["files"]
        .as_array()?
        .iter()
        .max_by_key(|f| f["meta"]["width"].as_u64().unwrap_or(0))?;
    best["href"].as_str().map(String::from)
}

/// Include untuk relationship items (album/playlist): track + artis + album + cover.
const INCLUDE_ITEMS: &str = "items,items.artists,items.albums,items.albums.coverArt";

/// Include untuk dokumen searchResults: lagu, album, dan artis sekaligus supaya
/// tiap hasil bisa ditandai jenisnya (sesuai relationship searchResults di spec).
const SEARCH_INCLUDE: &str = "tracks,tracks.artists,tracks.albums,tracks.albums.coverArt,albums,albums.artists,albums.coverArt,artists,artists.profileArt";

/// GET JSON dengan Bearer token; kegagalan HTTP diberi konteks `label`.
async fn get_json(state: &AuthState, token: &str, url: &str, label: &str) -> Result<Value, String> {
    let resp = state
        .client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{label}: {status} - {body}"));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// URL halaman berikutnya dari `links.next` (biasanya path relatif), dibuat
/// absolut dan dipastikan masih membawa parameter `include` lengkap.
fn next_page_url(json: &Value, include: &str) -> Option<String> {
    let next = json["links"]["next"].as_str()?;
    let mut url = if next.starts_with("http") {
        next.to_string()
    } else {
        format!("{API_BASE}{next}")
    };
    // Server kadang tidak meneruskan include di link berikutnya (discussion #47).
    if !url.contains("include=") {
        url.push_str(&format!("&include={include}"));
    }
    Some(url)
}

/// Kumpulkan semua halaman relationship `items` lalu gabung `data` + `included`
/// menjadi satu dokumen JSON:API agar diparse sekali oleh `parse_tracks`.
async fn get_all_pages(
    state: &AuthState,
    token: &str,
    first_url: &str,
    label: &str,
    include: &str,
) -> Result<Value, String> {
    let mut data: Vec<Value> = Vec::new();
    let mut included: Vec<Value> = Vec::new();
    let mut url: Option<String> = Some(first_url.to_string());
    let mut pages = 0usize;
    while let Some(u) = url {
        pages += 1;
        if pages > 500 {
            break; // pengaman loop tak berujung
        }
        let json = get_json(state, token, &u, label).await?;
        match &json["data"] {
            Value::Array(items) => data.extend(items.iter().cloned()),
            Value::Object(_) => data.push(json["data"].clone()),
            _ => {}
        }
        if let Value::Array(items) = &json["included"] {
            included.extend(items.iter().cloned());
        }
        url = next_page_url(&json, include);
    }
    Ok(serde_json::json!({ "data": data, "included": included }))
}

/// Durasi v2 berbentuk string ISO 8601 ("PT4M16S"); bentuk lama angka detik.
fn parse_duration(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let s = v.as_str()?;
    let body = s.strip_prefix('P')?;
    let (date, time) = match body.find('T') {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, ""),
    };
    let mut total = 0.0f64;
    if !date.is_empty() {
        let days = date.strip_suffix('D')?;
        if !days.is_empty() {
            total += days.parse::<f64>().ok()? * 86_400.0;
        }
    }
    let mut num = String::new();
    for c in time.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        if num.is_empty() {
            return None;
        }
        let n: f64 = num.parse().ok()?;
        total += match c {
            'H' => n * 3_600.0,
            'M' => n * 60.0,
            'S' => n,
            _ => return None,
        };
        num.clear();
    }
    if !num.is_empty() {
        return None;
    }
    Some(total)
}

fn build_track(track: &Value, by_id: &std::collections::HashMap<&str, &Value>) -> TrackInfo {
    let attrs = &track["attributes"];
    let artists = names_of(track, "artists", by_id);
    let album_id = track["relationships"]["albums"]["data"][0]["id"]
        .as_str()
        .map(|s| s.to_string());
    let (album, cover_url) = album_id
        .as_deref()
        .and_then(|aid| by_id.get(aid))
        .map(|a| {
            (
                a["attributes"]["title"].as_str().unwrap_or("").to_string(),
                cover_from(a, "coverArt", by_id),
            )
        })
        .unwrap_or_default();
    TrackInfo {
        id: track["id"].as_str().unwrap_or_default().to_string(),
        title: attrs["title"].as_str().unwrap_or("").to_string(),
        artists,
        album,
        album_id,
        cover_url,
        duration: parse_duration(&attrs["duration"]),
        kind: "song".to_string(),
    }
}

/// Hasil pencarian berupa album: title = judul album, artists = artis album.
fn build_album(album: &Value, by_id: &std::collections::HashMap<&str, &Value>) -> TrackInfo {
    let id = album["id"].as_str().unwrap_or_default().to_string();
    TrackInfo {
        title: album["attributes"]["title"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        artists: names_of(album, "artists", by_id),
        album: String::new(),
        album_id: Some(id.clone()),
        cover_url: cover_from(album, "coverArt", by_id),
        duration: parse_duration(&album["attributes"]["duration"]),
        kind: "album".to_string(),
        id,
    }
}

/// Hasil pencarian berupa artis: title = nama artis, cover dari `profileArt`.
fn build_artist(artist: &Value, by_id: &std::collections::HashMap<&str, &Value>) -> TrackInfo {
    TrackInfo {
        id: artist["id"].as_str().unwrap_or_default().to_string(),
        title: artist["attributes"]["name"].as_str().unwrap_or("").to_string(),
        artists: String::new(),
        album: String::new(),
        album_id: None,
        cover_url: cover_from(artist, "profileArt", by_id),
        duration: None,
        kind: "artist".to_string(),
    }
}

/// Parser JSON:API generik — mendukung tiga bentuk respons:
/// 1. searchResults: `data` = searchResults → `relationships.tracks`
/// 2. relationships (album/playlist): `data` = array resource tracks
/// 3. single resource: `data` = object track
fn parse_tracks(json: &Value) -> Vec<TrackInfo> {
    let mut by_id: std::collections::HashMap<&str, &Value> = Default::default();
    if let Some(included) = json["included"].as_array() {
        for r in included {
            if let Some(id) = r["id"].as_str() {
                by_id.insert(id, r);
            }
        }
    }
    let data_items: Vec<&Value> = match &json["data"] {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&json["data"]],
        _ => Vec::new(),
    };
    // resource di `data` juga ikut jadi lookup (untuk single/full resource)
    for it in &data_items {
        if let Some(id) = it["id"].as_str() {
            by_id.entry(id).or_insert(it);
        }
    }

    let mut refs: Vec<&Value> = Vec::new();
    let mut album_refs: Vec<&Value> = Vec::new();
    let mut artist_refs: Vec<&Value> = Vec::new();
    for it in &data_items {
        if it["type"].as_str() == Some("tracks") {
            refs.push(it);
        } else if let Some(sub) = it["relationships"]["tracks"]["data"].as_array() {
            refs.extend(sub.iter());
        }
        // Dokumen searchResults juga memuat albums/artists → ikut ditampilkan
        // supaya user bisa membedakan lagu / album / artis.
        if it["type"].as_str() == Some("searchResults") {
            if let Some(sub) = it["relationships"]["albums"]["data"].as_array() {
                album_refs.extend(sub.iter());
            }
            if let Some(sub) = it["relationships"]["artists"]["data"].as_array() {
                artist_refs.extend(sub.iter());
            }
        }
    }

    let mut out: Vec<TrackInfo> = refs
        .iter()
        .filter_map(|r| {
            let id = r["id"].as_str()?;
            let full = by_id.get(id).copied().unwrap_or(r);
            // link tanpa atribut title tidak bisa dipakai untuk download
            if full["attributes"]["title"].is_null() && r["attributes"]["title"].is_null() {
                return None;
            }
            Some(build_track(full, &by_id))
        })
        .collect();
    for r in album_refs {
        if let Some(a) = r["id"].as_str().and_then(|id| by_id.get(id)) {
            if !a["attributes"]["title"].is_null() {
                out.push(build_album(a, &by_id));
            }
        }
    }
    for r in artist_refs {
        if let Some(a) = r["id"].as_str().and_then(|id| by_id.get(id)) {
            if !a["attributes"]["name"].is_null() {
                out.push(build_artist(a, &by_id));
            }
        }
    }
    out
}

enum TidalTarget {
    Album(String),
    Track(String),
    Playlist(String),
    Artist(String),
}

/// Deteksi link TIDAL → target resource yang sesuai.
/// Contoh yang didukung:
///   https://tidal.com/album/519355016
///   https://tidal.com/track/123456?u=x
///   https://tidal.com/playlist/e0f0f0f0-1111-2222-3333-444455556666
///   https://tidal.com/artist/123456
fn parse_tidal_url(input: &str) -> Option<TidalTarget> {
    let s = input.trim();
    if !(s.contains("tidal.com") || s.contains("tidalhifi.com")) {
        return None;
    }
    let rules: [(&str, fn(String) -> TidalTarget); 4] = [
        ("/album/", TidalTarget::Album),
        ("/track/", TidalTarget::Track),
        ("/playlist/", TidalTarget::Playlist),
        ("/artist/", TidalTarget::Artist),
    ];
    for (needle, ctor) in rules {
        if let Some(pos) = s.find(needle) {
            let rest = &s[pos + needle.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !id.is_empty() {
                return Some(ctor(id));
            }
        }
    }
    None
}

/// Titik masuk utama dari kolom pencarian: deteksi link TIDAL dulu,
/// baru fallback ke pencarian teks biasa.
pub async fn resolve_query(state: &AuthState, input: &str) -> Result<Vec<TrackInfo>, String> {
    let trimmed = input.trim();
    match parse_tidal_url(trimmed) {
        Some(TidalTarget::Album(id)) => get_album_tracks(state, &id).await,
        Some(TidalTarget::Track(id)) => get_track(state, &id).await,
        Some(TidalTarget::Playlist(id)) => get_playlist_tracks(state, &id).await,
        Some(TidalTarget::Artist(id)) => {
            let name = get_artist_name(state, &id).await?;
            search(state, &name).await
        }
        None => {
            if trimmed.contains("://") || trimmed.starts_with("www.") {
                return Err(
                    "Link tidak dikenali. Didukung: tidal.com/album, /track, /playlist, /artist"
                        .into(),
                );
            }
            search(state, trimmed).await
        }
    }
}

pub async fn search(state: &AuthState, query: &str) -> Result<Vec<TrackInfo>, String> {
    let token = ensure_access_token(state).await?;
    // Langkah 1: `/searchResults` hanya menerima `filter[query]`; menaruh kata kunci
    // sebagai id (`/searchResults/bohemian`) dibalas 400 INVALID_RESOURCE_ID
    // (diverifikasi langsung ke api TIDAL).
    let lookup = format!(
        "{API_BASE}/searchResults?countryCode={COUNTRY}&filter%5Bquery%5D={}",
        urlencoding::encode(query)
    );
    let found = get_json(state, &token, &lookup, "search").await?;
    let search_id = found["data"][0]["id"]
        .as_str()
        .or_else(|| found["data"]["id"].as_str())
        .ok_or("hasil pencarian kosong")?
        .to_string();
    // Langkah 2: lagu + album + artis (supaya jenis tiap hasil bisa dibedakan).
    let url = format!(
        "{API_BASE}/searchResults/{search_id}?countryCode={COUNTRY}&include={SEARCH_INCLUDE}"
    );
    match get_json(state, &token, &url, "search").await {
        Ok(json) => Ok(parse_tracks(&json)),
        // Jaring pengaman: kalau salah satu include tidak didukung versi API saat
        // ini, ulangi dengan lagu saja supaya pencarian tetap berfungsi.
        Err(_) => {
            let fallback = format!(
                "{API_BASE}/searchResults/{search_id}?countryCode={COUNTRY}&include=tracks,tracks.artists,tracks.albums,tracks.albums.coverArt"
            );
            let json = get_json(state, &token, &fallback, "search").await?;
            Ok(parse_tracks(&json))
        }
    }
}

pub async fn get_album_tracks(
    state: &AuthState,
    album_id: &str,
) -> Result<Vec<TrackInfo>, String> {
    let token = ensure_access_token(state).await?;
    // Relationship yang benar untuk album adalah `items` (bukan `tracks`); include
    // `items.albums.coverArt` membuat URL cover ikut terkirim (diverifikasi ke api).
    let url = format!(
        "{API_BASE}/albums/{album_id}/relationships/items?countryCode={COUNTRY}&include={INCLUDE_ITEMS}"
    );
    let json = get_all_pages(state, &token, &url, "album tracks", INCLUDE_ITEMS).await?;
    Ok(parse_tracks(&json))
}

pub async fn get_track(state: &AuthState, track_id: &str) -> Result<Vec<TrackInfo>, String> {
    let token = ensure_access_token(state).await?;
    // `albums.coverArt` wajib ada agar resource `artworks` ikut di `included`
    // sehingga URL cover bisa dirakit cover_from_album.
    let url = format!(
        "{API_BASE}/tracks/{track_id}?countryCode={COUNTRY}&include=albums,albums.coverArt,artists"
    );
    let resp = state
        .client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("track gagal: {status} - {body}"));
    }
    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    let tracks = parse_tracks(&json);
    if tracks.is_empty() {
        return Err("track tidak ditemukan".into());
    }
    Ok(tracks)
}

pub async fn get_playlist_tracks(
    state: &AuthState,
    playlist_id: &str,
) -> Result<Vec<TrackInfo>, String> {
    let token = ensure_access_token(state).await?;
    // Sama seperti album: pakai relationship `items`; `tracks` dibalas error.
    // Playlist panjang dipaginasi lewat `links.next` (cursor) oleh get_all_pages.
    let url = format!(
        "{API_BASE}/playlists/{playlist_id}/relationships/items?countryCode={COUNTRY}&include={INCLUDE_ITEMS}"
    );
    let json = get_all_pages(state, &token, &url, "playlist tracks", INCLUDE_ITEMS).await?;
    Ok(parse_tracks(&json))
}

pub async fn get_artist_name(state: &AuthState, artist_id: &str) -> Result<String, String> {
    let token = ensure_access_token(state).await?;
    let url = format!("{API_BASE}/artists/{artist_id}?countryCode={COUNTRY}");
    let resp = state
        .client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("artis gagal: {status} - {body}"));
    }
    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    let name = json["data"]["attributes"]["name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Err("artis tidak ditemukan".into());
    }
    Ok(name)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackInfo {
    #[serde(default)]
    manifest_mime_type: String,
    #[serde(default)]
    manifest: String,
}

/// Kode negara akun untuk request API (diambil dari token); fallback ke default.
async fn api_country(state: &AuthState) -> String {
    let guard = state.session.lock().await;
    guard
        .as_ref()
        .and_then(|s| s.country_code.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| COUNTRY.to_string())
}

/// URL endpoint playback v1. `v4` + `prefetch=false` sama seperti OrpheusDL
/// (`get_stream_url`: `tracks/{id}/playbackinfopostpaywall/v4`); tanpa suffix
/// `v4` TIDAL membalas 4005 / "Asset is not ready for playback".
fn playbackinfo_url(track_id: &str, quality: &str, country: &str) -> String {
    format!(
        "{LEGACY_API}/tracks/{track_id}/playbackinfopostpaywall/v4\
         ?playbackmode=STREAM&assetpresentation=FULL&audioquality={quality}\
         &prefetch=false&countryCode={country}"
    )
}

/// GET ke `api.tidal.com/v1` dengan header klien TV. Bukan `bearer_auth` biasa:
/// TIDAL memutuskan izin playback dari header `X-Tidal-Token` (client id) plus
/// User-Agent klien TV — inilah `auth_headers()` di OrpheusDL-TIDAL.
fn v1_get(state: &AuthState, token: &str, client_id: &str, url: &str) -> reqwest::RequestBuilder {
    state
        .client
        .get(url)
        .bearer_auth(token)
        .header("X-Tidal-Token", client_id)
        .header("User-Agent", TIDAL_ANDROID_UA)
}

pub async fn resolve_stream(
    state: &AuthState,
    track_id: &str,
    quality: &str,
) -> Result<(String, String), String> {
    let token = ensure_access_token(state).await?;
    let country = api_country(state).await;
    let client_id = session_client_id(state).await;
    let url = playbackinfo_url(track_id, quality, &country);
    let resp = v1_get(state, &token, &client_id, &url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // 4005 / 11004 / PREREQUISITE_MISSING = sesi tidak diizinkan stream:
        // akun tanpa langganan HiFi, sesi klien TV tidak valid (login ulang),
        // atau track region-locked. Bukan soal scope `playback`.
        if body.contains("4005") || body.contains("11004") || body.contains("PREREQUISITE_MISSING") {
            return Err(
                "Unduhan tidak tersedia: TIDAL menolak permintaan stream. \n\
                 Kemungkinan penyebab: (1) akun belum punya langganan HiFi/Plus, \n\
                 (2) sesi login lama (login ulang supaya memakai klien TV terbaru), \n\
                 (3) track is region-locked. Pencarian/album/playlist/cover tetap berfungsi."
                    .into(),
            );
        }
        return Err(format!("playbackinfo gagal: {status} - {body}"));
    }
    let info: PlaybackInfo = resp.json().await.map_err(|e| e.to_string())?;
    if info.manifest_mime_type.contains("bts") || info.manifest_mime_type.contains("json") {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&info.manifest)
            .map_err(|e| e.to_string())?;
        let manifest: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        let audio_url = manifest["urls"][0]
            .as_str()
            .or(manifest["fileUrl"].as_str())
            .ok_or("manifest tidak berisi URL audio")?
            .to_string();
        let ext = match manifest["codecs"].as_str().unwrap_or("") {
            c if c.contains("flac") => "flac".to_string(),
            // MQA juga dikirim dalam container FLAC (hanya metadatanya berbeda).
            c if c.contains("mqa") => "flac".to_string(),
            c if c.contains("alac") || c.contains("mha1") => "m4a".to_string(),
            c if c.contains("mp4a") || c.contains("aac") => "m4a".to_string(),
            c if c.contains("mp3") => "mp3".to_string(),
            _ => "bin".to_string(),
        };
        Ok((audio_url, ext))
    } else if info.manifest_mime_type.contains("dash")
        || info.manifest_mime_type.contains("mpeg")
    {
        Err("stream DASH terenkripsi tidak didukung - coba kualitas lebih rendah".into())
    } else {
        Err(format!(
            "manifest tidak dikenal: {}",
            info.manifest_mime_type
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_tracks_extracts_metadata() {
        let payload = json!({
            "data": [{
                "id": "sr-1",
                "type": "searchResults",
                "relationships": {
                    "tracks": { "data": [ { "id": "t-1", "type": "tracks" } ] }
                }
            }],
            "included": [
                {
                    "id": "t-1",
                    "type": "tracks",
                    "attributes": { "title": "Test Song", "duration": 213.5 },
                    "relationships": {
                        "artists": { "data": [ { "id": "a-1", "type": "artists" } ] },
                        "albums": { "data": [ { "id": "al-1", "type": "albums" } ] }
                    }
                },
                { "id": "a-1", "type": "artists", "attributes": { "name": "Test Artist" } },
                {
                    "id": "al-1",
                    "type": "albums",
                    "attributes": {
                        "title": "Test Album",
                        "imageLinks": [
                            { "href": "https://resources.tidal.com/small.jpg", "meta": { "width": 160 } },
                            { "href": "https://resources.tidal.com/big.jpg", "meta": { "width": 1280 } }
                        ]
                    }
                }
            ]
        });

        let tracks = parse_tracks(&payload);
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert_eq!(t.id, "t-1");
        assert_eq!(t.title, "Test Song");
        assert_eq!(t.artists, "Test Artist");
        assert_eq!(t.album, "Test Album");
        assert_eq!(t.album_id.as_deref(), Some("al-1"));
        assert_eq!(
            t.cover_url.as_deref(),
            Some("https://resources.tidal.com/big.jpg")
        );
        assert_eq!(t.duration, Some(213.5));
        assert_eq!(t.kind, "song");
    }

    #[test]
    fn parse_tracks_empty_is_safe() {
        let tracks = parse_tracks(&json!({}));
        assert!(tracks.is_empty());
    }

    #[test]
    fn parse_tidal_url_detects_targets() {
        assert!(matches!(
            parse_tidal_url("https://tidal.com/album/519355016"),
            Some(TidalTarget::Album(id)) if id == "519355016"
        ));
        assert!(matches!(
            parse_tidal_url("https://listen.tidal.com/track/12345?u=x"),
            Some(TidalTarget::Track(id)) if id == "12345"
        ));
        assert!(matches!(
            parse_tidal_url("https://tidal.com/playlist/e0f0f0f0-1111-2222-3333-444455556666"),
            Some(TidalTarget::Playlist(_))
        ));
        assert!(matches!(
            parse_tidal_url("https://tidal.com/artist/99887766"),
            Some(TidalTarget::Artist(_))
        ));
        // bukan link tidal → pencarian teks biasa
        assert!(parse_tidal_url("bohemian rhapsody").is_none());
        // link non-tidal yang tidak didukung → error di resolve_query (bukan search)
        assert!(parse_tidal_url("https://youtube.com/watch?v=abc").is_none());
    }

    #[test]
    fn parse_tracks_handles_relationships_array() {
        // bentuk respons /albums/{id}/relationships/items: data = array track resource
        let payload = json!({
            "data": [
                {
                    "id": "t-1",
                    "type": "tracks",
                    "attributes": { "title": "Satu", "duration": 100.0 },
                    "relationships": {
                        "artists": { "data": [ { "id": "a-1", "type": "artists" } ] },
                        "albums": { "data": [ { "id": "al-1", "type": "albums" } ] }
                    }
                },
                {
                    "id": "t-2",
                    "type": "tracks",
                    "attributes": { "title": "Dua" },
                    "relationships": { "artists": { "data": [] }, "albums": { "data": [] } }
                }
            ],
            "included": [
                { "id": "a-1", "type": "artists", "attributes": { "name": "Artis" } },
                {
                    "id": "al-1",
                    "type": "albums",
                    "attributes": { "title": "Album" }
                }
            ]
        });
        let tracks = parse_tracks(&payload);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Satu");
        assert_eq!(tracks[0].artists, "Artis");
        assert_eq!(tracks[0].album, "Album");
        assert_eq!(tracks[1].title, "Dua");
    }

    #[test]
    fn parse_tracks_handles_single_track_object() {
        // bentuk respons /tracks/{id}: data = object track tunggal
        let payload = json!({
            "data": {
                "id": "t-9",
                "type": "tracks",
                "attributes": { "title": "Solo" },
                "relationships": {
                    "artists": { "data": [ { "id": "a-1", "type": "artists" } ] },
                    "albums": { "data": [ { "id": "al-1", "type": "albums" } ] }
                }
            },
            "included": [
                { "id": "a-1", "type": "artists", "attributes": { "name": "Satu Artis" } },
                { "id": "al-1", "type": "albums", "attributes": { "title": "Album Solo" } }
            ]
        });
        let tracks = parse_tracks(&payload);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "t-9");
        assert_eq!(tracks[0].artists, "Satu Artis");
        assert_eq!(tracks[0].album, "Album Solo");
    }

    #[test]
    fn parse_tracks_labels_song_album_and_artist() {
        // Dokumen searchResults memuat tracks + albums + artists sekaligus.
        let payload = json!({
            "data": [{
                "id": "sr-1",
                "type": "searchResults",
                "relationships": {
                    "tracks": { "data": [ { "id": "t-1", "type": "tracks" } ] },
                    "albums": { "data": [ { "id": "al-1", "type": "albums" } ] },
                    "artists": { "data": [ { "id": "a-1", "type": "artists" } ] }
                }
            }],
            "included": [
                {
                    "id": "t-1",
                    "type": "tracks",
                    "attributes": { "title": "Lagu Satu" },
                    "relationships": {
                        "artists": { "data": [ { "id": "a-1", "type": "artists" } ] },
                        "albums": { "data": [ { "id": "al-1", "type": "albums" } ] }
                    }
                },
                {
                    "id": "al-1",
                    "type": "albums",
                    "attributes": { "title": "Album Satu" },
                    "relationships": {
                        "artists": { "data": [ { "id": "a-1", "type": "artists" } ] },
                        "coverArt": { "data": [ { "id": "art-1", "type": "artworks" } ] }
                    }
                },
                {
                    "id": "a-1",
                    "type": "artists",
                    "attributes": { "name": "Artis Satu" },
                    // profileArt = relationship tunggal (objek, bukan array)
                    "relationships": { "profileArt": { "data": { "id": "art-2", "type": "artworks" } } }
                },
                {
                    "id": "art-1",
                    "type": "artworks",
                    "attributes": { "files": [
                        { "href": "https://c/album-320.jpg", "meta": { "width": 320 } },
                        { "href": "https://c/album-1280.jpg", "meta": { "width": 1280 } }
                    ] }
                },
                {
                    "id": "art-2",
                    "type": "artworks",
                    "attributes": { "files": [
                        { "href": "https://c/artist-750.jpg", "meta": { "width": 750 } }
                    ] }
                }
            ]
        });

        let out = parse_tracks(&payload);
        assert_eq!(out.len(), 3);

        let song = out.iter().find(|t| t.kind == "song").expect("ada lagu");
        assert_eq!(song.title, "Lagu Satu");
        assert_eq!(song.artists, "Artis Satu");
        assert_eq!(song.album, "Album Satu");
        assert_eq!(song.album_id.as_deref(), Some("al-1"));

        let album = out.iter().find(|t| t.kind == "album").expect("ada album");
        assert_eq!(album.id, "al-1");
        assert_eq!(album.title, "Album Satu");
        assert_eq!(album.artists, "Artis Satu");
        assert_eq!(album.cover_url.as_deref(), Some("https://c/album-1280.jpg"));

        let artist = out
            .iter()
            .find(|t| t.kind == "artist")
            .expect("ada artis");
        assert_eq!(artist.id, "a-1");
        assert_eq!(artist.title, "Artis Satu");
        assert_eq!(artist.cover_url.as_deref(), Some("https://c/artist-750.jpg"));
    }

    #[test]
    fn playbackinfo_url_uses_v4_and_tv_params() {
        // Bentuk URL disamakan dengan OrpheusDL (`get_stream_url`): suffix /v4,
        // prefetch=false, dan countryCode wajib.
        let url = playbackinfo_url("12345", "LOSSLESS", "ID");
        assert_eq!(
            url,
            "https://api.tidal.com/v1/tracks/12345/playbackinfopostpaywall/v4?\
             playbackmode=STREAM&assetpresentation=FULL&audioquality=LOSSLESS&\
             prefetch=false&countryCode=ID"
        );
    }

    #[test]
    fn parses_playback_info_camel_case() {
        // Respons asli memakai camelCase: `manifestMimeType`. Tanpa
        // rename_all, field-nya kosong → muncul error "manifest tidak dikenal".
        let info: PlaybackInfo = serde_json::from_str(
            r#"{"manifestMimeType":"application/vnd.tidal.bts","manifest":"eyJ1cmxzIjpbXX0="}"#,
        )
        .unwrap();
        assert_eq!(info.manifest_mime_type, "application/vnd.tidal.bts");
        assert_eq!(info.manifest, "eyJ1cmxzIjpbXX0=");
    }

    #[test]
    fn parse_duration_handles_iso8601_and_seconds() {
        // API v2 mengirim durasi sebagai string ISO 8601 (probe: "PT4M16S", "PT46M25S").
        assert_eq!(parse_duration(&json!("PT4M16S")), Some(256.0));
        assert_eq!(parse_duration(&json!("PT46M25S")), Some(2785.0));
        assert_eq!(parse_duration(&json!("PT5M58S")), Some(358.0));
        assert_eq!(parse_duration(&json!("PT1H2M3S")), Some(3723.0));
        assert_eq!(parse_duration(&json!("P1DT1H")), Some(90_000.0));
        // bentuk lama: angka detik
        assert_eq!(parse_duration(&json!(213.5)), Some(213.5));
        // nilai tak dikenal → None (tanpa panic)
        assert_eq!(parse_duration(&json!(null)), None);
        assert_eq!(parse_duration(&json!("bukan durasi")), None);
        assert_eq!(parse_duration(&json!(true)), None);
    }
}


