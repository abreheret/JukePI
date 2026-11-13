//! HEOS CLI client (Denon/Marantz): TCP text protocol on port 1255.
//!
//! One background task owns the TCP connection. Commands are sent one at a time
//! (HEOS answers in order, with an optional interim "command under process"),
//! unsolicited `event/...` lines are fanned out on a broadcast channel.
//! The task reconnects by itself and re-discovers the Denon via SSDP when no host is configured.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{sleep, sleep_until, timeout, Instant};
use tracing::{debug, info, warn};

pub const HEOS_PORT: u16 = 1255;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// An unsolicited HEOS event, e.g. name `player_state_changed`, attrs `{pid, state}`.
/// The client also emits two synthetic events: `connected` and `disconnected`.
#[derive(Clone, Debug)]
pub struct HeosEvent {
    pub name: String,
    pub attrs: HashMap<String, String>,
}

struct Request {
    name: String,
    line: String,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Clone)]
pub struct HeosClient {
    tx: mpsc::Sender<Request>,
    events: broadcast::Sender<HeosEvent>,
    connected: Arc<AtomicBool>,
    pid: Arc<Mutex<Option<i64>>>,
    player_name: Option<String>,
}

/// `pid=1&state=play` -> {pid: 1, state: play}. Tokens without `=` (e.g. "command under process") are skipped.
pub fn parse_attrs(msg: &str) -> HashMap<String, String> {
    msg.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

impl HeosClient {
    /// `host`: fixed Denon IP, or None to discover it with SSDP.
    pub fn spawn(host: Option<String>, player_name: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel(32);
        let (events, _) = broadcast::channel(256);
        let connected = Arc::new(AtomicBool::new(false));
        let client = HeosClient { tx, events: events.clone(), connected: connected.clone(), pid: Arc::new(Mutex::new(None)), player_name };
        tokio::spawn(connection_loop(host, HEOS_PORT, rx, events, connected));
        client
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HeosEvent> {
        self.events.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Send `heos://{path}?k=v&...` and return the full JSON reply once `result == success`.
    /// Attribute values must already be safe for the protocol (our URLs are plain ASCII).
    pub async fn command(&self, path: &str, attrs: &[(&str, String)]) -> Result<Value> {
        if !self.is_connected() {
            bail!("HEOS not connected");
        }
        let mut line = format!("heos://{path}");
        for (i, (k, v)) in attrs.iter().enumerate() {
            line.push(if i == 0 { '?' } else { '&' });
            line.push_str(&format!("{k}={v}"));
        }
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request { name: path.to_string(), line, reply })
            .await
            .map_err(|_| anyhow!("HEOS task stopped"))?;
        timeout(REQUEST_TIMEOUT + Duration::from_secs(1), rx)
            .await
            .map_err(|_| anyhow!("{path}: timeout"))?
            .map_err(|_| anyhow!("{path}: connection lost"))?
    }

    /// Player id (cached). Picks `HEOS_PLAYER_NAME` if set, otherwise the first player.
    pub async fn player_id(&self) -> Result<i64> {
        if let Some(p) = *self.pid.lock().unwrap() {
            return Ok(p);
        }
        let r = self.command("player/get_players", &[]).await?;
        let players = r["payload"].as_array().ok_or_else(|| anyhow!("get_players: no payload"))?;
        let chosen = match &self.player_name {
            Some(n) => players.iter().find(|p| p["name"].as_str().map(|s| s.contains(n.as_str())).unwrap_or(false)),
            None => players.first(),
        }
        .ok_or_else(|| anyhow!("no matching HEOS player"))?;
        let pid = chosen["pid"].as_i64().ok_or_else(|| anyhow!("player without pid"))?;
        info!("HEOS player: {} (pid {pid})", chosen["name"].as_str().unwrap_or("?"));
        *self.pid.lock().unwrap() = Some(pid);
        Ok(pid)
    }

    async fn pcmd(&self, path: &str, extra: &[(&str, String)]) -> Result<Value> {
        let pid = self.player_id().await?;
        let mut attrs = vec![("pid", pid.to_string())];
        attrs.extend(extra.iter().cloned());
        self.command(path, &attrs).await
    }

    pub async fn play_url(&self, url: &str) -> Result<()> {
        self.pcmd("browse/play_stream", &[("url", url.to_string())]).await.map(|_| ())
    }
    pub async fn set_play_state(&self, state: &str) -> Result<()> {
        self.pcmd("player/set_play_state", &[("state", state.to_string())]).await.map(|_| ())
    }
    pub async fn clear_queue(&self) -> Result<()> {
        self.pcmd("player/clear_queue", &[]).await.map(|_| ())
    }
    pub async fn set_volume(&self, level: u8) -> Result<()> {
        self.pcmd("player/set_volume", &[("level", level.min(100).to_string())]).await.map(|_| ())
    }
    pub async fn set_mute(&self, on: bool) -> Result<()> {
        self.pcmd("player/set_mute", &[("state", if on { "on" } else { "off" }.to_string())]).await.map(|_| ())
    }
    pub async fn get_mute(&self) -> Result<bool> {
        let r = self.pcmd("player/get_mute", &[]).await?;
        Ok(parse_attrs(r["heos"]["message"].as_str().unwrap_or("")).get("state").map(String::as_str) == Some("on"))
    }
    pub async fn get_volume(&self) -> Result<u8> {
        let r = self.pcmd("player/get_volume", &[]).await?;
        let level = parse_attrs(r["heos"]["message"].as_str().unwrap_or(""))
            .get("level")
            .and_then(|l| l.parse().ok())
            .ok_or_else(|| anyhow!("get_volume: no level"))?;
        Ok(level)
    }
}

/// Find the Denon with an SSDP M-SEARCH for the HEOS device type; returns the responder's IP.
pub async fn discover() -> Result<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let msg = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-denon-com:device:ACT-Denon:1\r\n\r\n";
    for _ in 0..3 {
        sock.send_to(msg.as_bytes(), "239.255.255.250:1900").await?;
        let mut buf = [0u8; 2048];
        if let Ok(Ok((n, from))) = timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await {
            let text = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            if text.contains("denon") || text.contains("heos") {
                return Ok(from.ip());
            }
        }
    }
    bail!("no HEOS device answered the SSDP search")
}

async fn connection_loop(
    host: Option<String>,
    port: u16,
    mut rx: mpsc::Receiver<Request>,
    events: broadcast::Sender<HeosEvent>,
    connected: Arc<AtomicBool>,
) {
    let mut backoff = 2u64;
    loop {
        let target = match &host {
            Some(h) => Ok(h.clone()),
            None => discover().await.map(|ip| ip.to_string()),
        };
        match target {
            Ok(h) => match session(&h, port, &mut rx, &events, &connected).await {
                Ok(()) => return, // all senders dropped: shutting down
                Err(e) => {
                    warn!("HEOS session ended: {e:#}");
                    backoff = 2;
                }
            },
            Err(e) => warn!("HEOS discovery failed: {e:#}"),
        }
        if connected.swap(false, Ordering::Relaxed) {
            let _ = events.send(HeosEvent { name: "disconnected".into(), attrs: HashMap::new() });
        }
        sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(15);
    }
}

async fn session(
    host: &str,
    port: u16,
    rx: &mut mpsc::Receiver<Request>,
    events: &broadcast::Sender<HeosEvent>,
    connected: &Arc<AtomicBool>,
) -> Result<()> {
    // Requests queued while we were offline are stale (they were failed fast); drop them.
    while let Ok(stale) = rx.try_recv() {
        let _ = stale.reply.send(Err(anyhow!("HEOS not connected")));
    }
    let stream = timeout(Duration::from_secs(5), TcpStream::connect((host, port))).await??;
    stream.set_nodelay(true)?;
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    wr.write_all(b"heos://system/register_for_change_events?enable=on\r\n").await?;
    info!("HEOS connected to {host}:{port}");
    connected.store(true, Ordering::Relaxed);
    let _ = events.send(HeosEvent { name: "connected".into(), attrs: HashMap::new() });

    let mut queue: VecDeque<Request> = VecDeque::new();
    let mut pending: Option<(String, oneshot::Sender<Result<Value>>, Instant)> = None;

    let result: Result<()> = loop {
        if pending.is_none() {
            if let Some(r) = queue.pop_front() {
                debug!("HEOS >> {}", r.line);
                if let Err(e) = wr.write_all(format!("{}\r\n", r.line).as_bytes()).await {
                    let _ = r.reply.send(Err(anyhow!("write failed: {e}")));
                    break Err(e.into());
                }
                pending = Some((r.name, r.reply, Instant::now() + REQUEST_TIMEOUT));
            }
        }
        let deadline = pending.as_ref().map(|p| p.2);
        tokio::select! {
            line = lines.next_line() => match line {
                Err(e) => break Err(e.into()),
                Ok(None) => break Err(anyhow!("connection closed by the player")),
                Ok(Some(l)) => handle_line(&l, events, &mut pending),
            },
            r = rx.recv() => match r {
                Some(r) => queue.push_back(r),
                None => break Ok(()),
            },
            _ = async { match deadline { Some(d) => sleep_until(d).await, None => std::future::pending().await } } => {
                if let Some((name, reply, _)) = pending.take() {
                    warn!("HEOS: no reply to {name}");
                    let _ = reply.send(Err(anyhow!("{name}: timeout")));
                }
            }
        }
    };
    connected.store(false, Ordering::Relaxed);
    let _ = events.send(HeosEvent { name: "disconnected".into(), attrs: HashMap::new() });
    if let Some((_, reply, _)) = pending.take() {
        let _ = reply.send(Err(anyhow!("connection lost")));
    }
    for r in queue {
        let _ = r.reply.send(Err(anyhow!("connection lost")));
    }
    result
}

fn handle_line(line: &str, events: &broadcast::Sender<HeosEvent>, pending: &mut Option<(String, oneshot::Sender<Result<Value>>, Instant)>) {
    let Ok(json) = serde_json::from_str::<Value>(line) else {
        debug!("HEOS: non-JSON line ignored");
        return;
    };
    let heos = &json["heos"];
    let cmd = heos["command"].as_str().unwrap_or("");
    let message = heos["message"].as_str().unwrap_or("");
    if let Some(ev) = cmd.strip_prefix("event/") {
        debug!("HEOS << event {ev} {message}");
        let _ = events.send(HeosEvent { name: ev.to_string(), attrs: parse_attrs(message) });
        return;
    }
    if message.contains("command under process") {
        return; // interim reply, the final one follows
    }
    if pending.as_ref().map(|p| p.0 == cmd).unwrap_or(false) {
        let (_, reply, _) = pending.take().unwrap();
        let res = if heos["result"].as_str() == Some("success") {
            Ok(json)
        } else {
            Err(anyhow!("{cmd}: {message}"))
        };
        let _ = reply.send(res);
    } else {
        debug!("HEOS: unmatched reply {cmd}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn attrs_parse() {
        let a = parse_attrs("command under process&pid=7&state=play");
        assert_eq!(a.get("pid").map(String::as_str), Some("7"));
        assert_eq!(a.get("state").map(String::as_str), Some("play"));
        assert_eq!(a.len(), 2);
    }

    /// Mock Denon: checks interim replies, error replies, ordering and event fan-out.
    #[tokio::test]
    async fn client_against_mock_denon() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                for l in String::from_utf8_lossy(&buf[..n]).lines().filter(|l| !l.is_empty()) {
                    let out = if l.contains("get_players") {
                        "{\"heos\":{\"command\":\"player/get_players\",\"result\":\"success\",\"message\":\"command under process\"}}\r\n\
                         {\"heos\":{\"command\":\"player/get_players\",\"result\":\"success\",\"message\":\"\"},\"payload\":[{\"name\":\"Denon X\",\"pid\":42}]}\r\n".to_string()
                    } else if l.contains("play_stream") {
                        "{\"heos\":{\"command\":\"browse/play_stream\",\"result\":\"fail\",\"message\":\"eid=4&text=nope\"}}\r\n\
                         {\"heos\":{\"command\":\"event/player_state_changed\",\"message\":\"pid=42&state=play\"}}\r\n".to_string()
                    } else {
                        continue;
                    };
                    s.write_all(out.as_bytes()).await.unwrap();
                }
            }
        });
        let (tx, rx) = mpsc::channel(8);
        let (events, mut ev_rx) = broadcast::channel(16);
        let connected = Arc::new(AtomicBool::new(false));
        tokio::spawn(connection_loop(Some("127.0.0.1".into()), port, rx, events.clone(), connected.clone()));
        let c = HeosClient { tx, events, connected, pid: Arc::new(Mutex::new(None)), player_name: None };
        // wait for the connection
        for _ in 0..50 {
            if c.is_connected() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
        assert!(c.is_connected());
        assert_eq!(c.player_id().await.unwrap(), 42);
        let err = c.play_url("http://x/y.mp3").await.unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
        let mut saw_state = false;
        while let Ok(e) = ev_rx.try_recv() {
            if e.name == "player_state_changed" {
                assert_eq!(e.attrs["state"], "play");
                saw_state = true;
            }
        }
        assert!(saw_state);
    }
}
