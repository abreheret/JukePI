//! Scrobbling: tells Last.fm what the Denon plays, so the listening history on the account grows.
//!
//! Only what this server plays on the amplifier is sent (the phone's own player is not covered). Last.fm's rules: a
//! track counts once half of it (or 4 minutes) has been played, and only if it lasts more than 30 seconds; the
//! timestamp is the moment the track started. A "now playing" notice is sent when a track starts.
//!
//! PRIVACY: needs `LASTFM_API_KEY`, `LASTFM_SECRET` and an account linked through `/lastfm/auth/start`. Without all
//! three nothing is sent. Scrobbles that cannot be sent (no Internet) wait in the index database and go out later.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex};
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::lastfm::Client;
use crate::{db, player};

const MIN_TRACK_MS: u64 = 30_000;
const MAX_THRESHOLD_MS: u64 = 240_000;
/// A gap bigger than this between two looks at the player (suspend, stall) is not counted as listening time.
const MAX_GAP_MS: u64 = 5_000;
const BATCH: usize = 50;
/// Last.fm refuses scrobbles older than 2 weeks.
const MAX_AGE_SECS: i64 = 14 * 24 * 3600;

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    NowPlaying { track_id: i64 },
    Scrobble { track_id: i64, timestamp: i64 },
}

/// How much of a track must be played before it counts. None: too short to be scrobbled.
pub fn threshold_ms(duration_ms: u64) -> Option<u64> {
    match duration_ms {
        0 => Some(MAX_THRESHOLD_MS), // unknown (still loading): judge again once it is known
        d if d < MIN_TRACK_MS => None,
        d => Some((d / 2).min(MAX_THRESHOLD_MS)),
    }
}

/// Follows the playing track and decides when to announce / scrobble it. Pure: time is given by the caller.
#[derive(Default)]
pub struct Tracker {
    key: Option<(i64, usize)>,
    started_unix: i64,
    played_ms: u64,
    last_ms: Option<u64>,
    scrobbled: bool,
    now_playing_sent: bool,
}

impl Tracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `cur` = (track id, queue index) of the current track. `now_ms` must be monotonic.
    pub fn update(&mut self, now_ms: u64, unix_s: i64, cur: Option<(i64, usize)>, playing: bool, stopped: bool, duration_ms: u64) -> Vec<Action> {
        if stopped {
            self.reset(); // playing the same track again later is a new listen
            return vec![];
        }
        let Some(cur) = cur else { return vec![] };
        if self.key != Some(cur) {
            self.reset();
            self.key = Some(cur);
            self.started_unix = unix_s;
        }
        if let Some(l) = self.last_ms {
            self.played_ms += now_ms.saturating_sub(l).min(MAX_GAP_MS);
        }
        self.last_ms = playing.then_some(now_ms);

        let mut out = vec![];
        if playing && !self.now_playing_sent {
            self.now_playing_sent = true;
            out.push(Action::NowPlaying { track_id: cur.0 });
        }
        if !self.scrobbled {
            if let Some(th) = threshold_ms(duration_ms) {
                if self.played_ms >= th {
                    self.scrobbled = true;
                    out.push(Action::Scrobble { track_id: cur.0, timestamp: self.started_unix });
                }
            }
        }
        out
    }
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

struct Meta {
    artist: String,
    title: String,
    album: Option<String>,
    duration_s: Option<u64>,
}

fn album_of(conn: &Connection, track_id: i64) -> Option<String> {
    conn.query_row("SELECT a.name FROM tracks t JOIN albums a ON a.id = t.album_id WHERE t.id=?1", params![track_id], |r| r.get(0)).ok()
}

fn queue_row(conn: &Connection, m: &Meta, ts: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO scrobble_queue(artist, track, album, ts, duration) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![m.artist, m.title, m.album, ts, m.duration_s.map(|d| d as i64)],
    )?;
    Ok(())
}

/// Send what is waiting (oldest first, 50 at a time). A failure leaves everything queued for the next try.
async fn flush(client: &Client, db_path: &PathBuf) {
    let p = db_path.clone();
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String, String, Option<String>, i64, Option<i64>)>> {
        let c = db::connect(&p)?;
        c.execute("DELETE FROM scrobble_queue WHERE ts < ?1", params![unix_now() - MAX_AGE_SECS])?;
        let mut st = c.prepare("SELECT id, artist, track, album, ts, duration FROM scrobble_queue ORDER BY ts LIMIT ?1")?;
        let v = st.query_map(params![BATCH as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    })
    .await;
    let Ok(Ok(rows)) = rows else { return };
    if rows.is_empty() {
        return;
    }
    let mut form: Vec<(String, String)> = vec![];
    for (i, (_, artist, track, album, ts, dur)) in rows.iter().enumerate() {
        form.push((format!("artist[{i}]"), artist.clone()));
        form.push((format!("track[{i}]"), track.clone()));
        form.push((format!("timestamp[{i}]"), ts.to_string()));
        if let Some(a) = album {
            form.push((format!("album[{i}]"), a.clone()));
        }
        if let Some(d) = dur {
            form.push((format!("duration[{i}]"), d.to_string()));
        }
    }
    match client.post_signed("track.scrobble", &form, true).await {
        Ok(v) => {
            let accepted = v["scrobbles"]["@attr"]["accepted"].as_i64().unwrap_or(-1);
            info!("Last.fm: {} scrobble(s) sent ({accepted} accepted)", rows.len());
            let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
            let p = db_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(c) = db::connect(&p) {
                    for id in ids {
                        let _ = c.execute("DELETE FROM scrobble_queue WHERE id=?1", params![id]);
                    }
                }
            })
            .await;
        }
        Err(e) => warn!("Last.fm scrobble failed, will retry: {e:#}"),
    }
}

/// Watch the player and scrobble. Runs for the life of the process; does nothing until an account is linked.
pub fn spawn(client: Arc<Client>, db_path: PathBuf, mut rx: watch::Receiver<player::Snapshot>) {
    tokio::spawn(async move {
        let start = tokio::time::Instant::now();
        let mut tracker = Tracker::default();
        let mut tick = interval(Duration::from_secs(1));
        let mut retry = interval(Duration::from_secs(300));
        let flush_lock = Arc::new(Mutex::new(()));
        loop {
            tokio::select! {
                r = rx.changed() => { if r.is_err() { break; } }
                _ = tick.tick() => {}
                _ = retry.tick() => {
                    if client.can_scrobble() {
                        let (c, p, l) = (client.clone(), db_path.clone(), flush_lock.clone());
                        tokio::spawn(async move { let _g = l.lock().await; flush(&c, &p).await; });
                    }
                    continue;
                }
            }
            if !client.can_scrobble() {
                tracker.reset();
                continue;
            }
            // Read what is needed without copying the whole queue.
            let (cur, playing, stopped, duration, meta) = {
                let s = rx.borrow();
                let cur = s.current.as_ref().zip(s.index).map(|(c, i)| (c.track_id, i));
                let meta = s.current.as_ref().and_then(|c| {
                    c.artist.clone().map(|artist| Meta {
                        artist,
                        title: c.title.clone(),
                        album: None,
                        duration_s: (s.duration_ms > 0).then_some(s.duration_ms / 1000),
                    })
                });
                (cur, s.status == player::Status::Playing, s.status == player::Status::Stopped, s.duration_ms, meta)
            };
            let now_ms = start.elapsed().as_millis() as u64;
            for action in tracker.update(now_ms, unix_now(), cur, playing, stopped, duration) {
                let Some(mut m) = meta.as_ref().map(|m| Meta { artist: m.artist.clone(), title: m.title.clone(), album: None, duration_s: m.duration_s }) else {
                    continue; // no artist tag: Last.fm cannot take it
                };
                match action {
                    Action::NowPlaying { track_id } => {
                        let (c, p) = (client.clone(), db_path.clone());
                        tokio::spawn(async move {
                            let album = tokio::task::spawn_blocking(move || db::connect(&p).ok().and_then(|c| album_of(&c, track_id))).await.ok().flatten();
                            let mut form = vec![("artist".to_string(), m.artist), ("track".to_string(), m.title)];
                            if let Some(a) = album {
                                form.push(("album".into(), a));
                            }
                            if let Some(d) = m.duration_s {
                                form.push(("duration".into(), d.to_string()));
                            }
                            if let Err(e) = c.post_signed("track.updateNowPlaying", &form, true).await {
                                debug!("Last.fm now playing failed: {e:#}");
                            }
                        });
                    }
                    Action::Scrobble { track_id, timestamp } => {
                        let (c, p, l) = (client.clone(), db_path.clone(), flush_lock.clone());
                        tokio::spawn(async move {
                            let p2 = p.clone();
                            let queued = tokio::task::spawn_blocking(move || -> Result<()> {
                                let conn = db::connect(&p2)?;
                                m.album = album_of(&conn, track_id);
                                queue_row(&conn, &m, timestamp)
                            })
                            .await;
                            if !matches!(queued, Ok(Ok(()))) {
                                warn!("could not queue a scrobble");
                                return;
                            }
                            let _g = l.lock().await;
                            flush(&c, &p).await;
                        });
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_follow_the_lastfm_rules() {
        assert_eq!(threshold_ms(20_000), None, "under 30 s never counts");
        assert_eq!(threshold_ms(30_000), Some(15_000));
        assert_eq!(threshold_ms(200_000), Some(100_000), "half of the track");
        assert_eq!(threshold_ms(900_000), Some(240_000), "at most 4 minutes");
        assert_eq!(threshold_ms(0), Some(240_000));
    }

    fn run(t: &mut Tracker, from_ms: u64, to_ms: u64, cur: (i64, usize), dur: u64) -> Vec<Action> {
        let mut acts = vec![];
        let mut now = from_ms;
        while now <= to_ms {
            acts.extend(t.update(now, 1_000 + (now / 1000) as i64, Some(cur), true, false, dur));
            now += 1000;
        }
        acts
    }

    #[test]
    fn announces_then_scrobbles_once_at_half() {
        let mut t = Tracker::default();
        let acts = run(&mut t, 0, 150_000, (7, 0), 200_000);
        assert_eq!(acts[0], Action::NowPlaying { track_id: 7 });
        let scrobbles: Vec<_> = acts.iter().filter(|a| matches!(a, Action::Scrobble { .. })).collect();
        assert_eq!(scrobbles, vec![&Action::Scrobble { track_id: 7, timestamp: 1_000 }], "once, stamped with the start");
    }

    #[test]
    fn pause_time_does_not_count() {
        let mut t = Tracker::default();
        t.update(0, 1_000, Some((7, 0)), true, false, 100_000);
        for s in 1..=60 {
            assert!(t.update(s * 1000, 1_000, Some((7, 0)), false, false, 100_000).iter().all(|a| !matches!(a, Action::Scrobble { .. })));
        }
        // 60 s of pause, then 49 s of play: still under half of 100 s.
        let a = run(&mut t, 61_000, 109_000, (7, 0), 100_000);
        assert!(a.iter().all(|x| !matches!(x, Action::Scrobble { .. })));
        let a = run(&mut t, 110_000, 112_000, (7, 0), 100_000);
        assert!(a.iter().any(|x| matches!(x, Action::Scrobble { .. })));
    }

    #[test]
    fn skipping_early_scrobbles_nothing_and_next_track_starts_fresh() {
        let mut t = Tracker::default();
        assert!(run(&mut t, 0, 20_000, (1, 0), 200_000).iter().all(|a| !matches!(a, Action::Scrobble { .. })));
        let a = run(&mut t, 21_000, 22_000, (2, 1), 200_000);
        assert_eq!(a[0], Action::NowPlaying { track_id: 2 });
    }

    #[test]
    fn short_tracks_never_count_and_the_same_track_again_does() {
        let mut t = Tracker::default();
        assert!(run(&mut t, 0, 60_000, (1, 0), 25_000).iter().all(|a| !matches!(a, Action::Scrobble { .. })));
        let mut t = Tracker::default();
        assert!(run(&mut t, 0, 120_000, (5, 0), 100_000).iter().any(|a| matches!(a, Action::Scrobble { .. })));
        t.update(130_000, 2_000, Some((5, 0)), false, true, 100_000); // stopped
        let a = run(&mut t, 140_000, 200_000, (5, 0), 100_000);
        assert!(a.iter().any(|x| matches!(x, Action::Scrobble { .. })), "a second listen counts again");
    }

    #[test]
    fn a_long_gap_is_not_listening_time() {
        let mut t = Tracker::default();
        t.update(0, 1_000, Some((7, 0)), true, false, 100_000);
        // The process was frozen for an hour: at most MAX_GAP_MS is credited.
        let a = t.update(3_600_000, 5_000, Some((7, 0)), true, false, 100_000);
        assert!(a.iter().all(|x| !matches!(x, Action::Scrobble { .. })));
    }
}
