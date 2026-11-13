use crate::db;
use anyhow::{bail, Result};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::prelude::*;
use lofty::probe::Probe;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Serialize, Clone, Default, Debug)]
pub struct ScanState {
    pub running: bool,
    pub files_seen: u64,
    pub indexed: u64,
    pub unchanged: u64,
    pub tag_errors: u64,
    pub skipped_non_utf8: u64,
    pub walk_errors: u64,
    pub removed: u64,
    pub covers_resolved: u64,
    pub started_at: u64,
    pub finished_at: u64,
    pub duration_s: f64,
    pub error: Option<String>,
}

pub type SharedScan = Arc<Mutex<ScanState>>;

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Start a scan on a blocking thread. Returns false if one is already running.
pub fn spawn(root: PathBuf, db_path: PathBuf, state: SharedScan) -> bool {
    {
        let mut s = state.lock().unwrap();
        if s.running {
            return false;
        }
        *s = ScanState { running: true, started_at: unix_now(), ..Default::default() };
    }
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let res = run(&root, &db_path, &state);
        let mut s = state.lock().unwrap();
        s.running = false;
        s.finished_at = unix_now();
        s.duration_s = t0.elapsed().as_secs_f64();
        match res {
            Ok(()) => info!("scan done in {:.1}s: {:?}", s.duration_s, *s),
            Err(e) => {
                warn!("scan failed: {e:#}");
                s.error = Some(format!("{e:#}"));
            }
        }
    });
    true
}

struct Parsed {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    year: Option<u32>,
    duration_ms: Option<u64>,
    /// 1: `duration_ms` was read from the file's Xing/Info/VBRI header (exact); 2: no such header (an estimate).
    duration_exact: i64,
    bitrate: Option<u32>,
}

/// The exact length is in the Xing / Info / VBRI header when the file has one; lofty only estimates it from the
/// bitrate (and reads 32 kbps from the header frame itself, which gave 36 minutes for a 4 minute song).
fn apply_header_duration(path: &Path, p: &mut Parsed) {
    match crate::mp3seek::header_frame(path) {
        Ok(Some(h)) => match h.duration_ms {
            Some(ms) => {
                p.duration_ms = Some(ms);
                p.duration_exact = 1;
            }
            None => p.duration_exact = 2,
        },
        Ok(None) | Err(_) => p.duration_exact = 2,
    }
}

/// Existing rows come from before the header was read: fix them once, in the background (a few seconds of reading
/// per thousand files on a slow disk). Rows are marked 1 or 2 so the next start has nothing left to do.
pub fn repair_durations(root: PathBuf, db_path: PathBuf) {
    std::thread::spawn(move || {
        let Ok(conn) = db::connect(&db_path) else { return };
        let todo: Vec<(i64, String)> = match conn
            .prepare("SELECT id, path FROM tracks WHERE duration_exact=0")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).and_then(|rows| rows.collect()))
        {
            Ok(v) => v,
            Err(e) => {
                warn!("duration repair: {e}");
                return;
            }
        };
        if todo.is_empty() {
            return;
        }
        info!("duration repair: {} tracks to check", todo.len());
        let t0 = std::time::Instant::now();
        let (mut exact, mut none) = (0u32, 0u32);
        let _ = conn.execute_batch("BEGIN");
        for (n, (id, rel)) in todo.iter().enumerate() {
            let res = match crate::mp3seek::header_frame(&root.join(rel)) {
                Ok(Some(crate::mp3seek::HeaderFrame { duration_ms: Some(ms), .. })) => {
                    exact += 1;
                    conn.execute("UPDATE tracks SET duration_ms=?1, duration_exact=1 WHERE id=?2", params![ms as i64, id])
                }
                Ok(_) => {
                    none += 1;
                    conn.execute("UPDATE tracks SET duration_exact=2 WHERE id=?1", params![id])
                }
                Err(_) => continue, // unreadable now (disk away?): left for the next start
            };
            if let Err(e) = res {
                warn!("duration repair: {e}");
                break;
            }
            if n % 1000 == 999 {
                let _ = conn.execute_batch("COMMIT; BEGIN");
            }
        }
        let _ = conn.execute_batch("COMMIT");
        info!("duration repair done in {:.0}s: {exact} exact lengths read from headers, {none} without header", t0.elapsed().as_secs_f64());
    });
}

fn clean(s: Option<impl AsRef<str>>) -> Option<String> {
    s.map(|v| v.as_ref().trim().to_string()).filter(|v| !v.is_empty())
}

fn parse_tags(path: &Path, fallback_title: &str) -> (Parsed, bool) {
    let opts = ParseOptions::new().parsing_mode(ParsingMode::Relaxed).read_cover_art(false);
    let tagged = Probe::open(path).and_then(|p| p.options(opts).read());
    match tagged {
        Ok(tf) => {
            let props = tf.properties();
            let tag = tf.primary_tag().or_else(|| tf.first_tag());
            let (title, artist, album, genre, track_no, disc_no, year) = match tag {
                Some(t) => (
                    clean(t.title()),
                    clean(t.artist()),
                    clean(t.album()),
                    clean(t.genre()),
                    t.track(),
                    t.disk(),
                    t.year(),
                ),
                None => (None, None, None, None, None, None, None),
            };
            (
                Parsed {
                    title: title.unwrap_or_else(|| fallback_title.to_string()),
                    artist,
                    album,
                    genre,
                    track_no,
                    disc_no,
                    year,
                    duration_ms: Some(props.duration().as_millis() as u64),
                    duration_exact: 0,
                    bitrate: props.audio_bitrate(),
                },
                false,
            )
        }
        Err(e) => {
            warn!("tags unreadable ({e}): {}", path.display());
            // Keep the file playable: retry for audio properties only.
            let props_only = ParseOptions::new().read_tags(false).read_cover_art(false);
            let (duration_ms, bitrate) = match Probe::open(path).and_then(|p| p.options(props_only).read()) {
                Ok(tf) => (Some(tf.properties().duration().as_millis() as u64), tf.properties().audio_bitrate()),
                Err(_) => (None, None),
            };
            (
                Parsed {
                    title: fallback_title.to_string(),
                    artist: None,
                    album: None,
                    genre: None,
                    track_no: None,
                    disc_no: None,
                    year: None,
                    duration_ms,
                    duration_exact: 0,
                    bitrate,
                },
                true,
            )
        }
    }
}

fn bump(state: &SharedScan, f: impl FnOnce(&mut ScanState)) {
    f(&mut state.lock().unwrap());
}

pub fn run(root: &Path, db_path: &Path, state: &SharedScan) -> Result<()> {
    if !root.is_dir() {
        bail!("music root {} is not a directory (disk not mounted?)", root.display());
    }
    let conn = db::open(db_path)?;

    // Known files: path -> (id, size, mtime)
    let mut existing: HashMap<String, (i64, i64, i64, bool)> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT path, id, size, mtime, duration_ms IS NULL FROM tracks")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))))?;
        for r in rows {
            let (p, v) = r?;
            existing.insert(p, v);
        }
    }
    let mut albums: HashMap<String, i64> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT dir, id FROM albums")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for r in rows {
            let (d, i) = r?;
            albums.insert(d, i);
        }
    }
    info!("scan start: {} known tracks, {} known albums", existing.len(), albums.len());

    let mut seen: HashSet<i64> = HashSet::with_capacity(existing.len());
    let mut pending = 0u32;
    conn.execute_batch("BEGIN")?;

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("walk error: {e}");
                bump(state, |s| s.walk_errors += 1);
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let is_mp3 = entry.path().extension().map(|e| e.eq_ignore_ascii_case("mp3")).unwrap_or(false);
        if !is_mp3 {
            continue;
        }
        let rel = match entry.path().strip_prefix(root).ok().and_then(|p| p.to_str()) {
            Some(r) => r.to_string(),
            None => {
                warn!("non UTF-8 path skipped: {:?}", entry.path());
                bump(state, |s| s.skipped_non_utf8 += 1);
                continue;
            }
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                warn!("stat error {rel}: {e}");
                bump(state, |s| s.walk_errors += 1);
                continue;
            }
        };
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        bump(state, |s| s.files_seen += 1);

        if let Some(&(id, sz, mt, unreadable)) = existing.get(&rel) {
            if sz == size && mt == mtime && !unreadable {
                seen.insert(id);
                bump(state, |s| s.unchanged += 1);
                continue;
            }
        }

        let stem = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let (mut p, tag_err) = parse_tags(entry.path(), &stem);
        apply_header_duration(entry.path(), &mut p);
        let dir = Path::new(&rel).parent().and_then(|d| d.to_str()).unwrap_or("").to_string();
        let album_id = match albums.get(&dir) {
            Some(&id) => id,
            None => {
                let dir_name = Path::new(&dir).file_name().and_then(|n| n.to_str()).unwrap_or("(racine)").to_string();
                conn.execute(
                    "INSERT INTO albums(dir, dir_name, name) VALUES(?1, ?2, ?2) ON CONFLICT(dir) DO NOTHING",
                    params![dir, dir_name],
                )?;
                let id: i64 = conn.query_row("SELECT id FROM albums WHERE dir=?1", params![dir], |r| r.get(0))?;
                albums.insert(dir.clone(), id);
                id
            }
        };
        let id: i64 = conn.prepare_cached(
            "INSERT INTO tracks(path, album_id, title, artist, album_tag, genre, track_no, disc_no, year, duration_ms, bitrate_kbps, size, mtime, duration_exact)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(path) DO UPDATE SET album_id=excluded.album_id, title=excluded.title, artist=excluded.artist,
               album_tag=excluded.album_tag, genre=excluded.genre, track_no=excluded.track_no, disc_no=excluded.disc_no,
               year=excluded.year, duration_ms=excluded.duration_ms, duration_exact=excluded.duration_exact, bitrate_kbps=excluded.bitrate_kbps,
               size=excluded.size, mtime=excluded.mtime
             RETURNING id",
        )?
        .query_row(
            params![rel, album_id, p.title, p.artist, p.album, p.genre, p.track_no, p.disc_no, p.year, p.duration_ms, p.bitrate, size, mtime, p.duration_exact],
            |r| r.get(0),
        )?;
        seen.insert(id);
        bump(state, |s| {
            s.indexed += 1;
            if tag_err {
                s.tag_errors += 1;
            }
        });

        pending += 1;
        if pending >= 1000 {
            conn.execute_batch("COMMIT; BEGIN")?;
            pending = 0;
            let s = state.lock().unwrap().clone();
            info!("progress: seen={} indexed={} unchanged={}", s.files_seen, s.indexed, s.unchanged);
        }
    }
    conn.execute_batch("COMMIT")?;

    // Removal pass, guarded: an unmounted/empty disk must never wipe the index.
    let files_seen = state.lock().unwrap().files_seen;
    if files_seen == 0 && !existing.is_empty() {
        bail!("walk found no MP3 but index has {} tracks: refusing to delete (disk problem?)", existing.len());
    }
    remove_missing(&conn, &existing, &seen, state)?;
    refresh_albums(&conn)?;
    // Derived data: failures are logged, never fatal.
    let changed = {
        let st = state.lock().unwrap();
        st.indexed > 0 || st.removed > 0
    };
    if let Err(e) = crate::search::rebuild_if_needed(&conn, changed) {
        warn!("search index rebuild failed: {e:#}");
    }
    // A cover problem must never fail the scan: the library is usable without pictures.
    match crate::covers::refresh(&conn, root) {
        Ok(n) => bump(state, |s| s.covers_resolved = n),
        Err(e) => warn!("cover resolution failed: {e:#}"),
    }
    Ok(())
}

fn remove_missing(
    conn: &Connection,
    existing: &HashMap<String, (i64, i64, i64, bool)>,
    seen: &HashSet<i64>,
    state: &SharedScan,
) -> Result<()> {
    let gone: Vec<i64> = existing.values().map(|v| v.0).filter(|id| !seen.contains(id)).collect();
    conn.execute_batch("BEGIN")?;
    {
        let mut del = conn.prepare("DELETE FROM tracks WHERE id=?1")?;
        for id in &gone {
            del.execute(params![id])?;
        }
    }
    conn.execute_batch("DELETE FROM albums WHERE id NOT IN (SELECT DISTINCT album_id FROM tracks); DELETE FROM album_covers WHERE album_id NOT IN (SELECT id FROM albums); COMMIT")?;
    bump(state, |s| s.removed = gone.len() as u64);
    Ok(())
}

/// Recompute album display fields. Albums are keyed by directory; tags are only used for display.
fn refresh_albums(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE albums SET
           track_count = (SELECT COUNT(*) FROM tracks t WHERE t.album_id = albums.id),
           artist = COALESCE(
                     (SELECT artist FROM tracks t WHERE t.album_id = albums.id AND artist IS NOT NULL
                      GROUP BY artist ORDER BY COUNT(*) DESC, artist LIMIT 1),
                     CASE WHEN instr(dir, '/') > 0 THEN substr(dir, 1, instr(dir, '/') - 1) END),
           artist_count = (SELECT COUNT(DISTINCT artist) FROM tracks t WHERE t.album_id = albums.id AND artist IS NOT NULL),
           year = (SELECT MIN(year) FROM tracks t WHERE t.album_id = albums.id AND year > 0),
           added_at = COALESCE((SELECT MAX(mtime) FROM tracks t WHERE t.album_id = albums.id), 0),
           name = COALESCE((SELECT album_tag FROM tracks t WHERE t.album_id = albums.id AND album_tag IS NOT NULL
                            GROUP BY album_tag ORDER BY COUNT(*) DESC, album_tag LIMIT 1), dir_name);",
    )?;
    Ok(())
}
