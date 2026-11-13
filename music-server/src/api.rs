use crate::player::{self, Cmd, PlayerHandle, QueueItem};
use crate::{covers, db, scan, search};
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, types::Value, Connection};
use serde::Deserialize;
use serde_json::{json, Value as Json_};
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::services::ServeFile;

#[derive(Clone)]
pub struct AppState {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub scan: scan::SharedScan,
    pub player: PlayerHandle,
    pub covers: std::sync::Arc<covers::CoverService>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/albums", get(albums))
        .route("/albums/{id}", get(album))
        .route("/artists", get(artists))
        .route("/tracks/{id}", get(track))
        .route("/search", get(search_handler))
        .route("/cover/{album_id}", get(cover))
        .route("/covers", get(covers_stats))
        .route("/stream/{file}", get(stream))
        .route("/seek/{id}/{file}", get(seek_stream))
        .route("/scan", get(scan_status).post(scan_start))
        .route("/player", get(player_state))
        .route("/player/events", get(player_events))
        .route("/player/play", post(player_play))
        .route("/player/jump", post(player_jump))
        .route("/player/next", post(player_next))
        .route("/player/previous", post(player_prev))
        .route("/player/pause", post(player_pause))
        .route("/player/resume", post(player_resume))
        .route("/player/stop", post(player_stop))
        .route("/player/volume", post(player_volume))
        .route("/player/mute", post(player_mute))
        .route("/player/seek", post(player_seek))
        .with_state(state)
}

// ---------- errors ----------

pub enum ApiError {
    NotFound,
    BadRequest(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, Json(json!({"error": m}))).into_response(),
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"}))).into_response()
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// Run a blocking DB closure off the async runtime.
async fn with_db<T: Send + 'static>(
    st: &AppState,
    f: impl FnOnce(&Connection) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let p = st.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db::connect(&p)?;
        f(&conn)
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?
}

// ---------- handlers ----------

async fn health() -> Json<Json_> {
    Json(json!({"status": "ok"}))
}

#[derive(Deserialize)]
struct AlbumsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    sort: Option<String>,
    q: Option<String>,
    artist: Option<String>,
}

fn album_json(r: &rusqlite::Row) -> rusqlite::Result<Json_> {
    let artist_count: i64 = r.get("artist_count")?;
    let has_cover = r.get::<_, i64>("has_cover")? != 0;
    Ok(json!({
        "id": r.get::<_, i64>("id")?,
        "name": r.get::<_, String>("name")?,
        "artist": r.get::<_, Option<String>>("artist")?,
        "various_artists": artist_count > 3,
        "year": r.get::<_, Option<i64>>("year")?,
        "track_count": r.get::<_, i64>("track_count")?,
        "has_cover": has_cover,
        "cover_path": if has_cover { Some(format!("/cover/{}", r.get::<_, i64>("id")?)) } else { None },
    }))
}

async fn albums(State(st): State<AppState>, Query(q): Query<AlbumsQuery>) -> ApiResult<Json<Json_>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let order = match q.sort.as_deref().unwrap_or("name") {
        "name" => "name COLLATE NOCASE, id",
        "artist" => "artist COLLATE NOCASE, name COLLATE NOCASE, id",
        "recent" => "added_at DESC, id DESC",
        "random" => "RANDOM()",
        other => return Err(ApiError::BadRequest(format!("unknown sort '{other}'"))),
    };
    with_db(&st, move |c| {
        let mut wh = vec!["1=1".to_string()];
        let mut args: Vec<Value> = vec![];
        if let Some(s) = q.q.filter(|s| !s.trim().is_empty()) {
            let like = format!("%{}%", s.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            wh.push("(name LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\' OR dir_name LIKE ? ESCAPE '\\')".into());
            args.extend([Value::Text(like.clone()), Value::Text(like.clone()), Value::Text(like)]);
        }
        if let Some(a) = q.artist {
            wh.push("artist = ? COLLATE NOCASE".into());
            args.push(Value::Text(a));
        }
        let where_sql = wh.join(" AND ");
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM albums WHERE {where_sql}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        args.push(Value::Integer(limit));
        args.push(Value::Integer(offset));
        let mut st = c.prepare(&format!(
            "SELECT albums.id AS id, name, artist, artist_count, year, track_count, (c.kind IS NOT NULL AND c.kind <> 'none') AS has_cover
             FROM albums LEFT JOIN album_covers c ON c.album_id = albums.id
             WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
        ))?;
        let items = st
            .query_map(rusqlite::params_from_iter(args.iter()), album_json)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Json(json!({"total": total, "limit": limit, "offset": offset, "items": items})))
    })
    .await
}

fn track_json(r: &rusqlite::Row) -> rusqlite::Result<Json_> {
    let id: i64 = r.get("id")?;
    Ok(json!({
        "id": id,
        "album_id": r.get::<_, i64>("album_id")?,
        "title": r.get::<_, String>("title")?,
        "artist": r.get::<_, Option<String>>("artist")?,
        "genre": r.get::<_, Option<String>>("genre")?,
        "track_no": r.get::<_, Option<i64>>("track_no")?,
        "disc_no": r.get::<_, Option<i64>>("disc_no")?,
        "year": r.get::<_, Option<i64>>("year")?,
        "duration_ms": r.get::<_, Option<i64>>("duration_ms")?,
        "stream_path": format!("/stream/{id}.mp3"),
    }))
}

const TRACK_SELECT: &str = "SELECT t.id, t.album_id, t.title, COALESCE(t.artist, a.artist) AS artist, t.genre, t.track_no,
    t.disc_no, t.year, t.duration_ms FROM tracks t JOIN albums a ON a.id = t.album_id";

async fn album(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Json_>> {
    with_db(&st, move |c| {
        let album = c
            .query_row(
                "SELECT albums.id AS id, name, artist, artist_count, year, track_count, (c.kind IS NOT NULL AND c.kind <> 'none') AS has_cover
                 FROM albums LEFT JOIN album_covers c ON c.album_id = albums.id WHERE albums.id=?1",
                params![id],
                album_json,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ApiError::NotFound,
                e => e.into(),
            })?;
        let mut st = c.prepare(&format!(
            "{TRACK_SELECT} WHERE t.album_id=?1
             ORDER BY COALESCE(t.disc_no,1), COALESCE(t.track_no,9999), t.path COLLATE NOCASE"
        ))?;
        let tracks = st.query_map(params![id], track_json)?.collect::<Result<Vec<_>, _>>()?;
        Ok(Json(json!({"album": album, "tracks": tracks})))
    })
    .await
}

async fn track(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Json_>> {
    with_db(&st, move |c| {
        c.query_row(&format!("{TRACK_SELECT} WHERE t.id=?1"), params![id], track_json)
            .map(|j| Json(j))
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ApiError::NotFound,
                e => e.into(),
            })
    })
    .await
}

async fn artists(State(st): State<AppState>) -> ApiResult<Json<Json_>> {
    with_db(&st, |c| {
        let mut st = c.prepare(
            "SELECT artist, COUNT(*) AS n FROM albums WHERE artist IS NOT NULL AND artist_count <= 3
             GROUP BY artist COLLATE NOCASE ORDER BY artist COLLATE NOCASE",
        )?;
        let items = st
            .query_map([], |r| Ok(json!({"name": r.get::<_, String>(0)?, "album_count": r.get::<_, i64>(1)?})))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Json(json!({"total": items.len(), "items": items})))
    })
    .await
}

/// Streams by numeric id so every URL given to the Denon is plain ASCII.
/// ServeFile handles Range, HEAD, Content-Length, Last-Modified and audio/mpeg.
async fn stream(State(st): State<AppState>, Path(file): Path<String>, req: Request<Body>) -> ApiResult<Response> {
    let id: i64 = file
        .strip_suffix(".mp3")
        .unwrap_or(&file)
        .parse()
        .map_err(|_| ApiError::NotFound)?;
    let rel: Option<String> = with_db(&st, move |c| {
        match c.query_row("SELECT path FROM tracks WHERE id=?1", params![id], |r| r.get::<_, String>(0)) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
    .await?;
    let rel = rel.ok_or(ApiError::NotFound)?;
    let res = ServeFile::new(st.root.join(rel))
        .oneshot(req)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("serve file: {e}")))?;
    Ok(res.into_response())
}

async fn scan_status(State(st): State<AppState>) -> Json<scan::ScanState> {
    Json(st.scan.lock().unwrap().clone())
}

async fn scan_start(State(st): State<AppState>) -> (StatusCode, Json<Json_>) {
    if scan::spawn(st.root.clone(), st.db_path.clone(), st.scan.clone()) {
        (StatusCode::ACCEPTED, Json(json!({"status": "started"})))
    } else {
        (StatusCode::CONFLICT, Json(json!({"status": "already running"})))
    }
}


// ---------- player ----------

async fn player_state(State(st): State<AppState>) -> Json<player::Snapshot> {
    Json(st.player.snapshot())
}

/// Server-sent events: the full player snapshot now, then on every change.
/// Position is only refreshed on events (~5 s): clients extrapolate from `updated_at_ms`.
async fn player_events(State(st): State<AppState>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use tokio_stream::{wrappers::WatchStream, StreamExt};
    let stream = WatchStream::new(st.player.watch())
        .map(|snap| Ok(Event::default().json_data(&snap).unwrap_or_else(|_| Event::default().data("{}"))));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct PlayBody {
    track_ids: Option<Vec<i64>>,
    track_id: Option<i64>,
    album_id: Option<i64>,
    start: Option<usize>,
}

const MAX_QUEUE: usize = 1000;

fn load_items(c: &Connection, ids: &[i64]) -> ApiResult<Vec<QueueItem>> {
    let mut st = c.prepare(&format!("{TRACK_SELECT} WHERE t.id=?1"))?;
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        match st.query_row(params![id], |r| {
            Ok(QueueItem {
                track_id: r.get("id")?,
                album_id: r.get("album_id")?,
                title: r.get("title")?,
                artist: r.get("artist")?,
                duration_ms: r.get("duration_ms")?,
            })
        }) {
            Ok(i) => items.push(i),
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(ApiError::BadRequest(format!("unknown track {id}"))),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(items)
}

async fn player_play(State(st): State<AppState>, Json(body): Json<PlayBody>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    let start = body.start.unwrap_or(0);
    let items = with_db(&st, move |c| {
        let ids: Vec<i64> = if let Some(ids) = body.track_ids {
            ids
        } else if let Some(id) = body.track_id {
            vec![id]
        } else if let Some(album) = body.album_id {
            let mut s = c.prepare(
                "SELECT id FROM tracks WHERE album_id=?1
                 ORDER BY COALESCE(disc_no,1), COALESCE(track_no,9999), path COLLATE NOCASE",
            )?;
            let v = s.query_map(params![album], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
            v
        } else {
            return Err(ApiError::BadRequest("give track_ids, track_id or album_id".into()));
        };
        if ids.is_empty() {
            return Err(ApiError::BadRequest("nothing to play".into()));
        }
        if ids.len() > MAX_QUEUE {
            return Err(ApiError::BadRequest(format!("queue too long (max {MAX_QUEUE})")));
        }
        load_items(c, &ids)
    })
    .await?;
    st.player.send(Cmd::Play { items, start }).await?;
    Ok((StatusCode::ACCEPTED, Json(st.player.snapshot())))
}

#[derive(Deserialize)]
struct JumpBody {
    index: usize,
}

#[derive(Deserialize)]
struct VolumeBody {
    level: u8,
}

async fn player_cmd(st: &AppState, cmd: Cmd) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    st.player.send(cmd).await?;
    Ok((StatusCode::ACCEPTED, Json(st.player.snapshot())))
}

async fn player_jump(State(st): State<AppState>, Json(b): Json<JumpBody>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Jump(b.index)).await
}
async fn player_next(State(st): State<AppState>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Next).await
}
async fn player_prev(State(st): State<AppState>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Prev).await
}
async fn player_pause(State(st): State<AppState>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Pause).await
}
async fn player_resume(State(st): State<AppState>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Resume).await
}
async fn player_stop(State(st): State<AppState>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Stop).await
}
async fn player_volume(State(st): State<AppState>, Json(b): Json<VolumeBody>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    if b.level > 100 {
        return Err(ApiError::BadRequest("level must be 0-100".into()));
    }
    player_cmd(&st, Cmd::Volume(b.level)).await
}

// ---------- covers ----------

#[derive(Deserialize)]
struct CoverQuery {
    size: Option<u32>,
}

/// Album thumbnail (JPEG). `size` in {64,150,300,600}, aspect ratio kept, never upscaled.
/// 404 when the album has no cover: clients show a placeholder.
async fn cover(
    State(st): State<AppState>,
    Path(album_id): Path<i64>,
    Query(q): Query<CoverQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
    let size = q.size.unwrap_or(covers::DEFAULT_SIZE);
    if !covers::SIZES.contains(&size) {
        return Err(ApiError::BadRequest(format!("size must be one of {:?}", covers::SIZES)));
    }
    let row = with_db(&st, move |c| {
        match c.query_row(
            "SELECT kind, source, source_mtime, source_size FROM album_covers WHERE album_id=?1",
            params![album_id],
            |r| Ok(covers::CoverRow { kind: r.get(0)?, source: r.get(1)?, source_mtime: r.get(2)?, source_size: r.get(3)? }),
        ) {
            Ok(row) if row.kind != "none" => Ok(Some(row)),
            Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
    .await?
    .ok_or(ApiError::NotFound)?;

    let etag = format!("\"{}\"", covers::cache_key(&row, size));
    let cache_headers = [(CACHE_CONTROL, "public, max-age=86400".to_string()), (ETAG, etag.clone())];
    if headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return Ok((StatusCode::NOT_MODIFIED, cache_headers).into_response());
    }
    let (bytes, _) = match st.covers.thumb(album_id, &row, size).await {
        Ok(r) => r,
        Err(e) => {
            // A picture that cannot be decoded will never work: stop advertising it. IO errors (disk
            // unmounted...) are transient and must NOT demote the cover; a changed album is re-resolved by the next scan.
            if e.downcast_ref::<image::ImageError>().is_some() {
                let source = row.source.clone();
                let _ = with_db(&st, move |c| {
                    c.execute("UPDATE album_covers SET kind='none', source=NULL WHERE album_id=?1 AND source IS ?2", params![album_id, source])?;
                    Ok(())
                })
                .await;
            }
            return Err(ApiError::NotFound);
        }
    };
    Ok(([(CONTENT_TYPE, "image/jpeg".to_string())], cache_headers, bytes).into_response())
}

async fn covers_stats(State(st): State<AppState>) -> ApiResult<Json<covers::CoverStats>> {
    let resolved = st.scan.lock().unwrap().covers_resolved;
    with_db(&st, move |c| Ok(Json(covers::stats(c, resolved)?))).await
}

// ---------- search ----------

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
    /// Comma separated subset of `albums,artists,tracks` (default: all three).
    types: Option<String>,
}

/// Full-text search, accent-insensitive, every word is a prefix ("bea hel" finds "The Beatles - Help!").
/// Results are ranked by relevance (title weighs more than artist, artist more than album).
async fn search_handler(State(st): State<AppState>, Query(q): Query<SearchQuery>) -> ApiResult<Json<Json_>> {
    let text = q.q.unwrap_or_default();
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    let types = q.types.unwrap_or_else(|| "albums,artists,tracks".into());
    let want = |t: &str| types.split(',').any(|x| x.trim() == t);
    for t in types.split(',') {
        if !["albums", "artists", "tracks"].contains(&t.trim()) {
            return Err(ApiError::BadRequest(format!("unknown type '{t}' (albums, artists, tracks)")));
        }
    }
    let Some(fts) = search::match_query(&text) else {
        return Ok(Json(json!({"query": text, "albums": [], "artists": [], "tracks": []})));
    };
    let (want_albums, want_artists, want_tracks) = (want("albums"), want("artists"), want("tracks"));
    with_db(&st, move |c| {
        let albums: Vec<Json_> = if want_albums {
            let mut s = c.prepare(
                "SELECT albums.id AS id, albums.name AS name, albums.artist AS artist, artist_count, year, track_count,
                        (cv.kind IS NOT NULL AND cv.kind <> 'none') AS has_cover
                 FROM fts_albums
                 JOIN albums ON albums.id = fts_albums.album_id
                 LEFT JOIN album_covers cv ON cv.album_id = albums.id
                 WHERE fts_albums MATCH ?1
                 ORDER BY bm25(fts_albums, 10.0, 5.0, 3.0, 0.0), albums.id LIMIT ?2",
            )?;
            let v = s.query_map(params![fts, limit], album_json)?.collect::<Result<Vec<_>, _>>()?;
            v
        } else {
            vec![]
        };
        let artists: Vec<Json_> = if want_artists {
            let mut s = c.prepare(
                "SELECT fts_artists.name,
                        (SELECT COUNT(*) FROM albums a WHERE a.artist = fts_artists.name COLLATE NOCASE) AS n
                 FROM fts_artists WHERE fts_artists MATCH ?1 ORDER BY bm25(fts_artists), fts_artists.name LIMIT ?2",
            )?;
            let v = s
                .query_map(params![fts, limit], |r| Ok(json!({"name": r.get::<_, String>(0)?, "album_count": r.get::<_, i64>(1)?})))?
                .collect::<Result<Vec<_>, _>>()?;
            v
        } else {
            vec![]
        };
        let tracks: Vec<Json_> = if want_tracks {
            let mut s = c.prepare(
                "SELECT t.id, t.album_id, t.title, COALESCE(t.artist, a.artist) AS artist, t.genre, t.track_no,
                        t.disc_no, t.year, t.duration_ms, a.name AS album_name
                 FROM fts_tracks
                 JOIN tracks t ON t.id = fts_tracks.track_id
                 JOIN albums a ON a.id = t.album_id
                 WHERE fts_tracks MATCH ?1
                 ORDER BY bm25(fts_tracks, 10.0, 4.0, 2.0, 0.0, 0.0), t.id LIMIT ?2",
            )?;
            let v = s
                .query_map(params![fts, limit], |r| {
                    let mut j = track_json(r)?;
                    j["album_name"] = json!(r.get::<_, String>("album_name")?);
                    Ok(j)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            v
        } else {
            vec![]
        };
        Ok(Json(json!({"query": text, "albums": albums, "artists": artists, "tracks": tracks})))
    })
    .await
}

#[derive(Deserialize)]
struct MuteBody {
    mute: bool,
}

async fn player_mute(State(st): State<AppState>, Json(b): Json<MuteBody>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    player_cmd(&st, Cmd::Mute(b.mute)).await
}

#[derive(Deserialize)]
struct SeekBody {
    /// Absolute position in the current track, milliseconds.
    position_ms: Option<u64>,
    /// Relative move, milliseconds (negative = backwards). Ignored when `position_ms` is given.
    delta_ms: Option<i64>,
}

/// Move inside the current track. The stream is restarted at the wanted frame: expect ~0.4 s of silence.
async fn player_seek(State(st): State<AppState>, Json(b): Json<SeekBody>) -> ApiResult<(StatusCode, Json<player::Snapshot>)> {
    let cmd = match (b.position_ms, b.delta_ms) {
        (Some(p), _) => Cmd::Seek(p),
        (None, Some(d)) => Cmd::SeekBy(d),
        _ => return Err(ApiError::BadRequest("give position_ms or delta_ms".into())),
    };
    player_cmd(&st, cmd).await
}

/// The track from a given time: `/seek/{id}/{ms}.mp3`. The body starts at the MP3 frame at (or just after) `ms`.
/// ASCII-only path on purpose: the Denon does not decode escapes, `?` or `=` in the URL it is given.
async fn seek_stream(State(st): State<AppState>, Path((id, file)): Path<(i64, String)>) -> ApiResult<Response> {
    use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_TYPE};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let ms: u64 = file.strip_suffix(".mp3").unwrap_or(&file).parse().map_err(|_| ApiError::NotFound)?;
    let rel: Option<String> = with_db(&st, move |c| {
        match c.query_row("SELECT path FROM tracks WHERE id=?1", params![id], |r| r.get::<_, String>(0)) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
    .await?;
    let full = st.root.join(rel.ok_or(ApiError::NotFound)?);
    let path = full.clone();
    let point = tokio::task::spawn_blocking(move || crate::mp3seek::find_offset(&path, ms))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .map_err(|e| ApiError::Internal(e.into()))?
        .ok_or_else(|| ApiError::BadRequest("not an MPEG layer III file that can be cut".into()))?;
    let mut f = tokio::fs::File::open(&full).await.map_err(|e| ApiError::Internal(e.into()))?;
    let len = f.metadata().await.map_err(|e| ApiError::Internal(e.into()))?.len();
    let remaining = len.saturating_sub(point.byte_offset);
    f.seek(std::io::SeekFrom::Start(point.byte_offset)).await.map_err(|e| ApiError::Internal(e.into()))?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(f.take(remaining)));
    tracing::info!("seek stream: track {id} from {ms} ms -> frame at {} ms, byte {}", point.actual_ms, point.byte_offset);
    Ok(Response::builder()
        .header(CONTENT_TYPE, "audio/mpeg")
        .header(CONTENT_LENGTH, remaining)
        .header(ACCEPT_RANGES, "none")
        .body(body)
        .map_err(|e| ApiError::Internal(e.into()))?)
}
