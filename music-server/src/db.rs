use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

pub(crate) const SCHEMA: &str = "
-- AUTOINCREMENT: an id is never given again after its album/track is deleted. The app caches covers and keeps
-- downloads by id: a reused id would show another album's cover or claim another album is downloaded.
CREATE TABLE IF NOT EXISTS albums(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  dir TEXT NOT NULL UNIQUE,
  dir_name TEXT NOT NULL,
  name TEXT NOT NULL,
  artist TEXT,
  -- normalised artist (no case, accents, punctuation, leading The): the spellings of one artist share one key
  artist_key TEXT,
  artist_count INTEGER NOT NULL DEFAULT 0,
  year INTEGER,
  track_count INTEGER NOT NULL DEFAULT 0,
  added_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS tracks(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL UNIQUE,
  album_id INTEGER NOT NULL REFERENCES albums(id),
  title TEXT NOT NULL,
  artist TEXT,
  album_tag TEXT,
  genre TEXT,
  track_no INTEGER,
  disc_no INTEGER,
  year INTEGER,
  duration_ms INTEGER,
  -- 1: duration_ms is exact (read from the file's Xing/Info/VBRI header), 2: checked, no such header (an estimate), 0: not checked yet
  duration_exact INTEGER NOT NULL DEFAULT 0,
  bitrate_kbps INTEGER,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id);
CREATE INDEX IF NOT EXISTS albums_artist ON albums(artist);
-- Full-text search (rebuilt by search.rs after a scan that changed something). unicode61 + remove_diacritics:
-- eric finds Éric, prefix indexes make type-ahead (bea -> Beatles) cheap.
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tracks USING fts5(
  title, artist, album, track_id UNINDEXED, album_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE VIRTUAL TABLE IF NOT EXISTS fts_albums USING fts5(
  name, artist, dir_name, album_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
CREATE VIRTUAL TABLE IF NOT EXISTS fts_artists USING fts5(
  name, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
-- Where each album's cover comes from. kind: file | embedded | online | none.
-- source: path relative to MUSIC_ROOT (file: the image, embedded: the MP3 holding it) or, for online, a file in the cache dir.
-- tracks_stamp/dir_mtime detect a changed album so it is resolved again; source_mtime/size feed the thumbnail cache key.
CREATE TABLE IF NOT EXISTS album_covers(
  album_id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  source TEXT,
  source_mtime INTEGER NOT NULL DEFAULT 0,
  source_size INTEGER NOT NULL DEFAULT 0,
  tracks_stamp INTEGER NOT NULL DEFAULT 0,
  dir_mtime INTEGER NOT NULL DEFAULT 0,
  checked_at INTEGER NOT NULL DEFAULT 0,
  online_checked_at INTEGER NOT NULL DEFAULT 0
);
-- Albums the user started (the last time and how many times): the Recent tab of the app. Cleared with DELETE /history.
CREATE TABLE IF NOT EXISTS album_plays(
  album_id INTEGER PRIMARY KEY,
  played_at INTEGER NOT NULL,
  plays INTEGER NOT NULL DEFAULT 1
);
-- Listens waiting to be sent to Last.fm (scrobble.rs). Emptied as soon as they are accepted.
CREATE TABLE IF NOT EXISTS scrobble_queue(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  artist TEXT NOT NULL,
  track TEXT NOT NULL,
  album TEXT,
  ts INTEGER NOT NULL,
  duration INTEGER
);
-- Last.fm similarity answers (recommend.rs), kept 7 days. Safe to drop: it is only a cache.
CREATE TABLE IF NOT EXISTS lastfm_cache(
  key TEXT PRIMARY KEY,
  fetched_at INTEGER NOT NULL,
  payload TEXT NOT NULL
);
";

/// Open (and create if needed) the index database. WAL lets the API read while a scan writes.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(SCHEMA)?;
    // Index created by an older version: add the column (a no-op on a new one).
    add_column_if_missing(&conn, "tracks", "duration_exact", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(&conn, "albums", "artist_key", "TEXT")?;
    conn.execute_batch("CREATE INDEX IF NOT EXISTS albums_artist_key ON albums(artist_key)")?;
    // Index created before ids were AUTOINCREMENT: rebuild those tables, keeping every id.
    for table in ["albums", "tracks"] {
        make_autoincrement(&conn, table)?;
    }
    Ok(conn)
}

/// Rebuilds `table` with `id INTEGER PRIMARY KEY AUTOINCREMENT` if it does not have it yet. Ids, columns
/// (including the ones added later by ALTER TABLE) and indexes are kept; the next id is above the highest one.
fn make_autoincrement(conn: &Connection, table: &str) -> rusqlite::Result<()> {
    let sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name=?1", [table], |r| r.get(0))?;
    if sql.contains("AUTOINCREMENT") {
        return Ok(());
    }
    let new_sql = sql
        .replacen("id INTEGER PRIMARY KEY", "id INTEGER PRIMARY KEY AUTOINCREMENT", 1)
        .replacen(&format!("TABLE {table}("), &format!("TABLE {table}_rebuild("), 1)
        .replacen(&format!("TABLE \"{table}\"("), &format!("TABLE {table}_rebuild("), 1);
    if !new_sql.contains("AUTOINCREMENT") || !new_sql.contains(&format!("{table}_rebuild(")) {
        return Ok(()); // unexpected definition: leave it alone rather than guess
    }
    let indexes: Vec<String> = {
        let mut st = conn.prepare("SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL")?;
        let v = st.query_map([table], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    conn.execute_batch(&format!(
        "PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;
         {new_sql};
         INSERT INTO {table}_rebuild SELECT * FROM {table};
         DROP TABLE {table};
         ALTER TABLE {table}_rebuild RENAME TO {table};
         {indexes}
         COMMIT;",
        indexes = indexes.iter().map(|i| format!("{i};")).collect::<Vec<_>>().join("\n")
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_ids_are_never_given_again() {
        let path = std::env::temp_dir().join(format!("jukepi-autoinc-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            // an index made by an older version: plain INTEGER PRIMARY KEY, the newest album deleted
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE albums(id INTEGER PRIMARY KEY, dir TEXT NOT NULL UNIQUE, dir_name TEXT NOT NULL, name TEXT NOT NULL,
                   artist TEXT, artist_count INTEGER NOT NULL DEFAULT 0, year INTEGER, track_count INTEGER NOT NULL DEFAULT 0,
                   added_at INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE tracks(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, album_id INTEGER NOT NULL REFERENCES albums(id),
                   title TEXT NOT NULL, artist TEXT, album_tag TEXT, genre TEXT, track_no INTEGER, disc_no INTEGER, year INTEGER,
                   duration_ms INTEGER, bitrate_kbps INTEGER, size INTEGER NOT NULL, mtime INTEGER NOT NULL);
                 CREATE INDEX tracks_album ON tracks(album_id);
                 INSERT INTO albums(id, dir, dir_name, name) VALUES(1, 'a', 'a', 'A'), (2, 'b', 'b', 'B'), (3, 'c', 'c', 'C');
                 INSERT INTO tracks(id, path, album_id, title, size, mtime) VALUES(10, 'a/1.mp3', 1, 't', 1, 1), (11, 'c/1.mp3', 3, 'u', 1, 1);
                 DELETE FROM tracks WHERE album_id = 3; DELETE FROM albums WHERE id = 3;",
            )
            .unwrap();
        }
        let c = open(&path).unwrap();
        let ids: Vec<i64> = c.prepare("SELECT id FROM albums ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(ids, [1, 2], "existing ids kept");
        let t: i64 = c.query_row("SELECT album_id FROM tracks WHERE id = 10", [], |r| r.get(0)).unwrap();
        assert_eq!(t, 1);
        c.execute_batch("DELETE FROM albums WHERE id = 2").unwrap(); // the newest one now
        c.execute_batch("INSERT INTO albums(dir, dir_name, name) VALUES('d', 'd', 'D')").unwrap();
        let new_id: i64 = c.query_row("SELECT id FROM albums WHERE dir = 'd'", [], |r| r.get(0)).unwrap();
        assert!(new_id > 2, "id {new_id} was given again");
        c.execute_batch("INSERT INTO tracks(path, album_id, title, size, mtime) VALUES('d/1.mp3', 1, 'v', 1, 1)").unwrap();
        let tid: i64 = c.query_row("SELECT id FROM tracks WHERE path = 'd/1.mp3'", [], |r| r.get(0)).unwrap();
        assert!(tid > 10);
        let idx: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='tracks_album'", [], |r| r.get(0)).unwrap();
        assert_eq!(idx, 1, "indexes kept");
        drop(c);
        let _ = open(&path).unwrap(); // second open: already AUTOINCREMENT, nothing to do
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_old_index_gets_the_new_column() {
        let path = std::env::temp_dir().join(format!("jukepi-old-index-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = open(&path).unwrap();
            c.execute_batch("INSERT INTO albums(id, dir, dir_name, name) VALUES(1, 'd', 'd', 'n')").unwrap();
            c.execute_batch("INSERT INTO tracks(path, album_id, title, size, mtime) VALUES('a.mp3', 1, 't', 1, 1)").unwrap();
            c.execute_batch("ALTER TABLE tracks DROP COLUMN duration_exact").unwrap(); // what an older version left behind
            c.execute_batch("DROP INDEX albums_artist_key; ALTER TABLE albums DROP COLUMN artist_key").unwrap();
        }
        let c = open(&path).unwrap();
        let v: i64 = c.query_row("SELECT duration_exact FROM tracks WHERE path='a.mp3'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 0, "existing rows start as 'not checked'");
        let k: Option<String> = c.query_row("SELECT artist_key FROM albums WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(k, None, "the artist key is filled in by the scan module at start-up");
        let _ = open(&path).unwrap(); // second open: no error
        drop(c);
        let _ = std::fs::remove_file(&path);
    }
}

fn add_column_if_missing(conn: &Connection, table: &str, column: &str, ddl: &str) -> rusqlite::Result<()> {
    let has: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{column}'"), [], |r| r.get(0))?;
    if has == 0 {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"))?;
    }
    Ok(())
}

/// Cheap connection for API handlers (schema already created at startup).
pub fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}
