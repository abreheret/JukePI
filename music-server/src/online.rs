//! Optional online cover lookup for albums that have no local picture: MusicBrainz (find the release)
//! then Cover Art Archive (download the front cover).
//!
//! PRIVACY: this sends artist and album names from the library to musicbrainz.org and
//! coverartarchive.org (which redirects to archive.org). It is OFF unless `COVERS_ONLINE=1`.
//! Etiquette: at most one MusicBrainz request per ~1.1 s, a descriptive User-Agent, each album tried once
//! every 30 days. Only strict matches are accepted (score >= 95 and same normalised title).

use anyhow::{anyhow, Result};
use rusqlite::params;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::db;

const RETRY_AFTER_SECS: i64 = 30 * 24 * 3600;
const BATCH: i64 = 200;
const MB_PACING: Duration = Duration::from_millis(1100);
const CANDIDATES: usize = 3;
const MIN_SCORE: i64 = 95;

pub struct OnlineConfig {
    pub db_path: PathBuf,
    pub cache_dir: PathBuf, // covers cache root; images go to <cache_dir>/online/
    pub contact: Option<String>,
    pub first_delay: Duration,
    pub interval: Duration,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Lowercase alphanumerics only, bracketed parts removed: "OK Computer (Deluxe)" == "ok computer".
pub fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            c if depth == 0 && c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => {}
        }
    }
    out
}

fn quote(s: &str) -> String {
    s.replace(['"', '\\'], " ")
}

/// Run forever: wait, process the pending albums, sleep, repeat.
pub async fn run_loop(cfg: OnlineConfig) {
    let ua = match &cfg.contact {
        Some(c) => format!("music-server/{} (self-hosted; {c})", env!("CARGO_PKG_VERSION")),
        None => format!("music-server/{} (self-hosted)", env!("CARGO_PKG_VERSION")),
    };
    let client = match reqwest::Client::builder().user_agent(ua).timeout(Duration::from_secs(20)).build() {
        Ok(c) => c,
        Err(e) => {
            warn!("online covers disabled: {e}");
            return;
        }
    };
    info!("online covers ENABLED: artist/album names are sent to musicbrainz.org and coverartarchive.org");
    sleep(cfg.first_delay).await;
    loop {
        match process_pending(&client, &cfg).await {
            Ok((tried, found)) => {
                if tried > 0 {
                    info!("online covers: {found} found out of {tried} album(s) tried")
                }
            }
            Err(e) => warn!("online covers run failed: {e:#}"),
        }
        sleep(cfg.interval).await;
    }
}

struct Pending {
    id: i64,
    artist: String,
    album: String,
}

fn load_pending(db_path: &std::path::Path) -> Result<Vec<Pending>> {
    let conn = db::connect(db_path)?;
    let mut st = conn.prepare(
        "SELECT a.id, a.artist, a.name FROM albums a JOIN album_covers c ON c.album_id = a.id
         WHERE c.kind = 'none' AND c.online_checked_at < ?1 AND a.artist IS NOT NULL AND a.artist_count <= 3
         ORDER BY a.id LIMIT ?2",
    )?;
    let v = st
        .query_map(params![now() - RETRY_AFTER_SECS, BATCH], |r| Ok(Pending { id: r.get(0)?, artist: r.get(1)?, album: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(v)
}

async fn process_pending(client: &reqwest::Client, cfg: &OnlineConfig) -> Result<(u32, u32)> {
    let (mut tried, mut found) = (0, 0);
    loop {
        let db_path = cfg.db_path.clone();
        let batch = tokio::task::spawn_blocking(move || load_pending(&db_path)).await??;
        if batch.is_empty() {
            return Ok((tried, found));
        }
        for p in batch {
            tried += 1;
            let result = lookup(client, &p).await;
            let ok = match result {
                Ok(Some(bytes)) => save(cfg, p.id, bytes).await.map(|_| true).unwrap_or_else(|e| {
                    warn!("online cover for album {} not saved: {e:#}", p.id);
                    false
                }),
                Ok(None) => false,
                Err(e) => {
                    // Network trouble: stop this run, do not mark the albums as tried.
                    return Err(anyhow!("lookup failed (will retry later): {e:#}"));
                }
            };
            if ok {
                found += 1;
            }
            let (db_path, id) = (cfg.db_path.clone(), p.id);
            tokio::task::spawn_blocking(move || -> Result<()> {
                db::connect(&db_path)?.execute("UPDATE album_covers SET online_checked_at=?2 WHERE album_id=?1", params![id, now()])?;
                Ok(())
            })
            .await??;
        }
    }
}

/// Image bytes of the best strict match, or None.
async fn lookup(client: &reqwest::Client, p: &Pending) -> Result<Option<Vec<u8>>> {
    let query = format!("release:\"{}\" AND artist:\"{}\"", quote(&p.album), quote(&p.artist));
    let resp = client
        .get("https://musicbrainz.org/ws/2/release/")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "8")])
        .send()
        .await?;
    sleep(MB_PACING).await;
    if !resp.status().is_success() {
        return Err(anyhow!("MusicBrainz answered {}", resp.status()));
    }
    let json: Value = resp.json().await?;
    let want = normalize(&p.album);
    let matches: Vec<&Value> = json["releases"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .filter(|r| r["score"].as_i64().unwrap_or(0) >= MIN_SCORE)
                .filter(|r| r["title"].as_str().map(|t| normalize(t) == want).unwrap_or(false))
                .take(CANDIDATES)
                .collect()
        })
        .unwrap_or_default();
    // The release-group cover is the "reference" artwork shared by all editions (a single
    // release may be an odd regional edition), so try it first, then each release.
    let mut urls: Vec<String> = vec![];
    for r in &matches {
        if let Some(rg) = r["release-group"]["id"].as_str() {
            let u = format!("https://coverartarchive.org/release-group/{rg}/front-500");
            if !urls.contains(&u) {
                urls.push(u);
            }
        }
    }
    for r in &matches {
        if let Some(id) = r["id"].as_str() {
            urls.push(format!("https://coverartarchive.org/release/{id}/front-500"));
        }
    }
    for url in urls {
        let r = client.get(&url).send().await?;
        if r.status().is_success() {
            let bytes = r.bytes().await?.to_vec();
            debug!("online cover: {} - {} <- {url}", p.artist, p.album);
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

async fn save(cfg: &OnlineConfig, album_id: i64, bytes: Vec<u8>) -> Result<()> {
    let (db_path, dir) = (cfg.db_path.clone(), cfg.cache_dir.join("online"));
    tokio::task::spawn_blocking(move || -> Result<()> {
        image::load_from_memory(&bytes).map_err(|e| anyhow!("downloaded image is not valid: {e}"))?;
        std::fs::create_dir_all(&dir)?;
        let name = format!("{album_id}.jpg");
        std::fs::write(dir.join(&name), &bytes)?;
        db::connect(&db_path)?.execute(
            "UPDATE album_covers SET kind='online', source=?2, source_mtime=?3, source_size=?4 WHERE album_id=?1 AND kind='none'",
            params![album_id, name, now(), bytes.len() as i64],
        )?;
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_titles() {
        assert_eq!(normalize("OK Computer (Deluxe Edition)"), "okcomputer");
        assert_eq!(normalize("Ok  Computer"), "okcomputer");
        assert_eq!(normalize("Albüm É [2016]"), "albümé");
        assert_ne!(normalize("OK Computer"), normalize("OK Computer OKNOTOK"));
    }

    #[test]
    fn quotes_are_neutralised() {
        assert_eq!(quote(r#"a"b\c"#), "a b c");
    }
}
