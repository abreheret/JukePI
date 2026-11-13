//! Album covers: find a source per album at scan time, make thumbnails on demand.
//!
//! Source priority: image file in the album folder > picture embedded in a track > nothing
//! (> optional online lookup, see `online.rs`). Thumbnails (64/150/300/600 px, JPEG) are generated
//! lazily, cached on disk, and keyed by the source's identity so they refresh when the image changes.

use anyhow::{anyhow, bail, Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, Limits};
use lofty::config::ParseOptions;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

pub const SIZES: [u32; 4] = [64, 150, 300, 600];
pub const DEFAULT_SIZE: u32 = 300;
const MAX_SOURCE_BYTES: u64 = 30 * 1024 * 1024;
const MAX_DIMENSION: u32 = 12_000;
const JPEG_QUALITY: u8 = 82;
const EMBEDDED_TRIES: usize = 3;

#[derive(Clone, Debug)]
pub struct CoverRow {
    pub kind: String,
    pub source: Option<String>,
    pub source_mtime: i64,
    pub source_size: i64,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct CoverStats {
    pub albums: u64,
    pub file: u64,
    pub embedded: u64,
    pub online: u64,
    pub none: u64,
    /// Albums (re)resolved by the last scan.
    pub resolved_last_scan: u64,
}

// ---------- choosing an image file in a folder ----------

const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
const BAD_TOKENS: [&str; 11] = ["back", "rear", "inlay", "inside", "tray", "cd", "disc", "booklet", "label", "insert", "spine"];
const SMALL_MARKERS: [&str; 4] = ["small", "thumb", "mini", "tiny"];

fn is_image(name: &str) -> bool {
    !name.starts_with('.')
        && Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
            .unwrap_or(false)
}

/// Higher is better. Understands Windows Media Player leftovers (`AlbumArt_{GUID}_Large.jpg`,
/// `AlbumArtSmall.jpg`, `Folder.jpg`) as well as the usual `cover`/`front`/`folder` names.
fn score(name: &str, size: u64) -> i64 {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let tokens: Vec<&str> = stem.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let mut s = match stem.as_str() {
        "cover" => 100,
        "folder" => 95,
        "front" => 92,
        "albumart" => 90,
        "album" => 85,
        _ if tokens.contains(&"cover") => 80,
        _ if tokens.contains(&"front") => 78,
        _ if tokens.contains(&"folder") => 75,
        _ if tokens.contains(&"albumart") => 70,
        _ => 20,
    };
    if tokens.contains(&"large") {
        s += 5;
    }
    if SMALL_MARKERS.iter().any(|m| stem.contains(m)) {
        s -= 60;
    }
    if tokens.iter().any(|t| BAD_TOKENS.contains(t)) {
        s -= 50;
    }
    if size < 4 * 1024 {
        s -= 30;
    }
    s + (size / (100 * 1024)).min(10) as i64
}

/// Index of the best cover among `(file name, size)` pairs, if any is an image.
pub fn pick_cover(files: &[(String, u64)]) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| is_image(n))
        .max_by_key(|(_, (n, sz))| (score(n, *sz), *sz))
        .map(|(i, _)| i)
}

fn is_disc_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    ["cd", "disc", "disk", "dvd"].iter().any(|p| {
        n.strip_prefix(p)
            .map(|rest| rest.trim_start_matches([' ', '_', '-', '.']).chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
            .unwrap_or(false)
    })
}

// ---------- resolving (scan time) ----------

fn unix(t: std::io::Result<SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn best_image_in(root: &Path, dir: &str) -> Option<CoverRow> {
    let rd = std::fs::read_dir(root.join(dir)).ok()?;
    let mut files: Vec<(String, u64, i64)> = vec![];
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let Ok(md) = e.metadata() else { continue };
        if md.is_file() {
            files.push((name, md.len(), unix(md.modified())));
        }
    }
    let pairs: Vec<(String, u64)> = files.iter().map(|(n, s, _)| (n.clone(), *s)).collect();
    let i = pick_cover(&pairs)?;
    let (name, size, mtime) = &files[i];
    let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
    Some(CoverRow { kind: "file".into(), source: Some(rel), source_mtime: *mtime, source_size: *size as i64 })
}

/// Best embedded picture of a track: front cover first, otherwise the first picture.
pub fn embedded_picture(path: &Path) -> Option<Vec<u8>> {
    let opts = ParseOptions::new().read_cover_art(true);
    let tf = Probe::open(path).ok()?.options(opts).read().ok()?;
    let tag = tf.primary_tag().or_else(|| tf.first_tag())?;
    let pics = tag.pictures();
    pics.iter().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| pics.first()).map(|p| p.data().to_vec())
}

fn resolve_album(root: &Path, dir: &str, track_paths: &[String]) -> CoverRow {
    let none = CoverRow { kind: "none".into(), source: None, source_mtime: 0, source_size: 0 };
    if let Some(r) = best_image_in(root, dir) {
        return r;
    }
    // Multi-disc layout: Artist/Album/CD1/track.mp3 with the cover one level up.
    let base = Path::new(dir).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if is_disc_dir(base) {
        if let Some(parent) = Path::new(dir).parent().and_then(|p| p.to_str()).filter(|p| !p.is_empty()) {
            if let Some(r) = best_image_in(root, parent) {
                return r;
            }
        }
    }
    for rel in track_paths.iter().take(EMBEDDED_TRIES) {
        let full = root.join(rel);
        if embedded_picture(&full).is_some() {
            let md = std::fs::metadata(&full).ok();
            return CoverRow {
                kind: "embedded".into(),
                source: Some(rel.clone()),
                source_mtime: md.as_ref().map(|m| unix(m.modified())).unwrap_or(0),
                source_size: md.map(|m| m.len() as i64).unwrap_or(0),
            };
        }
    }
    none
}

/// Resolve the cover source of every album that is new or changed since last time.
/// Called at the end of a scan; cheap when nothing changed (one `stat` per album folder).
pub fn refresh(conn: &Connection, root: &Path) -> Result<u64> {
    struct Todo {
        id: i64,
        dir: String,
        stamp: i64,
        dir_mtime: i64,
    }
    let mut todo = vec![];
    {
        let mut st = conn.prepare(
            "SELECT a.id, a.dir, a.added_at, c.tracks_stamp, c.dir_mtime, c.kind, c.source
             FROM albums a LEFT JOIN album_covers c ON c.album_id = a.id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        for r in rows {
            let (id, dir, stamp, c_stamp, c_dirm, kind, source) = r?;
            let dir_mtime = unix(std::fs::metadata(root.join(&dir)).and_then(|m| m.modified()));
            let source_gone = matches!(kind.as_deref(), Some("file") | Some("embedded"))
                && source.as_deref().map(|s| !root.join(s).exists()).unwrap_or(true);
            let stale = kind.is_none() || c_stamp != Some(stamp) || c_dirm != Some(dir_mtime) || source_gone;
            // Online covers live in the cache dir: keep them unless the folder changed.
            if stale && !(kind.as_deref() == Some("online") && c_stamp == Some(stamp) && c_dirm == Some(dir_mtime)) {
                todo.push(Todo { id, dir, stamp, dir_mtime });
            }
        }
    }
    if todo.is_empty() {
        return Ok(0);
    }
    info!("covers: resolving {} album(s)", todo.len());
    let now = unix(Ok(SystemTime::now()));
    let mut done = 0u64;
    conn.execute_batch("BEGIN")?;
    for t in &todo {
        let tracks: Vec<String> = {
            let mut st = conn.prepare_cached(
                "SELECT path FROM tracks WHERE album_id=?1 ORDER BY COALESCE(disc_no,1), COALESCE(track_no,9999), path LIMIT 8",
            )?;
            let v = st.query_map(params![t.id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
            v
        };
        let row = resolve_album(root, &t.dir, &tracks);
        debug!("covers: album {} ({}) -> {} {:?}", t.id, t.dir, row.kind, row.source);
        conn.execute(
            "INSERT INTO album_covers(album_id, kind, source, source_mtime, source_size, tracks_stamp, dir_mtime, checked_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(album_id) DO UPDATE SET kind=excluded.kind, source=excluded.source, source_mtime=excluded.source_mtime,
               source_size=excluded.source_size, tracks_stamp=excluded.tracks_stamp, dir_mtime=excluded.dir_mtime, checked_at=excluded.checked_at",
            params![t.id, row.kind, row.source, row.source_mtime, row.source_size, t.stamp, t.dir_mtime, now],
        )?;
        done += 1;
        if done % 500 == 0 {
            conn.execute_batch("COMMIT; BEGIN")?;
            info!("covers: {done}/{} resolved", todo.len());
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(done)
}

pub fn stats(conn: &Connection, resolved_last_scan: u64) -> Result<CoverStats> {
    let mut s = CoverStats { resolved_last_scan, ..Default::default() };
    s.albums = conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get::<_, i64>(0))? as u64;
    let mut st = conn.prepare("SELECT kind, COUNT(*) FROM album_covers GROUP BY kind")?;
    for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))? {
        let (k, n) = r?;
        match k.as_str() {
            "file" => s.file = n,
            "embedded" => s.embedded = n,
            "online" => s.online = n,
            _ => s.none += n,
        }
    }
    // Albums without any row yet count as unresolved "none".
    s.none = s.albums.saturating_sub(s.file + s.embedded + s.online);
    Ok(s)
}

// ---------- thumbnails (request time) ----------

/// Stable (across Rust versions) 64-bit FNV-1a, used for cache keys.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn cache_key(row: &CoverRow, size: u32) -> String {
    let id = format!("{}|{}|{}|{}|{}", row.kind, row.source.as_deref().unwrap_or(""), row.source_mtime, row.source_size, size);
    format!("{:016x}", fnv(&id))
}

fn read_source(root: &Path, cache_dir: &Path, row: &CoverRow) -> Result<Vec<u8>> {
    let source = row.source.as_deref().ok_or_else(|| anyhow!("cover without source"))?;
    match row.kind.as_str() {
        "file" | "online" => {
            let path = if row.kind == "online" { cache_dir.join("online").join(source) } else { root.join(source) };
            let len = std::fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?.len();
            if len > MAX_SOURCE_BYTES {
                bail!("cover image too large ({len} bytes)");
            }
            Ok(std::fs::read(&path)?)
        }
        "embedded" => embedded_picture(&root.join(source)).ok_or_else(|| anyhow!("no embedded picture in {source}")),
        k => bail!("no cover source of kind {k}"),
    }
}

/// Decode, shrink to fit `size`x`size` (aspect ratio kept, never upscaled), re-encode as JPEG.
pub fn make_thumb(src: &[u8], size: u32) -> Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(src)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode()?;
    let img = if img.width() > size || img.height() > size { img.resize(size, size, FilterType::CatmullRom) } else { img };
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8()); // JPEG has no alpha
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&rgb)?;
    Ok(out)
}

pub struct CoverService {
    root: PathBuf,
    cache_dir: PathBuf,
    sem: Semaphore,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl CoverService {
    pub fn new(root: PathBuf, cache_dir: PathBuf) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&cache_dir);
        // Two at a time: decoding big images on a Pi must not starve other services.
        Arc::new(CoverService { root, cache_dir, sem: Semaphore::new(2), locks: Mutex::new(HashMap::new()) })
    }

    #[allow(dead_code)] // used by the online lookup
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// JPEG bytes of the album thumbnail and its cache key (usable as ETag).
    pub async fn thumb(self: &Arc<Self>, album_id: i64, row: &CoverRow, size: u32) -> Result<(Vec<u8>, String)> {
        let key = cache_key(row, size);
        let path = self.cache_dir.join(size.to_string()).join(format!("{album_id}-{key}.jpg"));
        if let Ok(b) = tokio::fs::read(&path).await {
            return Ok((b, key));
        }
        // One generation per thumbnail at a time: a grid asking for the same cover twice waits instead of redoing the work.
        let lock = self.locks.lock().unwrap().entry(path.to_string_lossy().into_owned()).or_default().clone();
        let _guard = lock.lock().await;
        if let Ok(b) = tokio::fs::read(&path).await {
            return Ok((b, key));
        }
        let _permit = self.sem.acquire().await?;
        let (root, cache, row2, path2) = (self.root.clone(), self.cache_dir.clone(), row.clone(), path.clone());
        let res = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let src = read_source(&root, &cache, &row2)?;
            let jpg = make_thumb(&src, size)?;
            if let Some(dir) = path2.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path2.with_extension("tmp");
            std::fs::write(&tmp, &jpg)?;
            std::fs::rename(&tmp, &path2)?;
            Ok(jpg)
        })
        .await?;
        self.locks.lock().unwrap().remove(&path.to_string_lossy().into_owned());
        match res {
            Ok(b) => Ok((b, key)),
            Err(e) => {
                warn!("cover {album_id} ({:?}): {e:#}", row.source);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(files: &[(&str, u64)]) -> Option<String> {
        let v: Vec<(String, u64)> = files.iter().map(|(n, s)| (n.to_string(), *s)).collect();
        pick_cover(&v).map(|i| v[i].0.clone())
    }

    #[test]
    fn prefers_cover_names() {
        assert_eq!(pick(&[("back.jpg", 300_000), ("cover.jpg", 100_000), ("scan01.jpg", 900_000)]).as_deref(), Some("cover.jpg"));
        assert_eq!(pick(&[("Folder.jpg", 50_000), ("front.png", 80_000)]).as_deref(), Some("Folder.jpg"));
    }

    #[test]
    fn windows_media_player_leftovers() {
        let got = pick(&[
            ("AlbumArtSmall.jpg", 6_000),
            ("AlbumArt_{ABC-123}_Small.jpg", 7_000),
            ("AlbumArt_{ABC-123}_Large.jpg", 90_000),
        ]);
        assert_eq!(got.as_deref(), Some("AlbumArt_{ABC-123}_Large.jpg"));
    }

    #[test]
    fn ignores_non_images_hidden_and_booklet() {
        assert_eq!(pick(&[("notes.txt", 10), ("._cover.jpg", 4000), ("a.m3u", 5)]), None);
        // a lone booklet scan still beats nothing, but a plain image beats it
        assert_eq!(pick(&[("booklet.jpg", 500_000), ("img001.jpg", 200_000)]).as_deref(), Some("img001.jpg"));
        assert_eq!(pick(&[("booklet.jpg", 500_000)]).as_deref(), Some("booklet.jpg"));
    }

    #[test]
    fn disc_folder_detection() {
        assert!(is_disc_dir("CD1"));
        assert!(is_disc_dir("Disc 2"));
        assert!(is_disc_dir("disk_3"));
        assert!(!is_disc_dir("Discovery"));
        assert!(!is_disc_dir("CD Single"));
    }

    #[test]
    fn thumbnail_shrinks_but_never_upscales() {
        let img = DynamicImage::new_rgb8(1000, 500);
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let t = make_thumb(&png, 300).unwrap();
        let out = image::load_from_memory(&t).unwrap();
        assert_eq!((out.width(), out.height()), (300, 150));
        let small = DynamicImage::new_rgb8(100, 100);
        let mut png = Vec::new();
        small.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let out = image::load_from_memory(&make_thumb(&png, 300).unwrap()).unwrap();
        assert_eq!((out.width(), out.height()), (100, 100));
    }

    #[test]
    fn cache_key_changes_with_source() {
        let a = CoverRow { kind: "file".into(), source: Some("A/cover.jpg".into()), source_mtime: 1, source_size: 10 };
        let mut b = a.clone();
        b.source_mtime = 2;
        assert_ne!(cache_key(&a, 300), cache_key(&b, 300));
        assert_ne!(cache_key(&a, 300), cache_key(&a, 150));
        assert_eq!(cache_key(&a, 300), cache_key(&a.clone(), 300));
    }
}
