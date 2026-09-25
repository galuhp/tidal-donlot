use std::path::{Path, PathBuf};

use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::auth::AuthState;
use crate::tidal::{resolve_stream, TrackInfo};
use crate::transcode::{self, Mp3Mode};

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub track_id: String,
    pub title: String,
    pub status: String, // started | progress | converting | done | error
    pub downloaded: u64,
    pub total: u64,
    pub message: Option<String>,
    pub path: Option<String>,
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_removes_illegal_windows_chars() {
        assert_eq!(sanitize("AC/DC: Back In Black?"), "AC DC  Back In Black");
        assert_eq!(sanitize("a<b>c\"d|e*f"), "a b c d e f");
    }

    #[test]
    fn sanitize_trims() {
        assert_eq!(sanitize("  spaced  "), "spaced");
    }
}

async fn fetch_cover(state: &AuthState, url: &str) -> Option<Vec<u8>> {
    let resp = state.client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok().map(|b| b.to_vec())
}

fn embed_tags(
    file_path: &Path,
    title: &str,
    artists: &str,
    album: &str,
    cover: Option<&[u8]>,
) -> Result<(), String> {
    let mut tagged = Probe::open(file_path)
        .map_err(|e| e.to_string())?
        .guess_file_type()
        .map_err(|e| e.to_string())?
        .read()
        .map_err(|e| e.to_string())?;

    let mut tag = match tagged.primary_tag_mut() {
        Some(t) => t.clone(),
        None => match tagged.first_tag_mut() {
            Some(t) => t.clone(),
            None => Tag::new(tagged.primary_tag_type()),
        },
    };
    tag.set_title(title.to_string());
    tag.set_artist(artists.to_string());
    tag.set_album(album.to_string());
    if let Some(data) = cover {
        let pic = Picture::new_unchecked(
            PictureType::CoverFront,
            Some(MimeType::Jpeg),
            None,
            data.to_vec(),
        );
        let _ = tag.set_picture(0, pic);
    }
    tagged.insert_tag(tag);
    tagged
        .save_to_path(file_path, lofty::config::WriteOptions::default())
        .map_err(|e| e.to_string())
}

/// Download one track, emit progress events, embed metadata, save to `dir`.
///
/// `format` (dropdown "Format" di UI) menentukan hasil akhirnya:
/// - `original` / nilai lain: file disimpan apa adanya — Lossless/Hi-Res → FLAC,
///   High/Low → AAC di dalam `.m4a`.
/// - `mp3_same`: dikonversi ke MP3 CBR dengan bitrate file sumber.
/// - `mp3_vbr0`: dikonversi ke MP3 VBR V0 (LAME `-V0`).
///
/// Kalau konversi gagal, file hasil unduhan tetap disimpan (tidak dihapus)
/// supaya unduhan tidak sia-sia.
#[allow(clippy::too_many_arguments)]
pub async fn download_track(
    app: AppHandle,
    state: &AuthState,
    track: &TrackInfo,
    quality: &str,
    format: &str,
    dir: PathBuf,
) -> Result<PathBuf, String> {
    let emit = |p: DownloadProgress| {
        let _ = app.emit("download-progress", p);
    };
    let start = |status: &str, msg: Option<String>, path: Option<String>, dl: u64, total: u64| {
        emit(DownloadProgress {
            track_id: track.id.clone(),
            title: track.title.clone(),
            status: status.into(),
            downloaded: dl,
            total,
            message: msg,
            path,
        })
    };

    start("started", None, None, 0, 0);

    let (audio_url, ext) = resolve_stream(state, &track.id, quality).await?;

    let resp = state
        .client
        .get(&audio_url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download audio gagal: {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut audio: Vec<u8> = Vec::with_capacity(total.max(1024 * 1024) as usize);
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        audio.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        start("progress", None, None, downloaded, total);
    }

    let base = sanitize(&format!("{} - {}", track.artists, track.title));
    let path = dir.join(format!("{base}.{ext}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&path, &audio).map_err(|e| e.to_string())?;

    let cover = match track.cover_url.as_deref() {
        Some(url) => fetch_cover(state, url).await,
        None => None,
    };

    // Tanpa konversi: file disimpan apa adanya (FLAC, atau AAC `.m4a` untuk
    // kualitas High/Low).
    let Some(mode) = transcode::parse_mode(format) else {
        return finish(
            &start,
            path,
            track,
            cover.as_deref(),
            downloaded,
            None,
        );
    };

    // Konversi ke MP3 — CPU-heavy, jadi dijalankan di blocking thread supaya
    // runtime async (dan event progress) tidak ikut tertahan.
    let mode_label = match mode {
        Mp3Mode::SameBitrate => "bitrate sama",
        Mp3Mode::Vbr0 => "VBR V0",
    };
    start(
        "converting",
        Some(format!("Mengonversi ke MP3 ({mode_label})…")),
        None,
        downloaded,
        downloaded,
    );

    let mp3_path = dir.join(format!("{base}.mp3"));
    let source_kbps = transcode::probe_bitrate_kbps(&path);
    let (input, output) = (path.clone(), mp3_path.clone());
    let converted = tauri::async_runtime::spawn_blocking(move || {
        transcode::convert_to_mp3(&input, &output, mode, source_kbps)
    })
    .await;

    match converted {
        Ok(Ok(())) => {
            // Konversi sukses → MP3 jadi hasil akhir, file sumber dihapus
            // (kalau jalur file-nya memang berbeda).
            if path != mp3_path {
                let _ = std::fs::remove_file(&path);
            }
            finish(&start, mp3_path, track, cover.as_deref(), downloaded, None)
        }
        Ok(Err(e)) => finish(
            &start,
            path,
            track,
            cover.as_deref(),
            downloaded,
            Some(format!("konversi MP3 gagal — file asli disimpan: {e}")),
        ),
        Err(e) => finish(
            &start,
            path,
            track,
            cover.as_deref(),
            downloaded,
            Some(format!("konversi MP3 gagal — file asli disimpan: {e}")),
        ),
    }
}

/// Tag file hasil & kirim event `done` (dengan `warning` opsional).
#[allow(clippy::too_many_arguments)]
fn finish(
    start: &impl Fn(&str, Option<String>, Option<String>, u64, u64),
    path: PathBuf,
    track: &TrackInfo,
    cover: Option<&[u8]>,
    downloaded: u64,
    warning: Option<String>,
) -> Result<PathBuf, String> {
    let shown = path.display().to_string();
    // Tag gagal bukan error fatal: file sudah tersimpan.
    let message = match embed_tags(&path, &track.title, &track.artists, &track.album, cover) {
        Ok(()) => warning,
        Err(e) => Some(match warning {
            Some(w) => format!("{w}; tag gagal: {e}"),
            None => format!("tag gagal: {e}"),
        }),
    };
    start("done", message, Some(shown), downloaded, downloaded);
    Ok(path)
}
