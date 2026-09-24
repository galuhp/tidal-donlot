use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{ensure_access_token, AuthState};

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
}

fn cover_from_album(
    res: &Value,
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
    // Bentuk OpenAPI v2: album.relationships.coverArt -> resource `artworks`
    // di `included`, URL gambar di attributes.files[].href (pilih yang terlebar).
    let art_id = res["relationships"]["coverArt"]["data"][0]["id"].as_str()?;
    let art = by_id.get(art_id)?;
    let best = art["attributes"]["files"]
        .as_array()?
        .iter()
        .max_by_key(|f| f["meta"]["width"].as_u64().unwrap_or(0))?;
    best["href"].as_str().map(String::from)
}

/// Include untuk relationship items (album/playlist): track + artis + album + cover.
const INCLUDE_ITEMS: &str = "items,items.artists,items.albums,items.albums.coverArt";

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
    let artists = track["relationships"]["artists"]["data"]
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
        .unwrap_or_default();
    let album_id = track["relationships"]["albums"]["data"][0]["id"]
        .as_str()
        .map(|s| s.to_string());
    let (album, cover_url) = album_id
        .as_deref()
        .and_then(|aid| by_id.get(aid))
        .map(|a| {
            (
                a["attributes"]["title"].as_str().unwrap_or("").to_string(),
                cover_from_album(a, by_id),
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
    for it in &data_items {
        if it["type"].as_str() == Some("tracks") {
            refs.push(it);
        } else if let Some(sub) = it["relationships"]["tracks"]["data"].as_array() {
            refs.extend(sub.iter());
        }
    }

    refs.iter()
        .filter_map(|r| {
            let id = r["id"].as_str()?;
            let full = by_id.get(id).copied().unwrap_or(r);
            // link tanpa atribut title tidak bisa dipakai untuk download
            if full["attributes"]["title"].is_null() && r["attributes"]["title"].is_null() {
                return None;
            }
            Some(build_track(full, &by_id))
        })
        .collect()
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
    // Langkah 2: tracks lengkap (artis + album + cover) dari id hasil pencarian.
    let url = format!(
        "{API_BASE}/searchResults/{search_id}?countryCode={COUNTRY}&include=tracks,tracks.artists,tracks.albums,tracks.albums.coverArt"
    );
    let json = get_json(state, &token, &url, "search").await?;
    Ok(parse_tracks(&json))
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
struct PlaybackInfo {
    #[serde(default)]
    manifest_mime_type: String,
    #[serde(default)]
    manifest: String,
}

pub async fn resolve_stream(
    state: &AuthState,
    track_id: &str,
    quality: &str,
) -> Result<(String, String), String> {
    let token = ensure_access_token(state).await?;
    let url = format!(
        "{LEGACY_API}/tracks/{track_id}/playbackinfopostpaywall?audioquality={quality}&playbackmode=STREAM&assetpresentation=FULL"
    );
    let resp = state
        .client
        .post(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "playbackinfo gagal: {status} - {body} (pastikan akun premium & scope playback aktif)"
        ));
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


