//! Play queue + state machine driving the Denon through HEOS `play_stream`.
//!
//! Facts measured on the real DRA-900H (see docs/HEOS-FINDINGS.md) that shape this file:
//!  * `play_stream` plays ONE url; the queue lives here, we chain tracks ourselves.
//!  * Every `play_stream` (and `clear_queue`) emits a spurious `state=stop` before `state=play`.
//!    While a transition we started is in flight, `stop` events are ignored.
//!  * `clear_queue` while playing stops the track. We only clear right after our own `stop`,
//!    because the Denon keeps an internal queue and wraps around to stale entries otherwise.
//!  * End of track = a `stop` we did not cause, with the estimated position within ~3 s of the duration.
//!  * `player_now_playing_progress` arrives every ~5 s, so position is extrapolated in between.
//!  * The gap between two tracks is ~0.3-0.5 s; this cannot be improved with pre-emptive sends.

use crate::heos::{HeosClient, HeosEvent};
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, Instant};
use tracing::{info, warn};

const NATURAL_END_MARGIN_MS: u64 = 3000;
const TRANSITION_TIMEOUT: Duration = Duration::from_secs(12);
const ERROR_DEDUPE: Duration = Duration::from_secs(3);
const MAX_ERRORS_IN_ROW: u32 = 3;
const PREV_RESTART_AFTER_MS: u64 = 3000;

#[derive(Clone, Serialize, Debug)]
pub struct QueueItem {
    pub track_id: i64,
    pub album_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub duration_ms: Option<i64>,
    /// `duration_ms` is exact (read from the file's VBR header): the stream handed to the Denon has no such header, so
    /// the Denon can only estimate, and this value is the one to trust.
    #[serde(default)]
    pub duration_exact: bool,
}

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Loading,
    Playing,
    Paused,
}

#[derive(Clone, Serialize, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub heos_connected: bool,
    pub index: Option<usize>,
    pub current: Option<QueueItem>,
    /// Position at `updated_at_ms`; clients extrapolate while status == playing.
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: Option<u8>,
    pub mute: bool,
    pub queue: Vec<QueueItem>,
    pub error: Option<String>,
    pub updated_at_ms: u64,
}

/// Same bound as the API's `/player/play`.
const MAX_QUEUE: usize = 1000;

pub enum Cmd {
    Play { items: Vec<QueueItem>, start: usize },
    Jump(usize),
    /// Append to the end of the queue without touching what is playing.
    Enqueue(Vec<QueueItem>),
    Next,
    Prev,
    Pause,
    Resume,
    Stop,
    Volume(u8),
    Mute(bool),
    /// Absolute position in the current track.
    Seek(u64),
    /// Relative move (negative = backwards) from the current position.
    SeekBy(i64),
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: mpsc::Sender<Cmd>,
    snap: watch::Receiver<Snapshot>,
}

impl PlayerHandle {
    pub async fn send(&self, cmd: Cmd) -> anyhow::Result<()> {
        self.tx.send(cmd).await.map_err(|_| anyhow::anyhow!("player task stopped"))
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snap.borrow().clone()
    }
    pub fn watch(&self) -> watch::Receiver<Snapshot> {
        self.snap.clone()
    }
}

/// True when a `stop` event looks like the track reaching its end (vs. the user or another app stopping it).
/// Unknown duration (0) cannot be judged: treat it as an end, the stop was not caused by us.
pub fn is_natural_end(duration_ms: u64, est_position_ms: u64) -> bool {
    duration_ms == 0 || est_position_ms + NATURAL_END_MARGIN_MS >= duration_ms
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Actor {
    heos: HeosClient,
    base_url: String,
    queue: Vec<QueueItem>,
    index: Option<usize>,
    status: Status,
    pos_ms: u64,
    pos_at: Instant,
    dur_ms: u64,
    volume: Option<u8>,
    mute: bool,
    /// Where the stream we handed to the Denon starts inside the track (0 unless we sought). The Denon reports
    /// its position from the start of THAT stream, so the real position is `offset_ms + cur_pos`.
    offset_ms: u64,
    error: Option<String>,
    /// Set from the moment we ask the Denon to start a track until it reports `play`.
    transition: Option<Instant>,
    errors_in_row: u32,
    last_error_at: Option<Instant>,
    pid: Option<i64>,
    snap_tx: watch::Sender<Snapshot>,
}

pub fn spawn(heos: HeosClient, base_url: String) -> PlayerHandle {
    let (tx, rx) = mpsc::channel(32);
    let mut actor = Actor {
        heos,
        base_url,
        queue: vec![],
        index: None,
        status: Status::Stopped,
        pos_ms: 0,
        pos_at: Instant::now(),
        dur_ms: 0,
        volume: None,
        mute: false,
        offset_ms: 0,
        error: None,
        transition: None,
        errors_in_row: 0,
        last_error_at: None,
        pid: None,
        snap_tx: watch::channel(empty_snapshot()).0,
    };
    let (snap_tx, snap_rx) = watch::channel(actor.snapshot());
    actor.snap_tx = snap_tx;
    tokio::spawn(actor.run(rx));
    PlayerHandle { tx, snap: snap_rx }
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        status: Status::Stopped,
        heos_connected: false,
        index: None,
        current: None,
        position_ms: 0,
        duration_ms: 0,
        volume: None,
        mute: false,
        queue: vec![],
        error: None,
        updated_at_ms: now_ms(),
    }
}

impl Actor {
    fn est_pos(&self) -> u64 {
        let p = if self.status == Status::Playing { self.pos_ms + self.pos_at.elapsed().as_millis() as u64 } else { self.pos_ms };
        if self.dur_ms > 0 { p.min(self.dur_ms) } else { p }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            status: self.status,
            heos_connected: self.heos.is_connected(),
            index: self.index,
            current: self.index.and_then(|i| self.queue.get(i).cloned()),
            position_ms: self.est_pos(),
            duration_ms: self.dur_ms,
            volume: self.volume,
            mute: self.mute,
            queue: self.queue.clone(),
            error: self.error.clone(),
            updated_at_ms: now_ms(),
        }
    }

    fn publish(&self) {
        let _ = self.snap_tx.send(self.snapshot());
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Cmd>) {
        let mut events = self.heos.subscribe();
        let mut tick = interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => self.on_cmd(c).await,
                    None => return,
                },
                ev = events.recv() => match ev {
                    Ok(e) => self.on_event(e).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => warn!("player lagged, {n} HEOS events lost"),
                    Err(_) => return,
                },
                _ = tick.tick() => {
                    if let Some(t) = self.transition {
                        if t.elapsed() > TRANSITION_TIMEOUT {
                            self.transition = None;
                            self.fail("the Denon never started playing (timeout)".into());
                        } else { continue; }
                    } else { continue; }
                }
            }
            self.publish();
        }
    }

    fn fail(&mut self, msg: String) {
        warn!("player: {msg}");
        self.status = Status::Stopped;
        self.transition = None;
        self.error = Some(msg);
    }

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play { items, start } => {
                if items.is_empty() {
                    return;
                }
                self.queue = items;
                self.errors_in_row = 0;
                self.start_track(start.min(self.queue.len() - 1)).await;
            }
            Cmd::Jump(i) if i < self.queue.len() => {
                self.errors_in_row = 0;
                self.start_track(i).await
            }
            Cmd::Jump(_) => self.error = Some("index out of range".into()),
            Cmd::Enqueue(items) => {
                if self.queue.len() + items.len() <= MAX_QUEUE {
                    self.queue.extend(items);
                } else {
                    self.error = Some("queue is full".into());
                }
            }
            Cmd::Next => match self.index {
                Some(i) if i + 1 < self.queue.len() => {
                    self.errors_in_row = 0;
                    self.start_track(i + 1).await
                }
                _ => {}
            },
            Cmd::Prev => {
                if let Some(i) = self.index {
                    let target = if self.est_pos() > PREV_RESTART_AFTER_MS || i == 0 { i } else { i - 1 };
                    self.errors_in_row = 0;
                    self.start_track(target).await;
                }
            }
            Cmd::Pause => {
                if let Err(e) = self.heos.set_play_state("pause").await {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Cmd::Resume => {
                if let Err(e) = self.heos.set_play_state("play").await {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Cmd::Stop => {
                self.pos_ms = self.est_pos();
                self.status = Status::Stopped;
                self.transition = None;
                if let Err(e) = self.heos.set_play_state("stop").await {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Cmd::Volume(level) => match self.heos.set_volume(level).await {
                Ok(()) => self.volume = Some(level.min(100)),
                Err(e) => self.error = Some(format!("{e:#}")),
            },
            Cmd::Mute(on) => match self.heos.set_mute(on).await {
                Ok(()) => self.mute = on,
                Err(e) => self.error = Some(format!("{e:#}")),
            },
            Cmd::Seek(ms) => self.seek_to(ms).await,
            Cmd::SeekBy(delta) => {
                let target = (self.est_pos() as i64 + delta).max(0) as u64;
                self.seek_to(target).await
            }
        }
    }

    async fn start_track(&mut self, i: usize) {
        self.start_at(i, 0, false).await
    }

    /// Seek inside the current track. The Denon cannot seek in a streamed URL, so we restart the stream at the
    /// wanted frame (`/seek/{id}/{ms}.mp3`): a short silence (~0.4 s), like a track change.
    async fn seek_to(&mut self, ms: u64) {
        let Some(i) = self.index else { return };
        if self.status == Status::Stopped || self.queue.is_empty() {
            return;
        }
        // Stay clear of the very end: seeking past it would just skip to the next track.
        let target = if self.dur_ms > 0 { ms.min(self.dur_ms.saturating_sub(1500)) } else { ms };
        self.errors_in_row = 0;
        self.start_at(i, target, true).await
    }

    /// stop -> clear_queue -> play_stream. The stop/clear side effects are swallowed by `transition`.
    /// `offset_ms` > 0 starts inside the track; `keep_duration` keeps the known total (the Denon would report the
    /// duration of the cut stream).
    async fn start_at(&mut self, i: usize, offset_ms: u64, keep_duration: bool) {
        let item = self.queue[i].clone();
        // Under a second in is the beginning: use the plain stream.
        let offset_ms = if offset_ms < 1000 { 0 } else { offset_ms };
        self.index = Some(i);
        self.status = Status::Loading;
        self.offset_ms = offset_ms;
        self.pos_ms = offset_ms;
        self.pos_at = Instant::now();
        if !keep_duration {
            self.dur_ms = item.duration_ms.unwrap_or(0).max(0) as u64;
        }
        self.error = None;
        self.transition = Some(Instant::now());
        // The Denon remembers the title/artist/album it found for a URL: a URL it once played without reading the
        // tags keeps showing "Url Stream" for ever. A different query string makes every start a new URL (the
        // server ignores it), so the tags are read each time. Still plain ASCII.
        let url = if offset_ms == 0 {
            format!("{}/stream/{}.mp3?n={}", self.base_url, item.track_id, now_ms())
        } else {
            format!("{}/seek/{}/{}.mp3?n={}", self.base_url, item.track_id, offset_ms, now_ms())
        };
        info!("player: track {}/{} -> {} ({})", i + 1, self.queue.len(), item.title, url);
        let _ = self.heos.set_play_state("stop").await;
        let _ = self.heos.clear_queue().await; // fails harmlessly when already empty
        if let Err(e) = self.heos.play_url(&url).await {
            self.fail(format!("{e:#}"));
        }
    }

    async fn advance(&mut self) {
        match self.index.map(|i| i + 1) {
            Some(n) if n < self.queue.len() => self.start_track(n).await,
            _ => {
                info!("player: end of queue");
                self.status = Status::Stopped;
                self.pos_ms = 0;
            }
        }
    }

    async fn on_event(&mut self, ev: HeosEvent) {
        if let Some(pid) = ev.attrs.get("pid").and_then(|p| p.parse::<i64>().ok()) {
            if self.pid.is_none() {
                self.pid = self.heos.player_id().await.ok();
            }
            if self.pid.map(|mine| mine != pid).unwrap_or(false) {
                return; // another HEOS device
            }
        }
        match ev.name.as_str() {
            "connected" => {
                self.pid = None;
                if let Ok(v) = self.heos.get_volume().await {
                    self.volume = Some(v);
                }
                if let Ok(m) = self.heos.get_mute().await {
                    self.mute = m;
                }
            }
            "disconnected" => {
                if self.status != Status::Stopped {
                    self.fail("connection to the Denon lost".into());
                }
            }
            "player_state_changed" => match ev.attrs.get("state").map(String::as_str) {
                Some("play") => {
                    self.transition = None;
                    self.errors_in_row = 0;
                    if self.status != Status::Playing {
                        self.pos_at = Instant::now();
                    }
                    self.status = Status::Playing;
                }
                Some("pause") if self.transition.is_none() => {
                    self.pos_ms = self.est_pos();
                    self.status = Status::Paused;
                }
                Some("stop") if self.transition.is_none() && self.status != Status::Stopped => {
                    let est = self.est_pos();
                    if is_natural_end(self.dur_ms, est) {
                        info!("player: end of track (pos {est} ms / {} ms)", self.dur_ms);
                        self.advance().await;
                    } else {
                        info!("player: stopped from outside at {est} ms");
                        self.pos_ms = est;
                        self.status = Status::Stopped;
                    }
                }
                _ => {}
            },
            "player_now_playing_progress" => {
                if let Some(p) = ev.attrs.get("cur_pos").and_then(|v| v.parse::<u64>().ok()) {
                    self.pos_ms = self.offset_ms + p;
                    self.pos_at = Instant::now();
                }
                if let Some(d) = ev.attrs.get("duration").and_then(|v| v.parse::<u64>().ok()) {
                    // After a seek the Denon reports the length of the CUT stream: only trust it to fill a gap.
                    if d > 0 && (self.offset_ms == 0 || self.dur_ms == 0) {
                        // The streams we hand over have no VBR header when the index knows the exact length (the
                        // Denon would not show the tags otherwise): the Denon then only estimates it, so keep ours.
                        let exact = self
                            .index
                            .and_then(|i| self.queue.get(i))
                            .filter(|q| q.duration_exact)
                            .and_then(|q| q.duration_ms)
                            .filter(|ms| *ms > 0);
                        self.dur_ms = match exact {
                            Some(ms) if self.offset_ms == 0 => ms as u64,
                            _ => self.offset_ms + d,
                        };
                    }
                }
            }
            "player_playback_error" => {
                let msg = ev.attrs.get("error").cloned().unwrap_or_default();
                if self.status == Status::Stopped {
                    return;
                }
                // The Denon repeats the same error several times: handle it once per window.
                if self.last_error_at.map(|t| t.elapsed() < ERROR_DEDUPE).unwrap_or(false) {
                    return;
                }
                self.last_error_at = Some(Instant::now());
                self.errors_in_row += 1;
                warn!("player: playback error ({}/{MAX_ERRORS_IN_ROW}): {msg}", self.errors_in_row);
                if self.errors_in_row >= MAX_ERRORS_IN_ROW {
                    self.fail(format!("too many playback errors: {msg}"));
                } else {
                    self.error = Some(msg);
                    self.transition = None;
                    self.advance().await;
                }
            }
            "player_volume_changed" => {
                if let Some(l) = ev.attrs.get("level").and_then(|v| v.parse::<f64>().ok()) {
                    self.volume = Some(l as u8);
                }
                if let Some(m) = ev.attrs.get("mute") {
                    self.mute = m == "on";
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_end_detection() {
        // stop right at the end of a 200 s track
        assert!(is_natural_end(200_000, 199_000));
        assert!(is_natural_end(200_000, 200_000));
        // user stop in the middle
        assert!(!is_natural_end(200_000, 90_000));
        // unknown duration: cannot tell, not caused by us -> treated as an end
        assert!(is_natural_end(0, 5_000));
    }
}
