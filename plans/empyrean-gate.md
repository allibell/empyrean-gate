# Empyrean Gate — pattern generator

## Goal

Greenfield Tauri + React desktop app that generates visual patterns for the Empyrean
Gate — a radial array of lights above a dance floor — and outputs them over sACN
(E1.31). GPU-computed patterns (Vulkan, no fallback), live preview UI, settings page,
audio-reactive (beat + multi-band features from DJ audio input), effects triggered by
keyboard/mouse/touch. CI builds a standalone binary (no installer/updater).

## Physical installation (defaults; ALL configurable in-app — user is unsure of exact numbers)

- 64 spokes of LED strip in a radial array ("wagon wheel" viewed from below).
- 16× Advatek Pixlite Mk4-S controllers, 4 strings each → 64 strings, 1 string = 1 spoke.
- ~350 px per spoke (default 350, configurable).
- Major (outer) diameter 50 ft → outer radius 25 ft. Minor radius ~15–20 ft diameter →
  default inner radius 8 ft (configurable; user unsure).
- LED density 30 or 60 LED/m (default 60, configurable; only affects physical-space mapping).
- **Strings are fed from the outside**: pixel 0 = outer radius (50 ft dia), last pixel =
  innermost radius. Spoke direction matters for chases.
- Protocol: sACN over UDP 5568. Unicast to controller IPs (configurable) or multicast
  239.255.u.u. 350 px = 1050 ch → 3 universes/spoke (170 px per universe), each spoke
  starts on a fresh universe boundary → 192 universes total by default.

## Decisions already made (don't re-ask)

- **wgpu locked to `Backends::VULKAN`** — satisfies "Vulkan for open-source portability"
  with far better ergonomics than raw ash; WGSL compiles to SPIR-V on Vulkan. No fallback
  backends; adapter failure = clear fatal error surfaced in UI.
- Patterns computed from scratch every frame in one compute dispatch (layer stack loop
  per pixel). No CPU-side pattern math.
- Frontend: React + TypeScript + Vite, **Bun** (`bun.lock`, per global instructions).
- sACN sender is hand-rolled (~150 lines, well-specified protocol) — no dep risk.
- sACN output defaults **OFF** on first launch (192 universes @ 60 fps is ~14 MB/s of
  UDP; don't flood networks by default). Big toggle in UI.
- Audio: cpal input + rustfft; spectral-flux onset detection, autocorrelation tempo,
  bass/mid/treble bands with slow AGC. All on CPU (tiny), features feed GPU uniforms.
- **Multiple audio inputs in parallel** (user request mid-build): config defines up to 4
  named sources; each source = capture device + channel selection (downmixed to mono for
  analysis), each with its own full analysis chain. Layers/effects carry an
  `audio_source` index selecting which source drives them. Multichannel interfaces (e.g.
  stage feed on ch 1–2, local mic on ch 3) map to separate sources on the same device.
- **Backend is primary; UI is a client** (user request mid-build): frame generation runs
  on a dedicated OS thread (GPU dispatch → readback → sACN) fully independent of any UI.
  The backend hosts an axum HTTP+WebSocket server (default port 9520, bind 0.0.0.0)
  serving the built React bundle and a single JSON+binary WS protocol. The Tauri window
  is just another WS client; phones/laptops on the LAN connect to the same server.
- **Remote inputs over WS**: browser mic → client-side WebAudio feature extraction
  (level/bands/spectral flux) streamed as compact packets; backend runs the same beat
  tracker on them as on local cpal sources (audio source kind = Device | Remote). Phone
  IMU/orientation → control bus (tilt/yaw uniforms, steer effect positions).
- Preview over WS binary frames; per-client fps + pixel decimation so phones on weak
  WiFi can subscribe cheaply. WebGL2 point rendering in React.
- **Headless mode** (user request mid-build): `empyrean-gate --headless` skips the Tauri
  window; backend + web UI only (for headless show machines). No auth for now, but
  `server.auth_token` config field + `token` in the WS Hello exist so tokens can be
  enforced later without protocol/config migration.
- Headless smoke-test binary (`engine-smoke`) inits Vulkan + renders one frame + exits,
  so CI/dev can verify the engine without opening a window.
- Standalone binary via `tauri build --no-bundle`; release builds embed frontend assets.
- Repo branch: `master`.

## Scaling intent (user request mid-build)

~20k pixels today; design must scale to hundreds of thousands, eventually millions,
"without much trouble". Already satisfied: pixel count is pure config, single compute
dispatch (OK to ~16M px), ping-pong staging readback (overlaps compute), zero-alloc
sACN packet scatter. Known walls at ~1M px, deliberately out of scope now:
- Network: ~5900 universes @ 60 fps ≈ 226 MB/s → 10GbE / multiple NICs (hardware).
- Per-packet `send_to` syscalls (~350k/s) → batch with `sendmmsg` (Linux) / RIO
  (Windows). The sender is a clean frame→transport interface so this can be swapped
  without touching the engine.
- GPU→sACN path stays: engine buffer → LUT scatter → resident packets; no per-frame
  allocation anywhere on that path.

## Architecture

```
src/                  React UI (Preview + Settings), WebGL2 preview, keyboard effects
src-tauri/src/
  main.rs             Tauri setup, commands, frame-loop thread spawn
  config.rs           AppConfig (geometry, controllers, audio, output), persisted JSON
  geometry.rs         polar layout, universe mapping
  engine/mod.rs       wgpu Vulkan init, pipeline, frame loop, readback
  engine/shaders/gate.wgsl   layer stack + effects + noise lib, all patterns
  layers.rs           LayerParams / EffectInstance structs (bytemuck ↔ WGSL)
  sacn.rs             E1.31 packet builder + per-universe sequenced sender
  audio.rs            cpal capture, FFT, features, beat tracker
  state.rs            shared EngineState (params written by UI commands, read by loop)
  bin/engine_smoke.rs headless one-frame render test
```

Data flow: UI commands mutate shared state → frame loop (fixed-rate thread) packs
uniforms (time, audio features, layers, effects) → compute dispatch → readback →
sACN sender + preview channel.

## Plan / steps

- [x] git init (master), plan doc
- [x] Scaffold: package.json/Vite/TS, src-tauri (Cargo, tauri.conf.json, capabilities, icons)
- [x] Rust: config + geometry + state + protocol
- [x] Rust: engine (wgpu Vulkan-only, ping-pong readback) + WGSL shader (11 layer kinds,
      4 effects, 3D simplex noise, soft-clip tonemap)
- [x] Rust: sACN sender (allocation-free, prebuilt packet templates)
- [x] Rust: audio (multi-source cpal + channel select, FFT features, beat tracker,
      remote-source chains)
- [x] axum server: embedded web UI + WS protocol, per-client preview throttle/decimate
- [x] Headless mode (`--headless`), auth-token placeholder
- [x] engine-smoke binary; `cargo check --all-targets` clean, zero warnings
- [x] React UI: preview (WebGL2, click-to-burst, keys 1–4), settings (layers/audio/
      output/geometry), remote mic + IMU senders
- [x] E2E test passed: HTTP + WS + 10 preview frames + effect trigger against live
      backend (scripts/e2e-test.ts)
- [x] GitHub Actions CI (windows + linux standalone binary artifacts)
- [x] README
- [x] `bun tauri build --no-bundle` release build validated (17.3 MB standalone exe;
      release engine-smoke passes, checksum matches debug — deterministic)
- [x] Fixed: audio streams / sACN plan no longer rebuilt on unrelated config changes
      (brightness slider was tearing down capture streams via the epoch bump)
- [x] Initial commit on master
- [x] WIP tracker entry added (P:\Projects\WIP\personal\empyrean-gate.md)

## Round 2 (same session, user requests mid-build)

- [x] **PWA**: manifest + minimal network-first service worker (registered only when
      served by the backend, not Tauri/dev) + iOS meta tags + 180/192/512 icons.
      Installable on iPad, standalone fullscreen.
- [x] **Live drawing with pens**: `Paint` WS message streams polar dabs (batch per
      pointer frame, coalesced events); backend keeps ≤512 aged dabs (oldest evicted);
      GPU renders Glow/Ripple/Sparkle pens each frame from scratch (binding 5).
      Collaborative across clients. E2E-tested.
- [x] **UI restructured into 4 hash-routed tabs**: View / Draw / Control / Settings
      (`/#draw` etc. — PWA shortcuts + popped-out windows pin a mode). Tauri app has
      "⧉ New window" (labels `aux-*`, capability added) for separate-window operation.

## Round 3 (first live run feedback, 2026-08-19)

- [x] **sACN egress interface picker** — root cause of "sACNView sees nothing":
      socket bound 0.0.0.0, multicast went out the default-route NIC, not the
      10.255.0.77 lighting network. `output.interface` binds the socket + sets
      IP_MULTICAST_IF (socket2). NIC list via local-ip-address in status.
- [x] Multicast now defaults ON (enable toggle still defaults OFF).
- [x] `sacn_pps` live packets/s in status + HUD ("is it transmitting" truth).
- [x] `sync_to_render` (default on, capped by fps field) + computed LED-wire fps
      ceiling shown in UI (~88 fps at 350 px: 800 kbps × 24 bits + reset).
- [x] **E1.31 universe synchronization** (`sync_universe`, 0=off): data packets carry
      sync address; one sync packet/frame to the selected output destination(s).
      PixLite Mk4 latches tear-free; non-supporting receivers ignore.
- [x] **Fixed: all tabs black after visiting Draw** — server sent PreviewMeta once per
      connection; later canvas mounts resubscribed but never got meta → GL never
      initialized. Meta now re-announced on every SubscribePreview. Also release GL
      contexts on unmount (browser ~16-context cap).
- [x] **Audio loopback sources** — WASAPI loopback via cpal (output device as input);
      picker lists output devices.
- [x] **Autopilot random walk** — OU (mean-reverting) drift per layer param, slider
      value = walk center, per-layer `walk_amount` = radius (the "limit"), global
      enable + speed (tau ≈ 45s/speed). Runtime-only; never rewrites config.
- [x] **6 new layers**: Rainbow, Wedges, Interference, Fire, Meteors, Warp (17 total).
- [x] Shader/pipeline validation now goes through an error scope → broken WGSL (live
      editing) surfaces as a UI error instead of killing the engine thread.
      (Found because `active` is a reserved WGSL keyword — the panic killed the loop.)
- [x] `default-run = "empyrean-gate"` (two-binary crate broke `tauri dev`).
- Unicast question answered: multicast + IGMP snooping is correct; static controller
  IPs would NOT improve performance; unicast only for snooping-less switches/WiFi.

## Round 4 (2026-08-19, later)

- [x] Web UI auto-refresh on stale bundle (compare content-hashed entry script vs
      freshly-fetched /index.html on every WS connect; sessionStorage loop guard).
- [x] **Connect QR**: `/qr.svg?data=` endpoint (qrcode crate, SVG); ⊕ Connect modal
      with per-interface join URL `http://<ip>:<port>/?join=<token>`.
- [x] **Client management**: persistent client ids + names; ClientRecord list in
      config; Clients panel (rename / revoke / unrevoke / forget); revoke kicks live
      (checked on the 2 Hz event tick) and blocks rejoin; `require_token` +
      `rotate_join_token` for real lockout; loopback always allowed; join token
      captured from `?join=` into localStorage and sent in hello.
- [x] **Seamless backend takeover**: new instance detects busy port → warms engine
      (sACN gated by `sacn_hold`) → `POST /handover` (loopback-only) → old instance
      stops sACN BEFORE replying (no two-source overlap), returns config +
      layer_phases → new adopts (phases transplanted via flag) → old exits.
      Verified end-to-end: A exits code 0, B serving in <2 s, sub-second sACN gap.
- [x] `EMPYREAN_CONFIG` env var overrides config path (tests / isolated instances).
- [x] Audio stream error log-throttling (underruns come in bursts).
- [x] Fix: handover exit task originally died with the tokio runtime → zombie
      process; exit now runs on a plain thread, and the headless main loop watches
      the shutdown flag.

### Gotcha (cost a dev-app crash)

Running extra instances of `target\debug\empyrean-gate.exe` while `tauri dev` is
watching → the watcher's relink hits "Access is denied" and `tauri dev` DIES
(taking the desktop window with it). Test spare instances from a COPY of the exe.

## Round 5 (2026-08-19, later)

- [x] **Two-phase handover**: GET /handover/state (prepare, side-effect-free) lets the
      successor adopt config+phases and warm its pipeline while the old instance still
      sends; POST /handover (commit) waits for the engine's quiesce ACK (measured
      6.6 ms) and returns fresh phases (drift correction). Wire gap ≈ 1–2 frame
      periods. Fallback to single-phase for old instances. Verified end-to-end.
- [x] Pushed to **github.com/cinderblock/empyrean-gate** (private). CI matrix now
      windows + linux + macos. (macOS minutes are 10× on private repos — flip public
      or drop macos if quota matters.)
- [x] CI green on ALL targets (run 32281656912, commit `825050f`): linux 8m ✓,
      macos 9m ✓, windows 14m ✓; standalone binary artifacts 7–8 MB each.
- Findings:
  - macOS: wgpu's plain `vulkan` feature is unimplemented there — `Instance::new`
    PANICS. Fixed with target-specific `vulkan-portability` (MoltenVK) + engine init
    wrapped in `catch_unwind` (frame loop and engine-smoke) so init panics surface
    as GPU errors, never dead threads.
  - Fat LTO (`lto = true`, `codegen-units = 1`) made CI Linux jobs take 45+ min
    (2-core runner, cache can't help the LTO relink). Switched to thin LTO +
    `codegen-units = 4` → Linux 8 min WITHOUT cache. rust-cache@v2 was already in
    the workflow; added `cache-on-failure: true` so red runs still prime it.
  - Repo made public on user request (also: free Actions minutes).

## Round 6 (2026-08-19, live-testing feedback)

- [x] **Alt-tab fps drop diagnosed + fixed**: Windows coarsens sleep granularity to
      ~15.6 ms when the app loses foreground → engine pacing overshot. Fixed with
      `timeBeginPeriod(1)` (winmm) in the engine thread. Affects real output, not
      just UI.
- [x] **Unsteady sACN rate fixed**: pacer was `last = now` (drifts + aliases against
      the render tick); now accumulator-scheduled (`next += interval`), and
      sync-to-render sends every rendered frame outright when the cap doesn't bind.
- [x] **pkts/s display steadied**: per-second buckets instead of fractional-window
      division; status reports the last full second.
- [x] **fps + pkt/s history bars** (last 30 s, per-second buckets) in View HUD and
      Control (Sparkbars component; single-series, direct-labeled, text-ink values).
- [x] **UI fixes**: sACN enable row reads as an action ("Enable sACN output") with a
      separate status pill; interface picker text cleaned up; "no save button —
      changes are live" hint + "✓ saved" flash in the top bar on every confirmed
      config change.
- [x] **Gray-code layer walk**: autopilot can now walk WHICH layers play — exactly
      one layer fades in/out per step (4 s envelope), never fewer than
      `walk_min_layers` on (default 2). Off by default; toggle in Control → Autopilot.

## Round 7 (2026-08-19, sACN protocol conformance)

Prompted by "what is the CID, does it change every boot, are we using sACN well?" —
audit found the identity/lifecycle half of E1.31 was unimplemented.

- [x] **Persistent CID** (`output.cid`, UUID v4, generated once in `config::load` like
      the join token). Was `b"EmpyreanGate" + process::id()` → **a new source identity
      every launch**. Consequences that fixes: no more 2.5 s HTP-merge against our own
      ghost after a restart; no burning a slot in controllers that cap sources per
      universe (PixLite: a handful); and handovers are now genuinely seamless because
      the successor process reads the *same* CID out of the config.
      Added `uuid = "1.24.1"` (v4) — the old value was not an RFC 4122 UUID at all.
- [x] **Stream termination** (options bit 6, 3 packets/universe): sent on output
      disable, on app exit, and for universes a reconfigure drops. Previously the rig
      held its last frame through the receiver's source-loss timeout and, on
      hold-last-look controllers, indefinitely. **Deliberately NOT sent on handover**
      (`state.leaving`) — the successor continues the same CID's stream, and
      terminating would blink the rig between instances.
      Exit needed a new `state.sacn_terminated` ack + a ≤500 ms wait in `lib::run`,
      or the process died before the packets left the socket.
- [x] **Universe discovery** (E1.31-2016): prebuilt pages, sent to 239.255.250.214
      every 10 s from inside `send_frame` (only due while transmitting; keeps cadence
      independent of frame rate). This is what makes the source visible in sACNView
      and controller UIs. Toggle: `output.discovery`, default on.
- [x] **Configurable source name** (`output.source_name`, default "Empyrean Gate"),
      64-byte field, truncated on a char boundary.
- [x] **First unit tests in the repo** (7, in `sacn.rs`): discovery packet layout
      field-by-field, 512-universe paging, terminate option bit + sequence + template
      restoration (over a real loopback UDP socket), name truncation, CID byte order.
      CI runs them with `cargo test --release --lib` *after* the release builds, so it
      reuses those artifacts instead of compiling the tree again in the debug profile.
- Declined (agreed with user): per-address priority (start code 0xDD) — an ETC
  convention, not core E1.31, and nothing in this rig consumes it.
- [x] Fixed in Round 8: multicast and controller unicast are now exclusive modes, so
      saved controller addresses no longer duplicate every packet while multicast is on.

## Released

- **v0.1.0** (2026-08-19): https://github.com/cinderblock/empyrean-gate/releases/tag/v0.1.0
  Cut by the tag-triggered Release workflow (checks → 3-target build → publish);
  assets: windows-x64.exe (19.4 MB), linux-x64 (23.8 MB), macos-arm64 (19.7 MB).
  Future releases: `git tag vX.Y.Z && git push origin vX.Y.Z` — CI does the rest.

## Round 8 (2026-08-20): self-update

- [x] Updater thread (`updater.rs`): GitHub Releases API check (6 h + startup,
      auto_check default on), download platform asset to a VERSIONED SIBLING file
      (never overwrite the locked running exe), spawn → two-phase takeover → old
      exits. Old versioned binaries cleaned up at later startups. auto_install
      opt-in. `EMPYREAN_FAKE_VERSION` test hook.
- [x] Version chip in the top-bar corner (click = check; lit = click to hot-swap) +
      Settings → Updates panel.
- [x] Verified end-to-end against the real v0.1.0 release: download → spawn →
      handover → successor serving (scripts/update-test.ts).
- [x] v0.2.0 bumped (separate commit, per user rule: bumps never mix with code) and
      tagged; released with all three assets:
      https://github.com/cinderblock/empyrean-gate/releases/tag/v0.2.0
- Note: v0.1.0 binaries predate the updater — first swap to 0.2.0 is manual.

## Round 9 (2026-08-20): windows restore, viewer queue, walk visibility

- [x] Window restore across restarts/self-updates (tauri-plugin-window-state, stable
      aux labels, aux_open recreated at startup, 5 s periodic saves). Needs one
      visual verification pass by the user.
- [x] Preview-slot queue: `server.max_preview_clients` (default 10) rations the
      preview stream (>98% of client bandwidth); control is never gated; FIFO with
      live position banner. queue-test.ts verifies (cap 2, 3 viewers, promotion).
- [x] Phone preview default 20 fps @ 1/6 px ≈ 1.4 Mbps/phone (was ~4.1) — ~10
      viewers ≈ 14 Mbps, comfortable on venue WiFi.
- [x] Walk visibility: global "Walk depth" (0–3) multiplier + "Add missing kinds"
      button so the gray-code layer walk can tour all 19 patterns. (Walk was
      working but too subtle at defaults, and only ever tours stack layers.)
- [x] v0.3.0 tagged (window fix + these features; bump in its own commit).

## Round 10 (2026-08-20): live video input + destination cleanup

- [x] **Video is a first-class GPU layer** (20th layer kind): a bounded RGBA storage
      texture is sampled directly in WGSL and remains composable with blend, opacity,
      audio response, and autopilot. Treatment controls: zoom, 0–10-way mirrored
      kaleidoscope, contrast, rotation/spin, saturation, tint/original-color mix, and
      brightness.
- [x] **iPad/browser transmitter**: Video tab accepts a URL or local video file,
      decodes with the browser's hardware media stack, square-crops to 64/96/128 px,
      and sends binary RGBA frames at 10/15/24 fps. Backpressure drops frames rather
      than queueing latency. The decoder stays mounted offscreen while the operator
      visits Live or Settings, and reconnect/takeover reclaims the source.
- [x] **URL resolution**: direct video, `og:video`, `twitter:player:stream`, and HTML
      `<video>/<source>` are supported. Short-lived opaque proxy paths forward Range
      requests and make canvas extraction CORS-clean. Optional `yt-dlp` fallback is
      probed before returning a provider result; DRM/login-gated pages are not claimed.
- [x] **SSRF defense**: HTTP(S) only, no URL credentials, public IP space only across
      DNS + every redirect, DNS answers pinned to the validated connection, redirect
      cap, bounded HTML inspection, request timeouts, and HTTP authorization matching
      the WS join policy.
- [x] **Single-source ownership**: one connection owns live video frames; another may
      take over deliberately, stale connections cannot inject/stop it, and disconnect
      clears the frame immediately. Source title/owner/dimensions/fps/frame count are
      visible to every client.
- [x] **sACN destination mode is exclusive**: multicast vs controller unicast is now
      an explicit choice. Controller addresses remain saved when switching modes, but
      sequence-identical duplicate packets are no longer emitted.
- [x] Verification: frontend production build + typecheck; all Rust targets compile;
      15 library tests and 6 benchmark tests; Vulkan shader validated with all 20
      layers plus a video-only GPU scenario; isolated-port HTTP/CORS/Range/SSRF tests;
      WS E2E video-frame test; real in-app browser test at desktop, iPad portrait, and
      narrow phone viewports.

## Next session pickup

- Run `bun tauri dev` and eyeball the actual patterns; tune defaults.
- Get real geometry numbers from the user (px/spoke, radii, LED density) and
  controller IPs; test against a PixLite.
- Consider GitHub remote + first CI run.
- Next performance ceiling: batched UDP I/O (`sendmmsg`/RIO) for 100k+ pixel scales.
- Media follow-ups: resilient provider-specific extraction and authenticated/DRM
  sources only if a deployment actually requires them.

## Round 12: unattended show scheduler

- [x] Durable saved playlists with embedded scene snapshots, per-cue dwell and
      crossfade times, reordering, add/remove, naming, repeat, skip, and stop/hold.
- [x] Backend-owned show clock: advances headlessly, persists the active cue, and
      resumes the enabled playlist after a process restart without a controller.
- [x] Smoothstep layer-stack crossfades with incoming phase preservation, so the
      end of a transition does not reset the new scene's motion.
- [x] Nine built-in long-play compositions and a one-click all-night journey
      (35 minutes each, 20 second transitions, repeat forever).
- [x] Accelerated two-scene integration run: transition observed, auto-advance
      confirmed, no GPU error, and active cue restored after restart.
- [ ] Real PixLite/sACN and production Mac mini validation is deliberately deferred
      until the installation hardware is unpacked on playa next week.

## Round 13: restore Replay as a production workflow

- [x] Reversed the product-level intent of `8a8325e`: Archive is again a normal
      production tab and `/#replay` works in desktop, headless web, and PWA builds.
- [x] Restored single-file playback, whole `Uprising-Data` folder indexing,
      metadata titles, recent filesystem references, seeking, looping, and variable
      playback speed. Recordings remain local and stream one frame at a time.
- [x] Kept the shared per-user Vite fixture cache as an optional development
      convenience without making Replay depend on that endpoint.

## Round 11: external rhythm sources

- [x] Split lighting timing from per-layer audio energy without changing the default
      behavior: Layer Audio still gives every layer the beat belonging to its own
      level/bands/waveform/spectrum source.
- [x] Add a global MIDI Timing Clock adapter (24 PPQN) with tempo/phase extrapolation,
      Start/Continue/Stop, Song Position, exact-port hot-plug recovery, ±250 ms visual
      latency calibration, live health, and optional fallback to a chosen audio source.
- [x] Manual BPM remains the explicit highest-priority override; half/normal/double
      time and beat taps operate on the selected effective lighting clock.
- [x] Add receive-only native PRO DJ LINK beat/status input. It listens on the
      standard UDP 50001/50002 ports, follows tempo-master status or a pinned player,
      handles master handoff, and deliberately never claims a virtual deck identity
      or emits a control packet onto the DJ network.
- [x] Add a real published Boiler Room track-list excerpt plus synthetic deck/BPM/
      cue annotations and a UDP+WebSocket E2E replay (`scripts/pioneer-link-test.ts`).
      No copyrighted audio is stored. Source facts are explicitly distinguished
      from test-only annotations in the fixture.
- [ ] Validate against the actual production deck/mixer models before enabling at a
      show. Add rekordbox track/cue/phrase metadata only after the beat/master path is
      proven on that hardware; official Bridge/TCNet remains an alternate adapter.

### Production performance baseline

- The production show machine is an older Mac mini than the development Mac; exact
  model/specs are not yet recorded. Treat its release-build benchmark as the real
  performance baseline before increasing layer/pixel load or doing speculative
  optimization.
- On that Mac mini, run
  `cargo run --release --bin engine-smoke -- --suite --warmup 120 --frames 600 --json`
  with the real geometry. Keep the report with the machine model, macOS version, GPU,
  and release version. The existing Intel-iGPU 1.74 ms development result is useful
  headroom evidence, not a production guarantee.
- Continue prioritizing deadline misses/p95-p99 frame time over mean frame time. The
  next known scaling optimization remains batched UDP I/O at 100k+ pixels.

## Findings / gotchas

- **wgpu 30 crashes (STATUS_ACCESS_VIOLATION) inside `vkCreateDevice` on this dev
  machine** — Intel UHD Graphics, driver 30.0.101.1660 (2022-03-17, Vulkan 1.3.205).
  Not the validation layer (crashes with `InstanceFlags::empty()` too). **wgpu 29 works
  fine on the same driver** → pinned `wgpu = "29"`. Revisit 30+ after a driver update.
  Consider updating the Intel driver on this machine (user decision, not done).
- wgpu validation layers are off by default (`EMPYREAN_GPU_DEBUG=1` opts in): the
  installed VulkanSDK 1.4.304 validation layer is another crash suspect on this driver.
- Engine perf on the Intel iGPU: 1.74 ms/frame for 22,400 px (default stack, debug
  build) — huge headroom vs the 16.6 ms 60 fps budget.
- cpal 0.18: `Device::name()` is gone → `device.description()?.name()`; `SampleRate`
  is a plain `u32`; `build_input_stream` takes `StreamConfig` by value.

## Open questions for the user

1. Exact pixel count / density / minor radius — defaults chosen, all editable in
   Settings → Geometry. Update `default-config` when real numbers are known.
2. Show machine OS? CI builds Windows + Linux; add macOS on request.
3. Which public video providers matter in practice? Direct files and standards-based
   metadata work now; changing provider sites remain optional `yt-dlp` territory.

## Things not to do

- **Unbrokered builds.** Wrap every cargo/vite build in the compute-budget broker:
  `node ~/.claude/bin/cpu-slots.mjs run --slots 4 --label "empyrean cargo" -- cargo …`
  (2 slots for vite, 1 per spare app instance). Don't build while `tauri dev` runs —
  it also relinks the same exe (see the dev-app crash gotcha above).
- Cross-compiling on sentinel: evaluated 2026-08-19, declined — Tauri Linux→Windows
  cross builds are fragile; CI artifacts are the remote release builder; sccache (cache
  on sentinel) is the approved-if-wanted accelerator for cold local builds.

- No non-Vulkan wgpu backends, no CPU fallback renderer — error clearly instead.
- Don't default sACN output on.
- Don't add an installer/updater to CI — raw binary artifact only.
