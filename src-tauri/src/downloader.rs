use std::path::{Path, PathBuf};

use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::auth::AuthState;
use crate::tidal::{resolve_stream, TrackInfo};

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub track_id: String,
    pub title: String,
    pub status: String, // started | progress | done | error
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
#[allow(clippy::too_many_arguments)]
pub async fn download_track(
    app: AppHandle,
    state: &AuthState,
    track: &TrackInfo,
    quality: &str,
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

    let file_name = sanitize(&format!("{} - {}", track.artists, track.title)) + "." + &ext;
    let path = dir.join(&file_name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&path, &audio).map_err(|e| e.to_string())?;

    let cover = match track.cover_url.as_deref() {
        Some(url) => fetch_cover(state, url).await,
        None => None,
    };
    if let Err(e) = embed_tags(&path, &track.title, &track.artists, &track.album, cover.as_deref()) {
        // not fatal: file is already saved
        start("done", Some(format!("tag gagal: {e}")), Some(path.display().to_string()), downloaded, downloaded);
        return Ok(path);
    }

    start(
        "done",
        None,
        Some(path.display().to_string()),
        downloaded,
        downloaded,
    );
    Ok(path)
}
