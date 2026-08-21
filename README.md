# Empyrean Gate

GPU pattern generator and sACN pixel driver for the **Empyrean Gate** — a radial array
of lights above a dance floor: 64 spokes of LED strip (~350 px each, fed from the
outside) in a 50 ft diameter ring, driven by 16× Advatek PixLite Mk4-S controllers over
sACN (E1.31).

Every frame is computed from scratch on the GPU (wgpu locked to **Vulkan** — no
fallback renderers, just clear errors), read back, and scattered into prebuilt sACN
packets with zero steady-state allocations. Audio from the DJ (multiple parallel
sources) drives the patterns via beat tracking and band energies.

## Screenshots

![Live tab — the array with pens and effect pads beside it](docs/live-wide.png)

| Squarish window — corner controls | Phone / portrait | Control | Settings |
|---|---|---|---|
| ![Live tab in a square window with controls in the corners](docs/live-square.png) | ![Live tab in portrait](docs/live-tall.png) | ![Control tab](docs/control.png) | ![Settings tab](docs/settings.png) |

The Live surface adapts to the window: the array view stays as large as possible and
the controls flow into whatever space is left — side columns, top/bottom bars, or the
corners the circle never reaches. The empty ring center carries the title, beat, and
live meters.

## Architecture

- **Backend is the app.** Frame generation runs on a dedicated thread:
  GPU compute → readback (ping-pong staging, no stalls) → sACN + preview fan-out.
  Kill every UI and the lights keep running.
- **Every UI is a WebSocket client.** The backend serves the web UI (embedded in the
  binary) plus a JSON + binary protocol on port 9520. The Tauri desktop window, LAN
  browsers, and phones all speak the same protocol.
- **Remote inputs**: a phone on the LAN can contribute its microphone (features
  extracted client-side, same beat tracker as local sources) and its IMU orientation
  (steers layers/effects). See Settings → This device.
- **Layers**: noise fields (3D simplex / multidimensional color noise), harmonic radial
  waves, spirals, plasma, spoke chases, sparkles, beat rings, breathing envelopes,
  rainbows, wedges, interference, fire, meteors, warp — plus MilkDrop-style raw-audio
  layers: **Waveform** (the PCM bent into a circular oscilloscope) and **Spectrum**
  (spoke-per-bin circular analyzer), plus **Video** (live browser-decoded texture,
  radial/kaleidoscope mapping and color treatment) —
  stacked with blend modes, each bound to an audio source. Effects (burst / strobe /
  swoosh / collapse) fire from keyboard (1–4), clicks/taps on the preview, or remote
  clients.
- **Four UI tabs**, deep-linkable by hash: Live (stage monitor + drawing), Video
  (URL/file intake), Control (touch-sized effect pads + master/layer faders), and Settings. In the desktop app,
  "New window" pops the current tab out into its own window. Old `/#view` and `/#draw`
  links redirect to Live.
- **Live drawing**: paint on the array from any client with Glow / Ripple / Sparkle
  pens (color swatches + size). Strokes stream as polar dabs over WS and render on the
  GPU with ~2 s trails; multiple people can draw at once.
- **PWA**: open the web UI on an iPad/phone, "Add to Home Screen", and it runs
  standalone fullscreen — a touch control surface for the floor. Manifest shortcuts
  jump straight to Draw or Control.
- **Video intake**: paste a public Instagram post/Reel URL, a direct MP4/WebM URL,
  or a publisher page with standard
  `og:video` / HTML video metadata, or choose a file on the iPad. The browser uses
  its native hardware decoder and sends a bounded 64–128 px RGBA texture at 10–24
  fps; the backend retains only the latest frame, so congestion drops frames instead
  of adding latency. A Video layer maps it across the radial array with zoom,
  kaleidoscope, contrast, rotation, color treatment, blend, audio, and autopilot
  controls. Its rhythm source can be the decoded video's own soundtrack or any
  configured live Gate input; soundtrack analysis sends only compact features and
  can stay silent on the control device. If current `yt-dlp` plus a supported
  JavaScript runtime is installed on the Gate machine, provider pages such as public
  YouTube and Instagram videos get an additional best-effort resolver.
  DRM/login-gated sources remain unsupported.
- **Autopilot**: a slow mean-reverting random walk drifts layer parameters around
  wherever the sliders are set (per-layer "Walk" amount = wander radius), so an
  unattended show evolves for hours without repeating.
- **Audio loopback**: pick a system *output* device as a source (WASAPI loopback) —
  music played on the show machine drives the beat with no cabling.
- **Audio hardware can come and go.** A missing or unplugged device never crashes or
  degrades the show: the source goes quiet (visuals decay calmly), reports "waiting
  for device", and is retried every 2 s until *that* device returns — a selected
  device is never silently substituted. The one automatic change: sources set to
  "system default" follow the OS default device when Windows changes it. Hot-plugged
  devices appear in the pickers within a few seconds.
- **Connect QR + client management**: ⊕ Connect in the top bar shows a QR (per
  interface) that joins a phone/iPad straight to the web UI. Devices get persistent
  ids and friendly names; Settings → Clients lets you rename, revoke (kicks live,
  blocks rejoin), and optionally require the join token so only QR-scanned devices
  can connect (rotate the token to lock everyone new out).
- **Seamless takeover**: start a new backend while one is running and it warms its
  GPU first, asks the old instance to stop and hand over its running state (config +
  layer animation phases), then continues the output — the structure sees a sub-second
  hold, no blackout, and patterns don't jump. Deploying a new build mid-show is just
  "start the new binary".
- **sACN**: pick the egress interface explicitly (multi-homed machines otherwise send
  multicast out the default route — invisible on the lighting NIC), sync sACN to
  render fps or fix a rate, and optionally enable E1.31 universe synchronization
  (PixLite Mk4 latches all universes per sync packet, tear-free). Live packets/s in
  the status HUD tells you it's actually transmitting.
  Multicast and controller unicast are exclusive destination modes, so a configured
  receiver never gets duplicate sequence-identical packets.
- **A well-behaved sACN source.** The CID (source identity) is generated once and
  persisted, as the spec requires — so restarts and handovers look like the *same*
  source instead of a second one fighting the first in every receiver's merge for
  2.5 s. Streams are closed with E1.31 termination packets when output is switched
  off or the app exits, rather than leaving the rig holding its last frame until the
  receivers time out. The universe list is advertised on the discovery universe every
  10 s, so the source shows up in sACNView and controller UIs. Source name is
  configurable.

```
src/                React UI (preview + settings), WebGL2 preview, sensors
src-tauri/src/
  engine/           wgpu Vulkan engine + WGSL layer shader (hot-reloads in dev)
  audio/            cpal capture (per-source channel select) + FFT features + beat tracker
  sacn.rs           allocation-free E1.31 sender (prebuilt per-universe packets)
  server.rs         axum HTTP + WS (serves UI, speaks the protocol)
  media.rs          guarded URL resolver + ranged same-origin media proxy
  config.rs         geometry / output / audio / layers, persisted JSON
```

## Development

Requirements: [Rust](https://rustup.rs), [Bun](https://bun.sh), a Vulkan-capable GPU +
driver. A current [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) installation with a
supported external JavaScript runtime (Node works) is optional for resolving provider
pages; direct media URLs, metadata pages, and local device files do not need it.

```sh
bun install
bun tauri dev          # desktop app w/ vite dev server (hot reload, shader hot-reload)
```

Useful during pattern development:

- Edit `src-tauri/src/engine/shaders/gate.wgsl` while the app runs — the pipeline
  rebuilds on save.
- `cargo run --bin engine-smoke` (in `src-tauri/`) — quick headless correctness +
  timing check, no window.
- `cargo run --release --bin engine-smoke -- --suite --warmup 120 --frames 600`
  — repeatable GPU regression suite at the 24,192-pixel installation size plus
  a 70k heavy-load headroom case. Add
  `--json` for a versioned machine-readable report, or use `--pixels`, `--layers`,
  `--effects`, and `--dabs` to define one workload. Reports mean, p50/p95/p99/max,
  standard deviation, throughput, and missed frames against `--fps-budget`.
- `cargo run -- --headless` — full backend without the desktop window; open
  `http://localhost:9520` (or from a phone on the LAN).
- `bun run demo:uprising` — optional convenience: use authenticated GitHub access to
  fetch the small **Warm Windstorm** clip referenced by the saved 2024 show state. It
  lands in ignored `demo-data/uprising/`. While Vite is running, open `/#replay` to use
  the development-only frame fixture viewer; it is omitted from production navigation
  and builds. Other testers can choose a clip from their own `Uprising-Data` checkout.
- `bun scripts/e2e-test.ts` — protocol smoke test against a running backend.
  It also sends a generated video texture and verifies live source status.

## Production build

```sh
bun install
bun tauri build --no-bundle
# → src-tauri/target/release/empyrean-gate(.exe)  — standalone, UI embedded
```

CI (GitHub Actions) builds Windows, Linux, and macOS binaries on every push. The same
binary runs the desktop app or `--headless` for show machines.

## Releases

Releases are cut by CI only — push a version tag and the Release workflow runs the
check suite, builds all targets, and publishes a GitHub Release with the standalone
binaries attached:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

Grab binaries from https://github.com/cinderblock/empyrean-gate/releases.

## Self-update

No installer needed. The app checks GitHub Releases (startup + every 6 h; toggle in
Settings → Updates) and shows a lit version chip in the top bar when a newer release
exists — click it (or use Settings) to update: the new binary downloads *beside* the
running one, launches, and takes over via the seamless handover. Mid-show updates
cost about one frame. Auto-install is available but off by default; old versioned
binaries are cleaned up automatically. (Instances older than v0.2.0 predate the
updater and need one manual swap.)

## Safety notes

- **sACN output is OFF by default** — enable it in Settings → sACN output. 192
  universes at 60 fps is real traffic; unicast to controller IPs is preferred over
  multicast.
- Everything about the geometry (spoke count, pixels, radii, universe layout) is
  config, editable live in Settings and persisted to the user config dir.
- Remote media fetching accepts only HTTP(S), rejects credentials and local/private/
  reserved destinations, pins each connection to its validated DNS answers,
  bypasses system proxies, revalidates redirects, caps inspected HTML and live proxy
  sessions, and exposes streams through short-lived opaque URLs. This prevents the
  feature becoming an unauthenticated proxy into the show/control network.

## Not yet

- Bundled extraction for changing provider sites; optional `yt-dlp` is best-effort,
  and DRM/login-gated video is intentionally out of scope.
- Batched UDP I/O (`sendmmsg`/RIO) for 100k+ pixel scales.
