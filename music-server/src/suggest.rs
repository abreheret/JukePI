//! Suggestions appended behind the play queue by the server itself, so the music goes on even when no app is open.
//!
//! The decision "is it time to add tracks?" is a pure function of the player snapshot (`wanted`), tested below.
//! The same extension is used by the explicit `POST /player/suggest_more` (an app scrolled to the end of the queue).
//!
//! Rules:
//!  * a queue that holds no suggestion yet gets one batch right after it starts ("start");
//!  * when the queue is about to run out (the last or last-but-one track plays) another batch follows ("end");
//!  * one batch at a time (`busy`), and a trigger that found nothing is not retried until the queue changes;
//!  * tracks already queued and the ones played recently are never suggested again;
//!  * nothing is added while 200 or more tracks are already waiting to be played.

use crate::player::{Cmd, PlayerHandle, Snapshot, Status};
use crate::recommend::Recommender;
use crate::{api, db};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Tracks added per batch.
pub const BATCH: usize = 10;
/// No batch while this many tracks are already waiting after the current one.
const MAX_AHEAD: usize = 200;
/// Tracks played recently that are not suggested again.
const RECENT: usize = 300;

/// What the snapshot asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Wanted {
    /// Nothing now; keep what was already done for this queue.
    Hold,
    /// Nothing to do and the queue is not in a state that needs a batch: forget the last trigger.
    Reset,
    /// A batch, identified by a key so the same trigger fires once.
    Batch(String),
}

pub fn wanted(s: &Snapshot) -> Wanted {
    if !s.auto_suggest || s.queue.is_empty() {
        return Wanted::Reset;
    }
    if s.status != Status::Playing && s.status != Status::Loading {
        return Wanted::Hold;
    }
    let Some(index) = s.index else { return Wanted::Hold };
    let ahead = s.queue.len().saturating_sub(index + 1);
    if ahead >= MAX_AHEAD {
        return Wanted::Hold;
    }
    if !s.queue.iter().any(|q| q.reason.is_some()) {
        return Wanted::Batch(format!("start:{}", s.queue[0].track_id));
    }
    if ahead <= 1 {
        return Wanted::Batch(format!("end:{}", s.queue[s.queue.len() - 1].track_id));
    }
    Wanted::Reset
}

struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct Suggester {
    recommend: Arc<Recommender>,
    db_path: PathBuf,
    player: PlayerHandle,
    setting_path: PathBuf,
    busy: AtomicBool,
    recent: Mutex<VecDeque<i64>>,
}

/// The saved setting; on when nothing was ever saved.
pub fn read_setting(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).map(|t| t.trim() != "off").unwrap_or(true)
}

impl Suggester {
    pub fn new(recommend: Arc<Recommender>, db_path: PathBuf, player: PlayerHandle, setting_path: PathBuf) -> Arc<Self> {
        Arc::new(Self { recommend, db_path, player, setting_path, busy: AtomicBool::new(false), recent: Mutex::new(VecDeque::new()) })
    }

    /// Turn the automatic suggestions on or off for every client, and remember it across restarts.
    pub async fn set_enabled(&self, on: bool) -> Result<()> {
        std::fs::write(&self.setting_path, if on { "on" } else { "off" })?;
        self.player.send(Cmd::AutoSuggest(on)).await
    }

    /// A track the user took out of the queue: it is not suggested again soon.
    pub fn avoid(&self, track_id: i64) {
        if let Ok(mut r) = self.recent.lock() {
            r.push_back(track_id);
            while r.len() > RECENT {
                r.pop_front();
            }
        }
    }

    fn note_played(&self, s: &Snapshot) {
        let Some(id) = s.current.as_ref().map(|c| c.track_id) else { return };
        if let Ok(mut r) = self.recent.lock() {
            if r.back() != Some(&id) {
                r.push_back(id);
                while r.len() > RECENT {
                    r.pop_front();
                }
            }
        }
    }

    /// Watch the player and add a batch whenever `wanted` says so.
    pub fn spawn(self: Arc<Self>) {
        let mut rx = self.player.watch();
        tokio::spawn(async move {
            let mut done: Option<String> = None;
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let s = rx.borrow_and_update().clone();
                self.note_played(&s);
                match wanted(&s) {
                    Wanted::Hold => {}
                    Wanted::Reset => done = None,
                    Wanted::Batch(key) => {
                        if done.as_deref() != Some(key.as_str()) {
                            done = Some(key.clone());
                            match self.extend(BATCH).await {
                                Ok(n) => info!("suggestions ({key}): {n} track(s) added"),
                                Err(e) => warn!("suggestions ({key}) failed: {e:#}"),
                            }
                        }
                    }
                }
            }
        });
    }

    /// Append up to `limit` tracks close to the last track of the queue. Returns how many were added; 0 when a batch is
    /// already being prepared, when nothing fits, or when the queue changed in the meantime.
    pub async fn extend(&self, limit: usize) -> Result<usize> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Ok(0);
        }
        let _busy = Busy(&self.busy);
        let snap = self.player.snapshot();
        let Some(seed) = snap.queue.last().map(|q| q.track_id) else { return Ok(0) };
        if snap.queue.len().saturating_sub(snap.index.map_or(0, |i| i + 1)) >= MAX_AHEAD {
            return Ok(0);
        }
        let mut exclude: HashSet<i64> = snap.queue.iter().map(|q| q.track_id).collect();
        if let Ok(r) = self.recent.lock() {
            exclude.extend(r.iter().copied());
        }
        let Some(out) = self.recommend.recommend(seed, limit, exclude).await? else { return Ok(0) };
        if out.recs.is_empty() {
            return Ok(0);
        }
        let ids: Vec<i64> = out.recs.iter().map(|r| r.track_id).collect();
        let reasons: HashMap<i64, &'static str> = out.recs.iter().map(|r| (r.track_id, r.reason)).collect();
        let p = self.db_path.clone();
        let mut items = tokio::task::spawn_blocking(move || {
            let c = db::connect(&p)?;
            api::load_items(&c, &ids).map_err(|_| anyhow!("a suggested track is not in the index"))
        })
        .await??;
        for i in &mut items {
            i.reason = reasons.get(&i.track_id).map(|r| r.to_string());
        }
        // A new queue may have started while Last.fm was answering: its tracks must not get these suggestions.
        if self.player.snapshot().queue.last().map(|q| q.track_id) != Some(seed) {
            return Ok(0);
        }
        let n = items.len();
        self.player.send(Cmd::Enqueue(items)).await?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::QueueItem;

    fn item(id: i64, reason: Option<&str>) -> QueueItem {
        QueueItem { track_id: id, album_id: 1, title: id.to_string(), artist: None, duration_ms: None, duration_exact: false, reason: reason.map(String::from) }
    }

    fn snap(queue: Vec<QueueItem>, index: Option<usize>, status: Status, auto: bool) -> Snapshot {
        Snapshot {
            status,
            heos_connected: true,
            index,
            current: index.and_then(|i| queue.get(i).cloned()),
            position_ms: 0,
            duration_ms: 0,
            volume: None,
            mute: false,
            queue,
            auto_suggest: auto,
            error: None,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn a_fresh_queue_gets_a_batch_once_playing() {
        let q = vec![item(1, None), item(2, None), item(3, None)];
        assert_eq!(wanted(&snap(q.clone(), Some(0), Status::Playing, true)), Wanted::Batch("start:1".into()));
        assert_eq!(wanted(&snap(q.clone(), Some(0), Status::Loading, true)), Wanted::Batch("start:1".into()));
        assert_eq!(wanted(&snap(q.clone(), Some(0), Status::Paused, true)), Wanted::Hold);
        assert_eq!(wanted(&snap(q, Some(0), Status::Stopped, true)), Wanted::Hold);
    }

    #[test]
    fn nothing_when_off_or_empty() {
        let q = vec![item(1, None)];
        assert_eq!(wanted(&snap(q, Some(0), Status::Playing, false)), Wanted::Reset);
        assert_eq!(wanted(&snap(vec![], None, Status::Playing, true)), Wanted::Reset);
    }

    #[test]
    fn a_batch_follows_when_the_queue_runs_short() {
        let mut q: Vec<QueueItem> = (1..=3).map(|i| item(i, None)).collect();
        q.extend((4..=6).map(|i| item(i, Some("same_genre"))));
        assert_eq!(wanted(&snap(q.clone(), Some(1), Status::Playing, true)), Wanted::Reset);
        assert_eq!(wanted(&snap(q.clone(), Some(4), Status::Playing, true)), Wanted::Batch("end:6".into()));
        assert_eq!(wanted(&snap(q, Some(5), Status::Playing, true)), Wanted::Batch("end:6".into()));
    }

    #[test]
    fn no_batch_while_many_tracks_wait() {
        let mut q: Vec<QueueItem> = (1..=300).map(|i| item(i, Some("same_genre"))).collect();
        q[0].reason = None;
        assert_eq!(wanted(&snap(q.clone(), Some(0), Status::Playing, true)), Wanted::Hold);
        assert_eq!(wanted(&snap(q, Some(250), Status::Playing, true)), Wanted::Reset);
    }

    #[test]
    fn the_setting_defaults_to_on() {
        let dir = std::env::temp_dir().join(format!("jukepi-sug-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("auto_suggest");
        assert!(read_setting(&p));
        std::fs::write(&p, "off").unwrap();
        assert!(!read_setting(&p));
        std::fs::write(&p, "on").unwrap();
        assert!(read_setting(&p));
        let _ = std::fs::remove_dir_all(dir);
    }
}
