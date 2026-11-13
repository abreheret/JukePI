//! Last.fm client shared by the recommendations (read-only, API key) and the scrobbler (write: key + shared secret +
//! a session linked to the user's account).
//!
//! Secrets: the key and the shared secret come from the environment (systemd drop-in, never from the repository).
//! The session key obtained through the browser authorisation is stored in a file readable by the service user only.
//! None of them is ever logged: reqwest errors embed their URL, so they are stripped with `without_url`.
//! The user's Last.fm password is never needed: the account is linked on last.fm/api/auth.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

const API: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH_URL: &str = "https://www.last.fm/api/auth/";

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    key: String,
    user: String,
}

pub struct Client {
    key: Option<String>,
    secret: Option<String>,
    /// Account name given in the configuration, used to read the listening history when no session is linked.
    user_hint: Option<String>,
    http: reqwest::Client,
    session_path: PathBuf,
    session: Mutex<Option<Session>>,
    /// Token waiting for the user to authorise it in the browser.
    pending: Mutex<Option<String>>,
}

/// `api_sig`: md5 of the parameters sorted by name, written `name` + `value` one after the other, followed by the
/// shared secret. `format` and `callback` are not signed.
pub fn sign(params: &[(&str, &str)], secret: &str) -> String {
    let mut p: Vec<&(&str, &str)> = params.iter().filter(|(k, _)| *k != "format" && *k != "callback").collect();
    p.sort_by(|a, b| a.0.cmp(b.0));
    let mut s = String::new();
    for (k, v) in p {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    format!("{:x}", md5::compute(s.as_bytes()))
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Client {
    pub fn new(key: Option<String>, secret: Option<String>, user_hint: Option<String>, session_path: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(format!("music-server/{} (self-hosted)", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        let session = std::fs::read_to_string(&session_path).ok().and_then(|t| serde_json::from_str::<Session>(&t).ok());
        let c = Self {
            key: clean(key),
            secret: clean(secret),
            user_hint: clean(user_hint),
            http,
            session_path,
            session: Mutex::new(session),
            pending: Mutex::new(None),
        };
        match (c.enabled(), c.can_scrobble()) {
            (false, _) => info!("Last.fm disabled (no LASTFM_API_KEY): recommendations use the local tags only"),
            (true, false) => info!("Last.fm similarity enabled: artist/title of the seed track are sent to ws.audioscrobbler.com; scrobbling is off"),
            (true, true) => info!(
                "Last.fm similarity and scrobbling enabled for account '{}': what the Denon plays is sent to ws.audioscrobbler.com",
                c.user().unwrap_or_default()
            ),
        }
        c
    }

    /// A key is configured: similarity lookups are possible.
    pub fn enabled(&self) -> bool {
        self.key.is_some()
    }

    pub fn has_secret(&self) -> bool {
        self.secret.is_some()
    }

    /// Key, secret and a linked account: listens can be sent.
    pub fn can_scrobble(&self) -> bool {
        self.key.is_some() && self.secret.is_some() && self.session.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    pub fn auth_pending(&self) -> bool {
        self.pending.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// The linked account, or the one named in `LASTFM_USER`.
    pub fn user(&self) -> Option<String> {
        self.session.lock().ok().and_then(|s| s.as_ref().map(|s| s.user.clone())).or_else(|| self.user_hint.clone())
    }

    fn check(status: reqwest::StatusCode, body: Value) -> Result<Value> {
        if let Some(code) = body.get("error") {
            return Err(anyhow!("Last.fm error {code}: {}", body["message"].as_str().unwrap_or("")));
        }
        if !status.is_success() {
            return Err(anyhow!("Last.fm HTTP {status}"));
        }
        Ok(body)
    }

    /// Read-only call, signed by the key alone (similar tracks, top artists...).
    pub async fn get(&self, method: &str, extra: &[(&str, &str)]) -> Result<Value> {
        let key = self.key.as_deref().ok_or_else(|| anyhow!("no Last.fm key"))?;
        let mut q: Vec<(&str, &str)> = vec![("method", method), ("api_key", key), ("format", "json"), ("autocorrect", "1")];
        q.extend_from_slice(extra);
        let resp = self.http.get(API).query(&q).send().await.map_err(|e| anyhow!("Last.fm request failed: {}", e.without_url()))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| anyhow!("Last.fm answer unreadable (HTTP {status}): {}", e.without_url()))?;
        Self::check(status, body)
    }

    /// Signed POST (auth, scrobbles, now playing). `with_session` adds the linked account's session key.
    pub async fn post_signed(&self, method: &str, extra: &[(String, String)], with_session: bool) -> Result<Value> {
        let (Some(key), Some(secret)) = (self.key.as_deref(), self.secret.as_deref()) else {
            return Err(anyhow!("Last.fm key and secret are required"));
        };
        let sk = if with_session {
            Some(self.session.lock().map_err(|_| anyhow!("session lock"))?.as_ref().ok_or_else(|| anyhow!("no Last.fm account linked"))?.key.clone())
        } else {
            None
        };
        let mut params: Vec<(String, String)> = vec![("method".into(), method.into()), ("api_key".into(), key.into())];
        if let Some(sk) = sk {
            params.push(("sk".into(), sk));
        }
        params.extend(extra.iter().cloned());
        let sig = {
            let refs: Vec<(&str, &str)> = params.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            sign(&refs, secret)
        };
        params.push(("api_sig".into(), sig));
        params.push(("format".into(), "json".into()));
        let resp = self.http.post(API).form(&params).send().await.map_err(|e| anyhow!("Last.fm request failed: {}", e.without_url()))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| anyhow!("Last.fm answer unreadable (HTTP {status}): {}", e.without_url()))?;
        Self::check(status, body)
    }

    /// Step 1 of linking the account: returns the page the user must open to authorise this server.
    pub async fn auth_start(&self) -> Result<String> {
        if !self.enabled() || !self.has_secret() {
            return Err(anyhow!("LASTFM_API_KEY and LASTFM_SECRET must both be set"));
        }
        if self.session.lock().map(|s| s.is_some()).unwrap_or(false) {
            return Err(anyhow!("an account is already linked (DELETE /lastfm/session to unlink it)"));
        }
        let v = self.post_signed("auth.getToken", &[], false).await?;
        let token = v["token"].as_str().ok_or_else(|| anyhow!("Last.fm gave no token"))?.to_string();
        let url = format!("{AUTH_URL}?api_key={}&token={token}", self.key.as_deref().unwrap_or_default());
        *self.pending.lock().map_err(|_| anyhow!("lock"))? = Some(token);
        Ok(url)
    }

    /// Step 2, after the user clicked "Allow": trades the token for a session and stores it.
    pub async fn auth_finish(&self) -> Result<String> {
        let token = self.pending.lock().map_err(|_| anyhow!("lock"))?.clone().ok_or_else(|| anyhow!("call /lastfm/auth/start first"))?;
        let v = self.post_signed("auth.getSession", &[("token".into(), token)], false).await?;
        let (Some(key), Some(user)) = (v["session"]["key"].as_str(), v["session"]["name"].as_str()) else {
            return Err(anyhow!("Last.fm gave no session"));
        };
        let session = Session { key: key.to_string(), user: user.to_string() };
        self.save(&session)?;
        *self.session.lock().map_err(|_| anyhow!("lock"))? = Some(session);
        *self.pending.lock().map_err(|_| anyhow!("lock"))? = None;
        info!("Last.fm account linked: {user}");
        Ok(user.to_string())
    }

    fn save(&self, s: &Session) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = self.session_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&self.session_path)?;
        f.write_all(serde_json::to_string(s)?.as_bytes())?;
        Ok(())
    }

    /// Forget the linked account (the session file is deleted).
    pub fn unlink(&self) {
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
        if let Err(e) = std::fs::remove_file(&self.session_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("could not delete the Last.fm session file: {e}");
            }
        }
        info!("Last.fm account unlinked");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_sorts_names_and_skips_format() {
        // md5("api_keyxxmethodauth.getTokenSECRET"): names sorted, `format` ignored, secret appended.
        let a = sign(&[("method", "auth.getToken"), ("format", "json"), ("api_key", "xx")], "SECRET");
        let expected = format!("{:x}", md5::compute(b"api_keyxxmethodauth.getTokenSECRET"));
        assert_eq!(a, expected);
        assert_eq!(a.len(), 32);
        let b = sign(&[("api_key", "xx"), ("method", "auth.getToken")], "SECRET");
        assert_eq!(a, b, "order of the parameters does not matter");
    }

    #[test]
    fn md5_known_vector() {
        assert_eq!(format!("{:x}", md5::compute(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn session_file_roundtrip_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("jukepi-test-{}", std::process::id()));
        let path = dir.join("s.json");
        let c = Client::new(Some("k".into()), Some("s".into()), None, path.clone());
        assert!(!c.can_scrobble());
        c.save(&Session { key: "sk".into(), user: "someone".into() }).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let c2 = Client::new(Some("k".into()), Some("s".into()), None, path.clone());
        assert!(c2.can_scrobble());
        assert_eq!(c2.user().as_deref(), Some("someone"));
        c2.unlink();
        assert!(!c2.can_scrobble() && !path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
