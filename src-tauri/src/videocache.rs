//! Video playlist upkeep: watched-folder scanning and a disk cache for URL
//! entries, because venue internet is unreliable and provider URLs expire.
//!
//! Every `PlaylistKind::Url` entry is downloaded once into
//! `<config dir>/EmpyreanGate/media-cache/<id>` (with a small `<id>.json` sidecar
//! for the content type). Playback prefers the cached copy via
//! `/media/file/{id}`; until it exists, clients fall back to the live resolver
//! proxy. Downloads reuse the resolver end-to-end, so yt-dlp extraction and the
//! public-IP safety checks apply to cached fetches too.

use crate::config::{PlaylistEntry, PlaylistKind};
use crate::media::MediaResolver;
use crate::state::SharedState;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Extensions browsers can generally decode; folder scans pick up only these.
const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "m4v", "mov", "webm", "ogv"];
const SCAN_DEPTH: usize = 3;

#[derive(Debug, Clone, Serialize, Default)]
pub struct VideoCacheStatus {
    pub id: String,
    /// "cached" | "downloading" | "pending" | "error" | "local"
    pub state: String,
    /// 0..1 while downloading (0 when total size unknown).
    pub progress: f32,
    pub bytes: u64,
    pub error: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    content_type: String,
}

pub fn cache_dir() -> PathBuf {
    crate::config::config_path()
        .parent()
        .map(|p| p.join("media-cache"))
        .unwrap_or_else(|| PathBuf::from("media-cache"))
}

pub fn cached_file(id: &str) -> Option<(PathBuf, String)> {
    // Ids are uuids we generated; refuse anything path-like defensively.
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let path = cache_dir().join(id);
    if !path.is_file() {
        return None;
    }
    let content_type = std::fs::read_to_string(cache_dir().join(format!("{id}.json")))
        .ok()
        .and_then(|s| serde_json::from_str::<CacheMeta>(&s).ok())
        .map(|m| m.content_type)
        .unwrap_or_else(|| "video/mp4".into());
    Some((path, content_type))
}

fn set_status(state: &SharedState, id: &str, status: VideoCacheStatus) {
    state.video_cache.lock().insert(id.to_string(), status);
}

/// Runs forever on the server runtime: reconcile watched folders into the
/// playlist, then download any uncached URL entries (one at a time).
pub async fn run(state: Arc<SharedState>, resolver: Arc<MediaResolver>) {
    let _ = std::fs::create_dir_all(cache_dir());
    let mut failed: HashSet<String> = HashSet::new();
    let mut failure_round = 0u32;

    while !state.shutdown.load(Ordering::Relaxed) {
        reconcile_dirs(&state);
        refresh_statuses(&state);

        // Periodically forgive failures so transient outages retry (~5 min).
        failure_round += 1;
        if failure_round % 30 == 0 {
            failed.clear();
        }

        let next = {
            let cfg = state.config.read();
            cfg.video
                .playlist
                .iter()
                .find(|e| {
                    e.kind == PlaylistKind::Url
                        && cached_file(&e.id).is_none()
                        && !failed.contains(&e.id)
                })
                .cloned()
        };

        if let Some(entry) = next {
            match download(&state, &resolver, &entry).await {
                Ok(()) => {
                    log::info!("cached video '{}' ({})", entry.title, entry.id);
                }
                Err(e) => {
                    log::warn!("caching '{}' failed: {e:#}", entry.title);
                    failed.insert(entry.id.clone());
                    set_status(
                        &state,
                        &entry.id,
                        VideoCacheStatus {
                            id: entry.id.clone(),
                            state: "error".into(),
                            error: format!("{e:#}"),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

/// Make the cache-state map reflect reality for every playlist entry.
fn refresh_statuses(state: &SharedState) {
    let playlist = state.config.read().video.playlist.clone();
    remove_orphaned_cache_files(&playlist);
    let mut map = state.video_cache.lock();
    let ids: HashSet<&str> = playlist.iter().map(|e| e.id.as_str()).collect();
    map.retain(|id, _| ids.contains(id.as_str()));
    for entry in &playlist {
        let current = map.get(&entry.id).map(|s| s.state.clone());
        match entry.kind {
            PlaylistKind::LocalFile => {
                map.insert(
                    entry.id.clone(),
                    VideoCacheStatus {
                        id: entry.id.clone(),
                        state: if Path::new(&entry.source).is_file() {
                            "local".into()
                        } else {
                            "error".into()
                        },
                        ..Default::default()
                    },
                );
            }
            PlaylistKind::Url => {
                if let Some((path, _)) = cached_file(&entry.id) {
                    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    map.insert(
                        entry.id.clone(),
                        VideoCacheStatus {
                            id: entry.id.clone(),
                            state: "cached".into(),
                            progress: 1.0,
                            bytes,
                            ..Default::default()
                        },
                    );
                } else if !matches!(current.as_deref(), Some("downloading") | Some("error")) {
                    map.insert(
                        entry.id.clone(),
                        VideoCacheStatus {
                            id: entry.id.clone(),
                            state: "pending".into(),
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }
}

/// Playlist deletion must reclaim its download. Without this, every removed URL
/// leaked a full video forever and an unattended machine could eventually fill
/// its system disk even though the UI showed an empty playlist.
fn remove_orphaned_cache_files(playlist: &[PlaylistEntry]) {
    remove_orphaned_cache_files_in(&cache_dir(), playlist);
}

fn remove_orphaned_cache_files_in(dir: &Path, playlist: &[PlaylistEntry]) {
    let live: HashSet<&str> = playlist.iter().map(|entry| entry.id.as_str()).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let id = name
            .strip_suffix(".json.part")
            .or_else(|| name.strip_suffix(".json"))
            .or_else(|| name.strip_suffix(".part"))
            .unwrap_or(&name);
        // Both browser-added and folder-discovered entries use UUID-simple IDs.
        // Do not delete arbitrary operator files that happen to live here.
        let looks_managed = uuid::Uuid::parse_str(id).is_ok();
        if looks_managed && !live.contains(id) {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => log::info!("removed orphaned media cache file {name}"),
                Err(e) => log::warn!("could not remove orphaned media cache file {name}: {e}"),
            }
        }
    }
}

async fn download(
    state: &Arc<SharedState>,
    resolver: &MediaResolver,
    entry: &PlaylistEntry,
) -> Result<()> {
    set_status(
        state,
        &entry.id,
        VideoCacheStatus {
            id: entry.id.clone(),
            state: "downloading".into(),
            ..Default::default()
        },
    );

    // Reuse the resolver end-to-end: yt-dlp extraction, public-IP validation,
    // redirect discipline — cached fetches get the same safety story as live ones.
    let resolved = resolver.resolve(&entry.source).await?;
    let session_id = resolved
        .playback_url
        .rsplit('/')
        .next()
        .context("resolver returned no session")?;
    let response = resolver.stream(session_id, None).await?;
    let total = response.content_length().unwrap_or(0);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/mp4")
        .to_string();

    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("{}.part", entry.id));
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut stream = response.bytes_stream();
    let mut bytes: u64 = 0;
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        if state.shutdown.load(Ordering::Relaxed) {
            anyhow::bail!("shutting down");
        }
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        bytes += chunk.len() as u64;
        set_status(
            state,
            &entry.id,
            VideoCacheStatus {
                id: entry.id.clone(),
                state: "downloading".into(),
                progress: if total > 0 { bytes as f32 / total as f32 } else { 0.0 },
                bytes,
                ..Default::default()
            },
        );
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    anyhow::ensure!(bytes > 100_000, "downloaded file is implausibly small ({bytes} bytes)");

    let meta_path = dir.join(format!("{}.json", entry.id));
    let meta_tmp = dir.join(format!("{}.json.part", entry.id));
    let mut meta = tokio::fs::File::create(&meta_tmp).await?;
    meta.write_all(serde_json::to_string(&CacheMeta { content_type })?.as_bytes())
        .await?;
    meta.sync_all().await?;
    drop(meta);
    if meta_path.exists() {
        std::fs::remove_file(&meta_path)?;
    }
    std::fs::rename(&meta_tmp, &meta_path)?;
    std::fs::rename(&tmp, dir.join(&entry.id))?;
    #[cfg(unix)]
    std::fs::File::open(&dir)?.sync_all()?;
    refresh_statuses(state);
    state.broadcast_state();
    Ok(())
}

/// Scan watched folders and reconcile auto-discovered entries into the playlist.
fn reconcile_dirs(state: &Arc<SharedState>) {
    let cfg = state.config.read().video.clone();
    let mut found: Vec<(String, String)> = Vec::new(); // (dir, absolute path)
    for dir in &cfg.dirs {
        collect_videos(Path::new(dir), dir, SCAN_DEPTH, &mut found);
    }

    let existing: HashSet<&str> = cfg
        .playlist
        .iter()
        .filter(|e| e.kind == PlaylistKind::LocalFile)
        .map(|e| e.source.as_str())
        .collect();
    let found_paths: HashSet<&str> = found.iter().map(|(_, p)| p.as_str()).collect();

    let additions: Vec<PlaylistEntry> = found
        .iter()
        .filter(|(_, path)| !existing.contains(path.as_str()))
        .map(|(dir, path)| PlaylistEntry {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title: Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone()),
            source: path.clone(),
            kind: PlaylistKind::LocalFile,
            from_dir: dir.clone(),
        })
        .collect();

    // Auto-discovered entries whose file vanished (or whose folder was removed
    // from the watch list) disappear; manual entries are never touched.
    let stale = |e: &PlaylistEntry| {
        e.kind == PlaylistKind::LocalFile
            && !e.from_dir.is_empty()
            && (!cfg.dirs.contains(&e.from_dir) || !found_paths.contains(e.source.as_str()))
    };
    let has_stale = cfg.playlist.iter().any(stale);

    if !additions.is_empty() || has_stale {
        log::info!(
            "playlist folder scan: +{} entries{}",
            additions.len(),
            if has_stale { ", removing stale" } else { "" }
        );
        state.update_config(|c| {
            c.video.playlist.retain(|e| !stale(e));
            c.video.playlist.extend(additions.iter().cloned());
        });
    }
}

fn collect_videos(path: &Path, origin_dir: &str, depth: usize, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if depth > 0 {
                collect_videos(&p, origin_dir, depth - 1, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push((origin_dir.to_string(), p.to_string_lossy().to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_cleanup_removes_deleted_entries_but_preserves_operator_files() {
        let dir = std::env::temp_dir().join(format!(
            "empyrean-gate-cache-cleanup-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let live = uuid::Uuid::new_v4().simple().to_string();
        let orphan = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(dir.join(&live), b"live").unwrap();
        std::fs::write(dir.join(format!("{orphan}.part")), b"partial").unwrap();
        std::fs::write(dir.join("README"), b"operator note").unwrap();
        let playlist = vec![PlaylistEntry {
            id: live.clone(),
            title: "live".into(),
            source: "https://example.com/live.mp4".into(),
            kind: PlaylistKind::Url,
            from_dir: String::new(),
        }];

        remove_orphaned_cache_files_in(&dir, &playlist);

        assert!(dir.join(live).exists());
        assert!(!dir.join(format!("{orphan}.part")).exists());
        assert!(dir.join("README").exists());
        std::fs::remove_file(dir.join(playlist[0].id.clone())).unwrap();
        std::fs::remove_file(dir.join("README")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
