//! Full-text search over tracks, albums and artists (SQLite FTS5).
//!
//! The three FTS tables are derived data: they are rebuilt from `tracks`/`albums` after a scan that changed
//! something (about a second on the Pi for ~44 000 tracks), or when their row count does not match the index.

use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

const MAX_TOKENS: usize = 8;

/// User text -> FTS5 query: every word becomes a quoted prefix term, all must match.
/// `bea hel` -> `"bea"* "hel"*`. Only alphanumerics survive, so no FTS5 syntax can be injected.
pub fn match_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_TOKENS)
        .map(|t| format!("\"{t}\"*"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

fn count(conn: &Connection, table: &str) -> Result<i64> {
    Ok(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
}

/// Rebuild when the scan changed something, or when the FTS tables are out of step with the index
/// (first start after an upgrade that introduced them).
pub fn rebuild_if_needed(conn: &Connection, scan_changed: bool) -> Result<bool> {
    let stale = scan_changed
        || count(conn, "fts_tracks")? != count(conn, "tracks")?
        || count(conn, "fts_albums")? != count(conn, "albums")?;
    if !stale {
        return Ok(false);
    }
    let t0 = std::time::Instant::now();
    conn.execute_batch(
        "BEGIN;
         DELETE FROM fts_tracks; DELETE FROM fts_albums; DELETE FROM fts_artists;
         INSERT INTO fts_tracks(title, artist, album, track_id, album_id)
           SELECT t.title, COALESCE(t.artist, a.artist), a.name, t.id, t.album_id
           FROM tracks t JOIN albums a ON a.id = t.album_id;
         INSERT INTO fts_albums(name, artist, dir_name, album_id)
           SELECT name, artist, dir_name, id FROM albums;
         INSERT INTO fts_artists(name)
           SELECT DISTINCT artist FROM albums WHERE artist IS NOT NULL AND artist_count <= 3;
         COMMIT;",
    )?;
    info!("search index rebuilt in {:.1}s ({} tracks, {} albums)", t0.elapsed().as_secs_f64(), count(conn, "fts_tracks")?, count(conn, "fts_albums")?);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prefix_queries() {
        assert_eq!(match_query("bea hel").as_deref(), Some("\"bea\"* \"hel\"*"));
        assert_eq!(match_query("  AC/DC - Back in Black ").as_deref(), Some("\"AC\"* \"DC\"* \"Back\"* \"in\"* \"Black\"*"));
        assert_eq!(match_query("Éric").as_deref(), Some("\"Éric\"*"));
    }

    #[test]
    fn rejects_empty_and_neutralises_syntax() {
        assert_eq!(match_query(""), None);
        assert_eq!(match_query("\" * ( ) - NEAR"), Some("\"NEAR\"*".to_string()));
        assert_eq!(match_query("a\" OR \"b").as_deref(), Some("\"a\"* \"OR\"* \"b\"*"));
    }

    #[test]
    fn caps_the_number_of_terms() {
        let q = "a b c d e f g h i j k l";
        assert_eq!(match_query(q).unwrap().matches('*').count(), MAX_TOKENS);
    }
}
