mod api;
mod covers;
mod db;
mod heos;
mod lastfm;
mod mdns;
mod mp3seek;
mod online;
mod player;
mod recommend;
mod scan;
mod scrobble;
mod search;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::info;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,lofty=error".into()))
        .init();

    let root = PathBuf::from(env_or("MUSIC_ROOT", "/mnt/music/music"));
    let home = env_or("HOME", ".");
    let db_path = PathBuf::from(env_or("MUSIC_DB", &format!("{home}/.local/share/music-server/index.db")));
    let bind = env_or("MUSIC_BIND", "0.0.0.0:8765");

    db::open(&db_path)?; // create schema before serving
    info!("root={} db={} bind={}", root.display(), db_path.display(), bind);

    let scan_state: scan::SharedScan = Arc::new(Mutex::new(scan::ScanState::default()));
    if std::env::var("MUSIC_NO_SCAN").is_err() {
        scan::spawn(root.clone(), db_path.clone(), scan_state.clone()); // background: API answers right away
    }

    // HEOS: fixed Denon IP via HEOS_HOST, otherwise SSDP discovery. The URL given to the Denon
    // must be reachable by it, so it comes from config (hostname -I also lists docker bridges).
    let heos_host = std::env::var("HEOS_HOST").ok().filter(|h| !h.is_empty());
    let heos_player = std::env::var("HEOS_PLAYER_NAME").ok().filter(|h| !h.is_empty());
    let public_url = env_or("MUSIC_PUBLIC_URL", "http://192.168.1.50:8765");
    let heos = heos::HeosClient::spawn(heos_host, heos_player);
    let player = player::spawn(heos, public_url.trim_end_matches('/').to_string());

    let cache_dir = PathBuf::from(env_or("MUSIC_CACHE_DIR", &format!("{home}/.cache/music-server")));
    let cover_service = covers::CoverService::new(root.clone(), cache_dir.join("covers"));

    // Optional, OFF by default: sends artist/album names to MusicBrainz / Cover Art Archive.
    if std::env::var("COVERS_ONLINE").map(|v| v == "1").unwrap_or(false) {
        tokio::spawn(online::run_loop(online::OnlineConfig {
            db_path: db_path.clone(),
            cache_dir: cache_dir.join("covers"),
            contact: std::env::var("MUSICBRAINZ_CONTACT").ok().filter(|c| !c.is_empty()),
            first_delay: std::time::Duration::from_secs(env_or("COVERS_ONLINE_DELAY_S", "60").parse().unwrap_or(60)),
            interval: std::time::Duration::from_secs(6 * 3600),
        }));
    }

    // mDNS announcement so the app can find the server. A failure here must not stop the server.
    let advert = if env_or("MUSIC_MDNS", "1") != "0" {
        let port = bind.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(8765);
        match mdns::start(&public_url, port, &env_or("MUSIC_MDNS_NAME", "Music Server")) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("mDNS disabled: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Optional, OFF without a key. LASTFM_API_KEY: similarity (sends the artist/title of the track being played).
    // LASTFM_SECRET + an account linked through /lastfm/auth/start: scrobbling (sends what the Denon plays).
    // LASTFM_USER: account whose listening history lifts the suggestions when none is linked.
    let data_dir = db_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let lastfm = Arc::new(lastfm::Client::new(
        std::env::var("LASTFM_API_KEY").ok(),
        std::env::var("LASTFM_SECRET").ok(),
        std::env::var("LASTFM_USER").ok(),
        data_dir.join("lastfm_session.json"),
    ));
    scrobble::spawn(lastfm.clone(), db_path.clone(), player.watch());
    let recommender = Arc::new(recommend::Recommender::new(lastfm.clone(), db_path.clone()));

    let app = api::router(api::AppState { root, db_path, scan: scan_state, player, covers: cover_service, recommend: recommender, lastfm });
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    if let Some(a) = advert {
        a.stop();
    }
    Ok(())
}
