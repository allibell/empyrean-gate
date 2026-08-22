//! Empyrean Gate backend. The frame-generation engine, audio analysis, sACN output,
//! and web/WS server all run here, independent of any UI. The Tauri window is an
//! optional shell (skipped in `--headless` mode) whose webview is just another
//! WebSocket client of the local server.

pub mod audio;
pub mod config;
pub mod diagnostics;
pub mod engine;
pub mod geometry;
pub mod layers;
pub mod media;
pub mod protocol;
pub mod rhythm;
pub mod sacn;
pub mod server;
pub mod state;
pub mod updater;
pub mod videocache;
pub mod firewall;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use state::SharedState;

pub struct Backend {
    pub state: Arc<SharedState>,
}

/// Start every backend subsystem. UI-independent; returns immediately.
///
/// If another instance is already running on our port, take over from it: warm the
/// GPU engine first (sACN gated), ask the old instance to stop and hand back its
/// running state (config + layer phases), then start sending — the structure sees
/// at most a few frames of hold, and patterns continue without a visual jump.
pub fn start_backend() -> Backend {
    let cfg = config::load();
    let port = cfg.server.port;
    let takeover = port_in_use(port);
    let state = SharedState::new(cfg);
    {
        let mut st = state.status.lock();
        st.interfaces = list_interfaces();
        st.version = updater::CURRENT_VERSION.to_string();
        st.firewall_pending = firewall::rule_missing(port);
        let diagnostics = diagnostics::status();
        st.diagnostics_path = diagnostics.path;
        st.diagnostics_active = diagnostics.active;
        st.diagnostics_error = diagnostics.error;
    }
    if takeover {
        log::info!("port {port} is busy — attempting takeover of the running instance");
        state.sacn_hold.store(true, Ordering::SeqCst);
    }
    let remote_chains = audio::spawn(state.clone());
    rhythm::spawn(state.clone());
    engine::spawn(state.clone());

    if takeover {
        // Two-phase takeover. Phase 1 (old instance keeps sending): fetch its
        // running state and fully prepare — adopt config (sACN plan, buffers) and
        // phases, then let a few frames flow through the render+readback pipeline
        // so we could send *immediately*. Phase 2: commit — the old instance
        // quiesces (acked), returns fresh phases (drift correction), and exits;
        // we ungate sACN and the very next engine tick sends. Wire gap ≈ 1-2
        // frame periods.
        wait_for_engine(&state, std::time::Duration::from_secs(8));
        let t0 = std::time::Instant::now();
        let prepared = match fetch_handover_state(port) {
            Ok(grant) => {
                log::info!("takeover phase 1: adopted running state; warming pipeline");
                *state.layer_phases.lock() = grant.layer_phases;
                state.phases_transplanted.store(true, Ordering::SeqCst);
                state.update_config(|c| *c = grant.config);
                wait_frames(&state, 3, std::time::Duration::from_secs(2));
                true
            }
            Err(e) => {
                log::warn!("old instance has no prepare endpoint ({e}); single-phase takeover");
                false
            }
        };
        match commit_handover(port) {
            Ok(grant) => {
                *state.layer_phases.lock() = grant.layer_phases;
                state.phases_transplanted.store(true, Ordering::SeqCst);
                if !prepared {
                    state.update_config(|c| *c = grant.config);
                }
                log::info!(
                    "takeover committed in {:.0} ms total; resuming sACN",
                    t0.elapsed().as_secs_f32() * 1000.0
                );
            }
            Err(e) => {
                log::warn!(
                    "takeover commit failed ({e}); continuing anyway — the server will \
                     retry binding the port"
                );
            }
        }
        state.sacn_hold.store(false, Ordering::SeqCst);
    }

    server::spawn(state.clone(), remote_chains);
    updater::spawn(state.clone());
    Backend { state }
}

fn port_in_use(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

fn wait_for_engine(state: &SharedState, timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        {
            let st = state.status.lock();
            if !st.gpu_name.is_empty() || st.gpu_error.is_some() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    log::warn!("engine warm-up timed out; proceeding with takeover anyway");
}

fn handover_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .into()
}

fn fetch_handover_state(port: u16) -> anyhow::Result<protocol::HandoverGrant> {
    let mut resp = handover_agent()
        .get(format!("http://127.0.0.1:{port}/handover/state"))
        .call()?;
    Ok(resp.body_mut().read_json::<protocol::HandoverGrant>()?)
}

fn commit_handover(port: u16) -> anyhow::Result<protocol::HandoverGrant> {
    let mut resp = handover_agent()
        .post(format!("http://127.0.0.1:{port}/handover"))
        .send_empty()?;
    Ok(resp.body_mut().read_json::<protocol::HandoverGrant>()?)
}

/// Wait until the engine has rendered `n` more frames (pipeline warm with the
/// adopted config) or the timeout passes.
fn wait_frames(state: &SharedState, n: u64, timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    let base = state.frames_rendered.load(Ordering::Relaxed);
    while state.frames_rendered.load(Ordering::Relaxed) < base + n && start.elapsed() < timeout {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Give the engine a moment to put E1.31 stream-termination packets on the wire
/// before the process goes away. Without this, `run` returns straight into process
/// teardown and the rig is left holding its last frame until the receivers time out.
fn await_sacn_terminate(state: &SharedState) {
    let start = std::time::Instant::now();
    while !state.sacn_terminated.load(Ordering::SeqCst)
        && start.elapsed() < std::time::Duration::from_millis(500)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Local IPv4 interfaces as "name — ip" for the sACN interface picker.
fn list_interfaces() -> Vec<String> {
    match local_ip_address::list_afinet_netifas() {
        Ok(ifas) => ifas
            .into_iter()
            .filter(|(_, ip)| ip.is_ipv4() && !ip.is_loopback())
            .map(|(name, ip)| format!("{name} — {ip}"))
            .collect(),
        Err(e) => {
            log::warn!("cannot enumerate network interfaces: {e}");
            Vec::new()
        }
    }
}

/// The port the local web/WS server listens on — the webview client asks for this.
#[tauri::command]
fn backend_info(state: tauri::State<'_, Backend>) -> serde_json::Value {
    let cfg = state.state.config.read();
    serde_json::json!({ "wsPort": cfg.server.port })
}

pub fn run(headless: bool) {
    let diagnostics = diagnostics::init();
    if diagnostics.active {
        log::info!("persistent diagnostics: {}", diagnostics.path);
    } else {
        log::warn!("persistent diagnostics unavailable at {}: {}", diagnostics.path, diagnostics.error);
    }
    let backend = start_backend();
    let state = backend.state.clone();
    state.headless.store(headless, Ordering::SeqCst);

    if headless {
        log::info!("headless mode: no desktop window; web UI only");
        // Park until Ctrl+C or a shutdown (e.g. a successor took over).
        while !state.shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        await_sacn_terminate(&state);
        return;
    }

    tauri::Builder::default()
        .manage(backend)
        // Persists per-label window geometry (position/size/maximized) across
        // restarts — and across versions, since the state file lives in the app
        // config dir. Combined with stable aux labels, a self-update handover
        // brings every window back where it was.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![backend_info, open_aux])
        .setup(|app| {
            use tauri::Manager;
            // Recreate the aux windows that were open last run (their geometry is
            // restored by the window-state plugin via their stable labels).
            let aux: Vec<String> = {
                let backend = app.state::<Backend>();
                let cfg = backend.state.config.read();
                cfg.windows.aux_open.clone()
            };
            for tab in aux {
                if let Err(e) = open_aux_window(app.handle(), &tab) {
                    log::warn!("could not restore '{tab}' window: {e}");
                }
            }
            // The handover exit path is process::exit, which skips graceful window
            // teardown — save window state periodically so at most ~5 s of window
            // moves can be lost.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let _ = handle.save_window_state(StateFlags::all());
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // A user closing an aux window removes it from the restore list;
            // process teardown fires Destroyed (not CloseRequested), so app exit
            // keeps the list intact.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(tab) = window.label().strip_prefix("aux-") {
                    use tauri::Manager;
                    let tab = tab.to_string();
                    let backend = window.app_handle().state::<Backend>();
                    backend.state.update_config(|c| {
                        c.windows.aux_open.retain(|t| *t != tab);
                    });
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    state.shutdown.store(true, Ordering::SeqCst);
    await_sacn_terminate(&state);
}

/// Create (or focus) the popped-out window for a tab, with a stable label so the
/// window-state plugin can restore its geometry. Records it for restore-on-start.
#[tauri::command]
fn open_aux(app: tauri::AppHandle, tab: String, state: tauri::State<'_, Backend>) -> Result<(), String> {
    use tauri::Manager;
    let label = format!("aux-{tab}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }
    open_aux_window(&app, &tab).map_err(|e| e.to_string())?;
    state.state.update_config(|c| {
        if !c.windows.aux_open.contains(&tab) {
            c.windows.aux_open.push(tab.clone());
        }
    });
    Ok(())
}

fn open_aux_window(app: &tauri::AppHandle, tab: &str) -> tauri::Result<()> {
    let label = format!("aux-{tab}");
    // The hash is applied by an init script (a fragment inside WebviewUrl::App
    // paths does not survive URL conversion reliably).
    tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App("index.html".into()))
        .title(format!("Empyrean Gate — {tab}"))
        .inner_size(900.0, 900.0)
        .initialization_script(format!("if (!location.hash) location.hash = '#{tab}';"))
        .build()?;
    Ok(())
}
