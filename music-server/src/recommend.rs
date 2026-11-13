//! "More like this": suggest tracks of the library that sound like a given track.
//!
//! Two sources, merged:
//!  1. Last.fm similarity (`track.getSimilar` + `artist.getSimilar`), only when `LASTFM_API_KEY` is set. The names
//!     Last.fm returns are matched against the library (case, accents, punctuation, a leading "The" and bracketed
//!     parts are ignored); everything the library does not own is dropped.
//!  2. A local fallback built from the tags: same genre, then same artist. It also fills the list when Last.fm
//!     knows too little about the seed.
//!
//! Taste: when a Last.fm account is known (linked for scrobbling, or named in `LASTFM_USER`), its most played
//! artists (`user.getTopArtists`) lift the matching suggestions, so the list leans towards what the user really
//! listens to.
//!
//! PRIVACY: with a key set, the artist and title of the seed track (and the account name, for the taste) are sent to
//! ws.audioscrobbler.com. Nothing is sent without the key. Answers are cached in the index database (7 days for
//! similarity, 1 day for the taste). The HTTP details, and the guarantee that secrets are never logged, live in
//! `lastfm.rs`.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

use crate::lastfm::Client;
use crate::{db, online};

const CACHE_TTL_SECS: i64 = 7 * 24 * 3600;
const TOP_TTL_SECS: i64 = 24 * 3600;
/// How much the most played artist is lifted (1.0 = no lift). Others get a share, by the square root of their plays.
const TASTE_LIFT: f64 = 0.6;
/// Tracks suggested per artist, so one artist does not fill the whole list.
const MAX_PER_ARTIST: usize = 2;
/// Tracks of the seed's own artist added as a last resort.
const MAX_SAME_ARTIST: usize = 5;

#[derive(Debug, Clone)]
pub struct Seed {
    pub track_id: i64,
    pub artist: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimTrack {
    pub artist: String,
    pub title: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimArtist {
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rec {
    pub track_id: i64,
    /// `lastfm_track`, `lastfm_artist`, `same_genre` or `same_artist`.
    pub reason: &'static str,
}

pub struct Recommender {
    client: Arc<Client>,
    db_path: PathBuf,
}

/// Normalised artist -> share of the most played artist's plays (0..1).
pub type Taste = HashMap<String, f64>;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Comparable form of an artist name: "The Beatles" == "beatles", "Simon & Garfunkel" == "simon and garfunkel".
pub fn norm_artist(s: &str) -> String {
    let lower = s.trim().to_lowercase().replace('&', " and ");
    let lower = lower.strip_prefix("the ").unwrap_or(&lower);
    online::normalize(lower)
}

/// Comparable form of a title: bracketed parts ("(Remastered 2009)") and punctuation are ignored.
pub fn norm_title(s: &str) -> String {
    online::normalize(s)
}

/// Multiplier for an artist the user listens to a lot.
fn lift(taste: &Taste, artist_norm: &str) -> f64 {
    1.0 + TASTE_LIFT * taste.get(artist_norm).copied().unwrap_or(0.0).sqrt()
}

fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn parse_similar_tracks(v: &Value) -> Vec<SimTrack> {
    v["similartracks"]["track"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(SimTrack {
                        title: t["name"].as_str()?.to_string(),
                        artist: t["artist"]["name"].as_str()?.to_string(),
                        score: as_f64(&t["match"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_similar_artists(v: &Value) -> Vec<SimArtist> {
    v["similarartists"]["artist"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| Some(SimArtist { name: t["name"].as_str()?.to_string(), score: as_f64(&t["match"]) }))
                .collect()
        })
        .unwrap_or_default()
}

/// `user.getTopArtists`: the score is the play count.
pub fn parse_top_artists(v: &Value) -> Vec<SimArtist> {
    v["topartists"]["artist"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| Some(SimArtist { name: t["name"].as_str()?.to_string(), score: as_f64(&t["playcount"]) }))
                .collect()
        })
        .unwrap_or_default()
}

pub fn taste_from(top: &[SimArtist]) -> Taste {
    let max = top.iter().map(|a| a.score).fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return Taste::new();
    }
    top.iter().map(|a| (norm_artist(&a.name), a.score / max)).collect()
}

/// Deterministic pseudo-random order, so the same seed gives the same suggestions during one listening session
/// but different seeds give different picks.
fn shuffle_key(seed: i64, id: i64) -> u64 {
    let mut x = (seed as u64) ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^ (x >> 29)
}

struct LibTrack {
    id: i64,
    title_norm: String,
    genre_norm: Option<String>,
}

/// Pure ranking: library + similarity data in, ordered suggestions out.
pub fn pick(
    conn: &Connection,
    seed: &Seed,
    sim_tracks: &[SimTrack],
    sim_artists: &[SimArtist],
    taste: &Taste,
    exclude: &HashSet<i64>,
    limit: usize,
) -> Result<Vec<Rec>> {
    // One pass over the library (about 44 000 rows): normalised artist -> its tracks.
    let mut by_artist: HashMap<String, Vec<LibTrack>> = HashMap::new();
    let mut st = conn.prepare(
        "SELECT t.id, COALESCE(t.artist, a.artist), t.title, t.genre FROM tracks t JOIN albums a ON a.id = t.album_id",
    )?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
    })?;
    for row in rows {
        let (id, artist, title, genre) = row?;
        let Some(artist) = artist else { continue };
        let genre_norm = genre.map(|g| online::normalize(&g)).filter(|g| !g.is_empty());
        by_artist.entry(norm_artist(&artist)).or_default().push(LibTrack { id, title_norm: norm_title(&title), genre_norm });
    }

    let seed_artist = seed.artist.as_deref().map(norm_artist).unwrap_or_default();
    let skip = |id: i64| id == seed.track_id || exclude.contains(&id);

    // (score, track, artist, reason)
    let mut cands: Vec<(f64, i64, String, &'static str)> = vec![];

    // Exact tracks first: score in 1.0..2.0.
    for s in sim_tracks {
        let a = norm_artist(&s.artist);
        if a == seed_artist {
            continue;
        }
        let t = norm_title(&s.title);
        if let Some(tracks) = by_artist.get(&a) {
            for lt in tracks.iter().filter(|lt| lt.title_norm == t && !skip(lt.id)) {
                cands.push((1.0 + s.score * lift(taste, &a), lt.id, a.clone(), "lastfm_track"));
            }
        }
    }
    // Then tracks of similar artists: score in 0.0..1.0, a few per artist in a stable pseudo-random order.
    for s in sim_artists {
        let a = norm_artist(&s.name);
        if a == seed_artist {
            continue;
        }
        if let Some(tracks) = by_artist.get(&a) {
            let mut ids: Vec<i64> = tracks.iter().map(|t| t.id).filter(|id| !skip(*id)).collect();
            ids.sort_by_key(|id| shuffle_key(seed.track_id, *id));
            for id in ids.into_iter().take(MAX_PER_ARTIST) {
                cands.push((s.score * lift(taste, &a), id, a.clone(), "lastfm_artist"));
            }
        }
    }
    cands.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<Rec> = vec![];
    let mut seen: HashSet<i64> = HashSet::new();
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    for (_, id, artist, reason) in cands {
        if out.len() >= limit {
            break;
        }
        let n = per_artist.entry(artist).or_insert(0);
        if *n >= MAX_PER_ARTIST || !seen.insert(id) {
            continue;
        }
        *n += 1;
        out.push(Rec { track_id: id, reason });
    }

    // Local fallback 1: same genre (tags are patchy, so it only completes the list).
    if out.len() < limit {
        let seed_genre = by_artist
            .values()
            .flatten()
            .find(|t| t.id == seed.track_id)
            .and_then(|t| t.genre_norm.clone());
        if let Some(g) = seed_genre {
            let mut same: Vec<(&String, &LibTrack)> = by_artist
                .iter()
                .filter(|(a, _)| **a != seed_artist)
                .flat_map(|(a, ts)| ts.iter().map(move |t| (a, t)))
                .filter(|(_, t)| t.genre_norm.as_deref() == Some(g.as_str()) && !skip(t.id) && !seen.contains(&t.id))
                .collect();
            // Artists the user plays a lot come first, the rest in a stable pseudo-random order.
            same.sort_by_key(|(a, t)| (std::cmp::Reverse((taste.get(*a).copied().unwrap_or(0.0) * 1000.0) as u64), shuffle_key(seed.track_id, t.id)));
            for (a, t) in same {
                if out.len() >= limit {
                    break;
                }
                let n = per_artist.entry(a.clone()).or_insert(0);
                if *n >= MAX_PER_ARTIST {
                    continue;
                }
                *n += 1;
                seen.insert(t.id);
                out.push(Rec { track_id: t.id, reason: "same_genre" });
            }
        }
    }
    // Local fallback 2: other tracks of the same artist.
    if out.len() < limit {
        if let Some(tracks) = by_artist.get(&seed_artist) {
            let mut ids: Vec<i64> = tracks.iter().map(|t| t.id).filter(|id| !skip(*id) && !seen.contains(id)).collect();
            ids.sort_by_key(|id| shuffle_key(seed.track_id, *id));
            for id in ids.into_iter().take(MAX_SAME_ARTIST.min(limit - out.len())) {
                out.push(Rec { track_id: id, reason: "same_artist" });
            }
        }
    }
    Ok(out)
}

fn cache_get(conn: &Connection, key: &str, ttl_secs: i64) -> Option<String> {
    conn.query_row(
        "SELECT payload FROM lastfm_cache WHERE key=?1 AND fetched_at > ?2",
        params![key, now() - ttl_secs],
        |r| r.get(0),
    )
    .ok()
}

fn cache_put(conn: &Connection, key: &str, payload: &str) {
    let _ = conn.execute(
        "INSERT INTO lastfm_cache(key, fetched_at, payload) VALUES(?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET fetched_at=?2, payload=?3",
        params![key, now(), payload],
    );
}

impl Recommender {
    pub fn new(client: Arc<Client>, db_path: PathBuf) -> Self {
        Self { client, db_path }
    }

    pub fn enabled(&self) -> bool {
        self.client.enabled()
    }

    /// One Last.fm read, through the cache. A hit never touches the network.
    async fn cached<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>(
        &self,
        key: String,
        ttl_secs: i64,
        method: &'static str,
        extra: Vec<(&'static str, String)>,
        parse: fn(&Value) -> Vec<T>,
    ) -> Result<Vec<T>> {
        let p = self.db_path.clone();
        let k = key.clone();
        let hit = tokio::task::spawn_blocking(move || db::connect(&p).ok().and_then(|c| cache_get(&c, &k, ttl_secs))).await?;
        if let Some(json) = hit {
            if let Ok(v) = serde_json::from_str::<Vec<T>>(&json) {
                debug!("Last.fm cache hit: {key}");
                return Ok(v);
            }
        }
        let extra_ref: Vec<(&str, &str)> = extra.iter().map(|(a, b)| (*a, b.as_str())).collect();
        let items = parse(&self.client.get(method, &extra_ref).await?);
        let json = serde_json::to_string(&items)?;
        let p = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(c) = db::connect(&p) {
                cache_put(&c, &key, &json);
            }
        })
        .await?;
        Ok(items)
    }

    /// Similar tracks and artists for the seed, plus the user's taste. Never fails: a Last.fm problem is logged
    /// and gives empty answers.
    pub async fn similar(&self, seed: &Seed) -> (Vec<SimTrack>, Vec<SimArtist>, Taste) {
        let (Some(artist), true) = (seed.artist.as_deref(), self.enabled()) else {
            return (vec![], vec![], Taste::new());
        };
        let ak = norm_artist(artist);
        let tt = self.cached(
            format!("tt|{ak}|{}", norm_title(&seed.title)),
            CACHE_TTL_SECS,
            "track.getsimilar",
            vec![("artist", artist.to_string()), ("track", seed.title.clone()), ("limit", "100".into())],
            parse_similar_tracks,
        );
        let aa = self.cached(
            format!("aa|{ak}"),
            CACHE_TTL_SECS,
            "artist.getsimilar",
            vec![("artist", artist.to_string()), ("limit", "60".into())],
            parse_similar_artists,
        );
        let top = async {
            match self.client.user() {
                Some(user) => {
                    self.cached(
                        format!("top|{}", user.to_lowercase()),
                        TOP_TTL_SECS,
                        "user.gettopartists",
                        vec![("user", user), ("period", "overall".into()), ("limit", "200".into())],
                        parse_top_artists,
                    )
                    .await
                }
                None => Ok(vec![]),
            }
        };
        let (tt, aa, top) = tokio::join!(tt, aa, top);
        let tt = tt.unwrap_or_else(|e| {
            warn!("{e:#}");
            vec![]
        });
        let aa = aa.unwrap_or_else(|e| {
            warn!("{e:#}");
            vec![]
        });
        let top = top.unwrap_or_else(|e| {
            warn!("Last.fm listening history unavailable (private profile?): {e:#}");
            vec![]
        });
        (tt, aa, taste_from(&top))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_compared_loosely() {
        assert_eq!(norm_artist("The Beatles"), norm_artist("beatles"));
        assert_eq!(norm_artist("Simon & Garfunkel"), norm_artist("Simon and Garfunkel"));
        assert_eq!(norm_artist("Éric Clapton"), norm_artist("éric  clapton"));
        assert_eq!(norm_title("Help! (Remastered 2009)"), norm_title("help"));
        assert_ne!(norm_artist("Theory of a Deadman"), norm_artist("ory of a deadman"));
    }

    #[test]
    fn parses_lastfm_answers() {
        let t = json!({"similartracks": {"track": [
            {"name": "Hey Jude", "match": 1, "artist": {"name": "The Beatles"}},
            {"name": "Imagine", "match": "0.5", "artist": {"name": "John Lennon"}},
            {"match": 0.2}
        ]}});
        let v = parse_similar_tracks(&t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], SimTrack { artist: "The Beatles".into(), title: "Hey Jude".into(), score: 1.0 });
        assert_eq!(v[1].score, 0.5);
        let a = json!({"similarartists": {"artist": [{"name": "Queen", "match": "0.9"}]}});
        assert_eq!(parse_similar_artists(&a), vec![SimArtist { name: "Queen".into(), score: 0.9 }]);
        assert!(parse_similar_tracks(&json!({"error": 6, "message": "x"})).is_empty());
    }

    fn library() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(db::SCHEMA).unwrap();
        let albums = [(1, "Seed", "Seed Artist"), (2, "Queen A", "Queen"), (3, "Abba A", "ABBA"), (4, "Lonely", "Nobody Knows")];
        for (id, name, artist) in albums {
            c.execute(
                "INSERT INTO albums(id, dir, dir_name, name, artist) VALUES(?1, ?2, ?2, ?3, ?4)",
                params![id, format!("/d{id}"), name, artist],
            )
            .unwrap();
        }
        // (id, album, title, genre)
        let tracks = [
            (1, 1, "Seed Song", "Rock"),
            (2, 1, "Seed Other", "Rock"),
            (10, 2, "Bohemian Rhapsody (Remastered)", "Rock"),
            (11, 2, "Under Pressure", "Rock"),
            (12, 2, "Radio Gaga", "Rock"),
            (13, 2, "Another One", "Rock"),
            (20, 3, "Dancing Queen", "Pop"),
            (30, 4, "Only Track", "Rock"),
        ];
        for (id, album, title, genre) in tracks {
            c.execute(
                "INSERT INTO tracks(id, path, album_id, title, genre, size, mtime) VALUES(?1, ?2, ?3, ?4, ?5, 1, 1)",
                params![id, format!("/p{id}.mp3"), album, title, genre],
            )
            .unwrap();
        }
        c
    }

    fn seed() -> Seed {
        Seed { track_id: 1, artist: Some("Seed Artist".into()), title: "Seed Song".into() }
    }

    #[test]
    fn exact_tracks_come_first_and_artists_are_capped() {
        let c = library();
        let st = vec![SimTrack { artist: "queen".into(), title: "Bohemian Rhapsody".into(), score: 0.9 }];
        let sa = vec![SimArtist { name: "The Queen".into(), score: 0.8 }, SimArtist { name: "ABBA".into(), score: 0.7 }, SimArtist { name: "Unknown".into(), score: 0.6 }];
        let r = pick(&c, &seed(), &st, &sa, &Taste::new(), &HashSet::new(), 10).unwrap();
        assert_eq!(r[0], Rec { track_id: 10, reason: "lastfm_track" });
        let queen = r.iter().filter(|x| [10, 11, 12, 13].contains(&x.track_id)).count();
        assert_eq!(queen, MAX_PER_ARTIST);
        assert!(r.iter().any(|x| x.track_id == 20 && x.reason == "lastfm_artist"));
        assert!(r.iter().all(|x| x.track_id != 1), "the seed itself is never suggested");
        let ids: HashSet<i64> = r.iter().map(|x| x.track_id).collect();
        assert_eq!(ids.len(), r.len(), "no duplicates");
    }

    #[test]
    fn exclusions_and_limit_are_respected() {
        let c = library();
        let sa = vec![SimArtist { name: "Queen".into(), score: 0.8 }];
        let ex: HashSet<i64> = [10, 11, 12].into_iter().collect();
        let r = pick(&c, &seed(), &[], &sa, &Taste::new(), &ex, 1).unwrap();
        assert_eq!(r, vec![Rec { track_id: 13, reason: "lastfm_artist" }]);
    }

    #[test]
    fn falls_back_to_the_local_tags_without_lastfm() {
        let c = library();
        let r = pick(&c, &seed(), &[], &[], &Taste::new(), &HashSet::new(), 20).unwrap();
        assert!(!r.is_empty());
        // Same genre first (Queen x2, Nobody Knows), ABBA is Pop and stays out, then the seed's other track.
        assert!(r.iter().all(|x| x.track_id != 20));
        assert!(r.iter().any(|x| x.reason == "same_genre"));
        assert_eq!(r.last().unwrap(), &Rec { track_id: 2, reason: "same_artist" });
    }

    #[test]
    fn same_seed_gives_same_order() {
        let c = library();
        let a = pick(&c, &seed(), &[], &[], &Taste::new(), &HashSet::new(), 20).unwrap();
        let b = pick(&c, &seed(), &[], &[], &Taste::new(), &HashSet::new(), 20).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parses_and_normalises_the_listening_history() {
        let v = json!({"topartists": {"artist": [{"name": "Queen", "playcount": "400"}, {"name": "The ABBA", "playcount": "100"}]}});
        let top = parse_top_artists(&v);
        assert_eq!(top[0], SimArtist { name: "Queen".into(), score: 400.0 });
        let taste = taste_from(&top);
        assert_eq!(taste["queen"], 1.0);
        assert_eq!(taste["abba"], 0.25);
        assert!(taste_from(&[]).is_empty());
    }

    #[test]
    fn favourite_artists_rise_in_the_list() {
        let c = library();
        // Same Last.fm score for both: without taste ABBA does not come first, with it it does.
        let sa = vec![SimArtist { name: "Queen".into(), score: 0.7 }, SimArtist { name: "ABBA".into(), score: 0.7 }];
        let first = |taste: &Taste| pick(&c, &seed(), &[], &sa, taste, &HashSet::new(), 1).unwrap()[0].track_id;
        let mut t = Taste::new();
        t.insert("abba".into(), 1.0);
        assert_eq!(first(&t), 20);
        t.clear();
        t.insert("queen".into(), 1.0);
        assert!([10, 11, 12, 13].contains(&first(&t)));
    }
}
