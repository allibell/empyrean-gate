//! HTTP + WebSocket server: serves the built web UI (embedded in the binary) and the
//! control protocol used by every client — the Tauri webview, LAN browsers, phones.
//!
//! Also hosts:
//! - `/qr.svg?data=...` — QR rendering for the connect dialog.
//! - `POST /handover` (loopback only) — lets a freshly-started backend take over:
//!   this instance stops its sACN output, hands back config + layer phases, and
//!   exits shortly after, so the successor can continue with visual continuity.
//!
//! Access control: clients identify with a persistent id. Revoked ids are refused
//! and kicked live. With `server.require_token` on, unknown ids must present the
//! join token (from the QR); loopback clients are always allowed.
//!
//! The frame loop never blocks on this server: frames arrive over a broadcast
//! channel and slow clients simply lag (dropped preview frames), never
//! back-pressuring the engine.

use crate::audio::RemoteChains;
use crate::config::ClientRecord;
use crate::media::{MediaResolver, ResolveRequest};
use crate::protocol::{
    BrowserAudioStream, ClientMsg, HandoverGrant, ServerMsg, PREVIEW_MAGIC, VIDEO_FRAME_MAGIC,
};
use crate::state::{PreviewFrame, SharedState};
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tower_http::cors::{Any, CorsLayer};

#[derive(rust_embed::Embed)]
#[folder = "../dist"]
struct Assets;

#[derive(Clone)]
struct Ctx {
    state: Arc<SharedState>,
    remote: RemoteChains,
    media: Arc<MediaResolver>,
}

pub fn spawn(state: Arc<SharedState>, remote: RemoteChains) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(serve(state, remote));
        })
        .expect("spawn server thread")
}

async fn serve(state: Arc<SharedState>, remote: RemoteChains) {
    let (bind, port) = {
        let cfg = state.config.read();
        (cfg.server.bind.clone(), cfg.server.port)
    };
    let media = MediaResolver::new().expect("media resolver HTTP client");
    // Background playlist upkeep: watched-folder scans + URL downloads to the
    // local media cache, so playback survives venue internet.
    tokio::spawn(crate::videocache::run(state.clone(), media.clone()));
    let ctx = Ctx { state: state.clone(), remote, media };
    let app = Router::new()
        .route("/ws", get(ws_upgrade))
        .route("/qr.svg", get(qr_svg))
        .route("/handover/state", get(handover_state))
        .route("/handover", post(handover))
        .route("/media/resolve", post(resolve_media))
        .route("/media/stream/{id}", get(stream_media))
        .route("/media/file/{id}", get(serve_media_file))
        .fallback(get(serve_asset))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([header::CONTENT_TYPE, header::RANGE]),
        )
        .with_state(ctx);

    let addr = format!("{bind}:{port}");
    // Retry: after a takeover the previous instance needs a moment to exit and
    // release the port.
    let mut listener = None;
    for attempt in 0..40 {
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) if attempt == 39 => {
                log::error!("cannot bind web server on {addr}: {e}");
                return;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    let listener = listener.unwrap();
    log::info!("web UI + control server on http://{addr}");
    let shutdown_state = state.clone();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        while !shutdown_state.shutdown.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    if let Err(e) = server.await {
        log::error!("web server error: {e}");
    }
}

// ---------------------------------------------------------------------------
// Browser-decodable media proxy
// ---------------------------------------------------------------------------

fn media_authorized(state: &SharedState, addr: SocketAddr, req: &ResolveRequest) -> bool {
    let cfg = state.config.read();
    if cfg
        .clients
        .iter()
        .any(|c| c.id == req.client_id && c.revoked)
    {
        return false;
    }
    if addr.ip().is_loopback() || !cfg.server.require_token {
        return true;
    }
    cfg.clients.iter().any(|c| c.id == req.client_id)
        || (!req.token.is_empty() && req.token == cfg.server.join_token)
}

async fn resolve_media(
    State(ctx): State<Ctx>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::Json(req): axum::Json<ResolveRequest>,
) -> Response {
    if !media_authorized(&ctx.state, addr, &req) {
        return (StatusCode::FORBIDDEN, "media access denied").into_response();
    }
    match ctx.media.resolve(&req.url).await {
        Ok(resolved) => axum::Json(resolved).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

async fn stream_media(
    State(ctx): State<Ctx>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let upstream = match ctx.media.stream(&id, range).await {
        Ok(response) => response,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("media stream error: {e:#}")).into_response(),
    };
    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::CACHE_CONTROL,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Serve a playlist entry's media from disk: the cached download for URL entries
/// or the file itself for local ones. Single-range requests are honored — the
/// browser <video> element requires them for seeking.
async fn serve_media_file(
    State(ctx): State<Ctx>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    serve_media_file_ranged(ctx, id, range).await
}

async fn serve_media_file_ranged_entry(
    ctx: &Ctx,
    id: &str,
) -> Option<(std::path::PathBuf, String)> {
    if let Some(hit) = crate::videocache::cached_file(id) {
        return Some(hit);
    }
    // Local-file entries stream straight from their configured path.
    let cfg = ctx.state.config.read();
    let entry = cfg.video.playlist.iter().find(|e| e.id == id)?;
    if entry.kind != crate::config::PlaylistKind::LocalFile {
        return None;
    }
    let path = std::path::PathBuf::from(&entry.source);
    path.is_file().then(|| {
        let content_type = match path.extension().and_then(|e| e.to_str()) {
            Some("webm") => "video/webm",
            Some("ogv") => "video/ogg",
            Some("mov") => "video/quicktime",
            _ => "video/mp4",
        };
        (path, content_type.to_string())
    })
}

async fn serve_media_file_ranged(ctx: Ctx, id: String, range: Option<String>) -> Response {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Some((path, content_type)) = serve_media_file_ranged_entry(&ctx, &id).await else {
        return (StatusCode::NOT_FOUND, "not cached or not a local file").into_response();
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return (StatusCode::NOT_FOUND, "media file unreadable").into_response();
    };
    let total = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "stat failed").into_response(),
    };

    let (start, end) = match range.as_deref().and_then(|r| parse_range(r, total)) {
        Some(r) => r,
        None if range.is_some() => {
            return (StatusCode::RANGE_NOT_SATISFIABLE, "bad range").into_response()
        }
        None => (0, total.saturating_sub(1)),
    };
    let len = end - start + 1;
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "seek failed").into_response();
    }
    let mut data = vec![0u8; len as usize];
    if file.read_exact(&mut data).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "read failed").into_response();
    }

    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len);
    if range.is_some() {
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"));
    }
    builder
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn parse_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let spec = range.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        // Suffix range: last N bytes.
        let n: u64 = end_s.parse().ok()?;
        let start = total.saturating_sub(n);
        return (total > 0).then_some((start, total - 1));
    }
    let start: u64 = start_s.parse().ok()?;
    let end = if end_s.is_empty() {
        total.checked_sub(1)?
    } else {
        end_s.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };
    (start <= end && start < total).then_some((start, end))
}

// ---------------------------------------------------------------------------
// Static assets, QR, handover
// ---------------------------------------------------------------------------

async fn serve_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| Assets::get("index.html")) {
        Some(file) => {
            let mime = mime_for(path);
            ([(header::CONTENT_TYPE, mime)], file.data).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "UI assets not built into this binary. Run `bun run build` before `cargo build`, \
             or use the desktop app / vite dev server.",
        )
            .into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

#[derive(serde::Deserialize)]
struct QrQuery {
    data: String,
}

async fn qr_svg(Query(q): Query<QrQuery>) -> Response {
    match qrcode::QrCode::new(q.data.as_bytes()) {
        Ok(code) => {
            let svg = code
                .render::<qrcode::render::svg::Color>()
                .min_dimensions(280, 280)
                .quiet_zone(true)
                .build();
            ([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("QR error: {e}")).into_response(),
    }
}

/// Two-phase takeover, phase 1 (side-effect-free): a starting successor fetches the
/// running state early so it can fully prepare (adopt config, build its sACN plan,
/// fill its render pipeline) while THIS instance keeps sending.
async fn handover_state(
    State(ctx): State<Ctx>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "handover is local-only").into_response();
    }
    log::info!("handover state requested — a successor instance is preparing");
    let grant = HandoverGrant {
        config: ctx.state.config.read().clone(),
        layer_phases: ctx.state.layer_phases.lock().clone(),
    };
    axum::Json(grant).into_response()
}

/// Phase 2 (commit): stop sACN NOW — and wait for the engine loop to ACK that it
/// skipped a send, so the wire provably never has two sources — then hand back
/// fresh layer phases (drift correction for the successor) and exit shortly.
async fn handover(
    State(ctx): State<Ctx>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "handover is local-only").into_response();
    }
    let state = ctx.state.clone();
    log::info!("handover commit — stopping sACN output");
    let t0 = Instant::now();
    state.leaving.store(true, Ordering::SeqCst);
    // Wait for the engine's quiesce ack (~1 frame period; cap in case the engine
    // thread is down, e.g. GPU error — then it wasn't sending anyway).
    while !state.sacn_quiesced.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_millis(150)
    {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    log::info!(
        "sACN quiesced in {:.1} ms",
        t0.elapsed().as_secs_f32() * 1000.0
    );

    let grant = HandoverGrant {
        config: state.config.read().clone(),
        layer_phases: state.layer_phases.lock().clone(),
    };

    // Exit from a plain thread, not the tokio runtime: setting `shutdown` tears the
    // runtime down (graceful server exit), which would cancel a tokio task before
    // it ever reached `process::exit` — leaving a zombie process.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        log::info!("handover complete; this instance is exiting");
        state.shutdown.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        std::process::exit(0);
    });

    axum::Json(grant).into_response()
}

// ---------------------------------------------------------------------------
// WebSocket clients
// ---------------------------------------------------------------------------

async fn ws_upgrade(
    State(ctx): State<Ctx>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| client_task(ctx, socket, addr))
}

struct PreviewSub {
    min_interval: Duration,
    decimate: u32,
    last_sent: Instant,
}

async fn client_task(ctx: Ctx, socket: WebSocket, addr: SocketAddr) {
    let state = ctx.state.clone();
    let mut events_rx = state.events.subscribe();
    let mut preview_rx = state.preview.subscribe();
    let (mut tx, mut rx) = socket.split();

    let conn_id = state.conn_seq.fetch_add(1, Ordering::SeqCst);
    state.status.lock().clients += 1;
    let mut client_id = String::new();
    let mut preview: Option<PreviewSub> = None;
    let mut announced_meta = (0u32, 0u32, 0u32);
    let mut queued_notified: Option<u32> = None;
    // Loopback clients (the desktop window, aux windows, local browsers) are
    // exempt from preview-slot rationing: their frames never cross the NIC.
    let is_local = addr.ip().is_loopback();
    let max_preview = |state: &SharedState| {
        state.config.read().server.max_preview_clients.max(1) as usize
    };

    // Greet with full state immediately.
    let hello = ServerMsg::State {
        config: Box::new(state.config.read().clone()),
        status: state.status.lock().clone(),
    };
    let _ = send_json(&mut tx, &hello).await;

    loop {
        tokio::select! {
            msg = rx.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(m) => {
                                let mut reset_meta = false;
                                if handle_msg(&ctx, m, &mut client_id, conn_id, addr, &mut preview, &mut reset_meta, &mut tx).await.is_err() {
                                    break;
                                }
                                if reset_meta {
                                    announced_meta = (0, 0, 0);
                                }
                            }
                            Err(e) => {
                                let _ = send_json(&mut tx, &ServerMsg::Error {
                                    message: format!("bad message: {e}"),
                                }).await;
                            }
                        }
                    }
                    Message::Binary(bytes) => {
                        if !client_id.is_empty() {
                            handle_video_frame(&state, conn_id, &bytes);
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            ev = events_rx.recv() => {
                // Live revocation: kicked within one event tick (status @2 Hz).
                if !client_id.is_empty() && is_revoked(&state, &client_id) {
                    let _ = send_json(&mut tx, &ServerMsg::Denied {
                        reason: "Access revoked by the operator.".into(),
                    }).await;
                    break;
                }
                // Queue position updates for clients waiting on a preview slot.
                if preview.is_some() && !is_local {
                    let pos = state.preview_gate.lock().position(conn_id);
                    if pos != queued_notified {
                        queued_notified = pos;
                        let msg = ServerMsg::PreviewQueue { position: pos.unwrap_or(0) };
                        if send_json(&mut tx, &msg).await.is_err() { break; }
                    }
                }
                match ev {
                    Ok(ev) => { if send_json(&mut tx, &ev).await.is_err() { break; } }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
            frame = preview_rx.recv() => {
                let frame = match frame {
                    Ok(f) => f,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let Some(sub) = preview.as_mut() else { continue };
                // Bandwidth rationing: only slot holders stream frames (loopback
                // clients are always allowed — no NIC involved).
                if !is_local {
                    let max = max_preview(&state);
                    if !state.preview_gate.lock().is_active(conn_id, max) { continue; }
                }
                if sub.last_sent.elapsed() < sub.min_interval { continue; }
                sub.last_sent = Instant::now();
                let meta = (frame.spokes, frame.pixels_per_spoke, sub.decimate);
                if meta != announced_meta {
                    announced_meta = meta;
                    let (outer, inner) = {
                        let g = &state.config.read().geometry;
                        (g.outer_radius_ft, g.inner_radius_ft)
                    };
                    let m = ServerMsg::PreviewMeta {
                        spokes: frame.spokes,
                        pixels: decimated_count(frame.pixels_per_spoke, sub.decimate),
                        decimate: sub.decimate,
                        outer_radius_ft: outer,
                        inner_radius_ft: inner,
                    };
                    if send_json(&mut tx, &m).await.is_err() { break; }
                }
                let bytes = encode_preview(&frame, sub.decimate);
                if tx.send(Message::Binary(bytes.into())).await.is_err() { break; }
            }
        }
    }

    {
        let max = max_preview(&state);
        state.preview_gate.lock().release(conn_id, max);
    }
    state.connected_clients.lock().remove(&conn_id);
    if state.stop_video(Some(conn_id)) {
        deactivate_video_audio(&ctx);
    }
    state.status.lock().clients -= 1;
}

fn handle_video_frame(state: &SharedState, conn_id: u64, bytes: &[u8]) {
    if bytes.len() < 12 {
        return;
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != VIDEO_FRAME_MAGIC {
        return;
    }
    let width = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
    let height = u16::from_le_bytes(bytes[10..12].try_into().unwrap());
    let _ = state.push_video_frame(conn_id, width, height, &bytes[12..]);
}

fn deactivate_video_audio(ctx: &Ctx) {
    let chains = ctx.remote.lock();
    for (key, chain) in chains.iter() {
        if matches!(key, crate::audio::BrowserAudioKey::Video) {
            chain.lock().deactivate(&ctx.state);
        }
    }
}

fn is_revoked(state: &SharedState, client_id: &str) -> bool {
    state
        .config
        .read()
        .clients
        .iter()
        .any(|c| c.id == client_id && c.revoked)
}

fn decimated_count(pixels: u32, decimate: u32) -> u32 {
    let d = decimate.max(1);
    pixels.div_ceil(d)
}

fn encode_preview(frame: &PreviewFrame, decimate: u32) -> Vec<u8> {
    let d = decimate.max(1);
    let out_pixels = decimated_count(frame.pixels_per_spoke, d);
    let mut bytes = Vec::with_capacity(12 + (frame.spokes * out_pixels * 3) as usize);
    bytes.extend_from_slice(&PREVIEW_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&(frame.frame_number as u32).to_le_bytes());
    bytes.extend_from_slice(&(frame.spokes as u16).to_le_bytes());
    bytes.extend_from_slice(&(out_pixels as u16).to_le_bytes());
    if d == 1 {
        bytes.extend_from_slice(&frame.rgb);
    } else {
        for spoke in 0..frame.spokes {
            let base = (spoke * frame.pixels_per_spoke) as usize;
            for i in (0..frame.pixels_per_spoke as usize).step_by(d as usize) {
                let o = (base + i) * 3;
                bytes.extend_from_slice(&frame.rgb[o..o + 3]);
            }
        }
    }
    bytes
}

type WsSink = futures_util::stream::SplitSink<WebSocket, Message>;

async fn send_json(tx: &mut WsSink, msg: &ServerMsg) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).expect("serialize ServerMsg");
    tx.send(Message::Text(text.into())).await
}

async fn deny(tx: &mut WsSink, reason: &str) -> Result<(), ()> {
    let _ = send_json(
        tx,
        &ServerMsg::Denied {
            reason: reason.into(),
        },
    )
    .await;
    Err(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_msg(
    ctx: &Ctx,
    msg: ClientMsg,
    client_id: &mut String,
    conn_id: u64,
    addr: SocketAddr,
    preview: &mut Option<PreviewSub>,
    reset_meta: &mut bool,
    tx: &mut WsSink,
) -> Result<(), ()> {
    let state = &ctx.state;
    match msg {
        ClientMsg::Hello {
            name,
            client_id: id,
            token,
        } => {
            let is_local = addr.ip().is_loopback();
            let (known, revoked, token_ok) = {
                let cfg = state.config.read();
                let rec = cfg.clients.iter().find(|c| c.id == id);
                (
                    rec.is_some(),
                    rec.is_some_and(|r| r.revoked),
                    token == cfg.server.join_token,
                )
            };
            if revoked {
                return deny(tx, "Access revoked by the operator.").await;
            }
            if state.config.read().server.require_token && !is_local && !known && !token_ok {
                return deny(
                    tx,
                    "This system requires a join token — scan the Connect QR code in the app.",
                )
                .await;
            }
            if !id.is_empty() {
                state.connected_clients.lock().insert(conn_id, id.clone());
                if !known {
                    let display_name = if name.is_empty() {
                        format!("device-{}", &id[id.len().saturating_sub(4)..])
                    } else {
                        name
                    };
                    state.update_config(|c| {
                        c.clients.push(ClientRecord {
                            id: id.clone(),
                            name: display_name,
                            revoked: false,
                        });
                    });
                }
            }
            *client_id = id;
        }
        ClientMsg::SetClientName { name } => {
            let id = client_id.clone();
            if !id.is_empty() && !name.is_empty() {
                state.update_config(|c| {
                    if let Some(r) = c.clients.iter_mut().find(|r| r.id == id) {
                        r.name = name;
                    }
                });
            }
        }
        ClientMsg::RenameClient { id, name } => {
            state.update_config(|c| {
                if let Some(r) = c.clients.iter_mut().find(|r| r.id == id) {
                    r.name = name;
                }
            });
        }
        ClientMsg::RevokeClient { id } => {
            state.update_config(|c| {
                if let Some(r) = c.clients.iter_mut().find(|r| r.id == id) {
                    r.revoked = true;
                }
            });
        }
        ClientMsg::UnrevokeClient { id } => {
            state.update_config(|c| {
                if let Some(r) = c.clients.iter_mut().find(|r| r.id == id) {
                    r.revoked = false;
                }
            });
        }
        ClientMsg::ForgetClient { id } => {
            state.update_config(|c| {
                c.clients.retain(|r| r.id != id);
            });
        }
        ClientMsg::RotateJoinToken => {
            state.update_config(|c| {
                c.server.join_token = crate::config::generate_token();
            });
        }
        ClientMsg::SetRequireToken { require } => {
            state.update_config(|c| {
                c.server.require_token = require;
            });
        }
        ClientMsg::GetState => {
            state.broadcast_state();
        }
        ClientMsg::SetConfig { config } => {
            let port_changed = {
                let cur = state.config.read();
                cur.server.port != config.server.port || cur.server.bind != config.server.bind
            };
            state.update_config(|c| {
                // Client management + tokens are edited via their dedicated
                // messages; don't let a stale full-config write clobber them.
                let clients = c.clients.clone();
                let join_token = c.server.join_token.clone();
                let require_token = c.server.require_token;
                let launch_at_startup = c.windows.launch_at_startup;
                *c = *config;
                c.clients = clients;
                c.server.join_token = join_token;
                c.server.require_token = require_token;
                c.windows.launch_at_startup = launch_at_startup;
            });
            if port_changed {
                let _ = send_json(
                    tx,
                    &ServerMsg::Error {
                        message: "Server bind/port changes take effect on restart".into(),
                    },
                )
                .await;
            }
        }
        ClientMsg::SetMaster { brightness, speed } => {
            state.update_config(|c| {
                if let Some(b) = brightness {
                    c.render.master_brightness = b.clamp(0.0, 1.0);
                }
                if let Some(s) = speed {
                    c.render.master_speed = s.clamp(0.0, 8.0);
                }
            });
        }
        ClientMsg::SetSacnEnabled { enabled } => {
            state.update_config(|c| c.output.enabled = enabled);
        }
        ClientMsg::AddLayer { layer } => {
            state.update_config(|c| {
                if c.layers.len() < crate::layers::MAX_LAYERS {
                    c.layers.push(layer);
                }
            });
        }
        ClientMsg::UpdateLayer { index, layer } => {
            state.update_config(|c| {
                if let Some(slot) = c.layers.get_mut(index) {
                    *slot = layer;
                }
            });
        }
        ClientMsg::RemoveLayer { index } => {
            state.update_config(|c| {
                if index < c.layers.len() {
                    c.layers.remove(index);
                }
            });
        }
        ClientMsg::MoveLayer { from, to } => {
            state.update_config(|c| {
                if from < c.layers.len() && to < c.layers.len() {
                    let l = c.layers.remove(from);
                    c.layers.insert(to, l);
                }
            });
        }
        ClientMsg::AuthorizeFirewall => {
            // Elevation blocks on the UAC dialog; run it off the async path.
            let state2 = state.clone();
            tokio::task::spawn_blocking(move || {
                let port = state2.config.read().server.port;
                match crate::firewall::authorize(port) {
                    Ok(()) => {
                        state2.status.lock().firewall_pending = false;
                        log::info!("firewall rule created for port {port}");
                    }
                    Err(e) => {
                        log::warn!("firewall authorization failed: {e:#}");
                        let _ = state2.events.send(ServerMsg::Error {
                            message: format!("Firewall authorization failed: {e}"),
                        });
                    }
                }
                state2.broadcast_state();
            });
        }
        ClientMsg::CheckUpdate => {
            state.update_check_requested.store(true, Ordering::SeqCst);
        }
        ClientMsg::InstallUpdate => {
            state.update_install_requested.store(true, Ordering::SeqCst);
        }
        ClientMsg::SetLaunchAtStartup { enabled } => {
            let state2 = state.clone();
            tokio::task::spawn_blocking(move || {
                let headless = state2.headless.load(Ordering::SeqCst);
                let outcome = crate::startup::reconcile(enabled, headless);
                let applied = outcome.succeeded && outcome.enabled == enabled;
                outcome.publish(&mut state2.status.lock());
                if applied {
                    state2.update_config(|c| c.windows.launch_at_startup = enabled);
                } else {
                    state2.broadcast_state();
                }
            });
        }
        ClientMsg::TriggerEffect { effect } => {
            state.trigger_effect(effect);
        }
        ClientMsg::Paint {
            pen,
            points,
            hue,
            saturation,
            brightness,
            size,
            intensity,
        } => {
            state.paint(pen, &points, hue, saturation, brightness, size, intensity);
        }
        ClientMsg::SubscribePreview { fps, decimate } => {
            *preview = Some(PreviewSub {
                min_interval: Duration::from_secs_f32(1.0 / fps.clamp(1.0, 60.0)),
                decimate: decimate.clamp(1, 64),
                last_sent: Instant::now() - Duration::from_secs(1),
            });
            // Loopback clients bypass slot rationing (no NIC traffic).
            if !addr.ip().is_loopback() {
                let max = state.config.read().server.max_preview_clients.max(1) as usize;
                let active = state.preview_gate.lock().request(conn_id, max);
                if !active {
                    let position = state.preview_gate.lock().position(conn_id).unwrap_or(0);
                    let _ = send_json(tx, &ServerMsg::PreviewQueue { position }).await;
                }
            }
            // Force a fresh PreviewMeta: the client may be a newly-mounted canvas
            // that never saw the one sent earlier on this connection.
            *reset_meta = true;
        }
        ClientMsg::UnsubscribePreview => {
            *preview = None;
            let max = state.config.read().server.max_preview_clients.max(1) as usize;
            state.preview_gate.lock().release(conn_id, max);
        }
        ClientMsg::StartVideo { title, source_url } => {
            if !client_id.is_empty() {
                deactivate_video_audio(ctx);
                state.start_video(conn_id, client_id, title, source_url);
            }
        }
        ClientMsg::StopVideo { force } => {
            if !client_id.is_empty()
                && state.stop_video(if force { None } else { Some(conn_id) })
            {
                deactivate_video_audio(ctx);
            }
        }
        ClientMsg::AudioFrame {
            stream,
            level,
            bass,
            mid,
            treble,
            flux,
        } => {
            // A soundtrack is authoritative only while this connection owns the
            // live video. Microphone packets retain their client-id routing.
            if stream == BrowserAudioStream::Video {
                let video = state.video.lock();
                if !video.active || video.owner_conn_id != conn_id {
                    return Ok(());
                }
            }
            let chains = ctx.remote.lock();
            for (key, chain) in chains.iter() {
                if key.matches(stream, client_id) {
                    chain
                        .lock()
                        .feed_remote(state, level, bass, mid, treble, flux);
                }
            }
        }
        ClientMsg::Imu {
            yaw,
            pitch,
            roll,
            shake,
        } => {
            let mut c = state.control.lock();
            c.yaw = yaw;
            c.pitch = pitch;
            c.roll = roll;
            c.shake = (c.shake + shake).min(3.0);
        }
    }
    Ok(())
}
