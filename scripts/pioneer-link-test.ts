// End-to-end PRO DJ LINK rhythm test against a running backend.
// Replays receive-only beat/status UDP packets derived from an attributed public
// set-list fixture, verifies master handoffs and BPM, then restores the config.
// Usage: bun scripts/pioneer-link-test.ts [ws url]

import dgram from "node:dgram";

const wsUrl = process.argv[2] ?? "ws://127.0.0.1:9520/ws";
const fixture = await Bun.file(
  new URL("./fixtures/derek-plaslaiko-boiler-room-link.json", import.meta.url),
).json() as {
  title: string;
  tracks: Array<{ artist: string; title: string; simulated_bpm: number; deck: number }>;
};
const udp = dgram.createSocket("udp4");
const ws = new WebSocket(wsUrl);
let originalConfig: any;
let configured = false;
let replayStarted = false;
let lastMaster = 0;
let verified = 0;
let expected: (typeof fixture.tracks)[number] | null = null;

function fail(message: string): never {
  console.error(`PIONEER LINK FAIL: ${message}`);
  if (originalConfig && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: "set_config", config: originalConfig }));
  }
  udp.close();
  ws.close();
  process.exit(1);
}

function header(packet: Buffer, kind: number, deck: number) {
  packet.write("Qspt1WmJOL", 0, "ascii");
  packet[0x0a] = kind;
  packet.write(`CDJ-${deck}000`, 0x0b, "ascii");
  packet[0x21] = deck;
}

function statusPacket(deck: number, master: boolean, beat: number): Buffer {
  const packet = Buffer.alloc(0xd4);
  header(packet, 0x0a, deck);
  packet[0x89] = 0x40 | (master ? 0x20 : 0);
  packet.writeUInt32BE(beat, 0xa0);
  return packet;
}

function beatPacket(deck: number, bpm: number, beatInBar: number): Buffer {
  const packet = Buffer.alloc(0x60);
  header(packet, 0x28, deck);
  packet[0x55] = 0x10; // normal pitch: 0x100000
  packet.writeUInt16BE(Math.round(bpm * 100), 0x5a);
  packet[0x5c] = beatInBar;
  return packet;
}

function send(packet: Buffer, port: number) {
  udp.send(packet, port, "127.0.0.1");
}

async function replayTrack(track: (typeof fixture.tracks)[number], index: number) {
  if (lastMaster && lastMaster !== track.deck) {
    send(statusPacket(lastMaster, false, index * 16), 50002);
  }
  lastMaster = track.deck;
  expected = track;
  send(statusPacket(track.deck, true, index * 16), 50002);
  const period = 60_000 / track.simulated_bpm;
  for (let beat = 0; beat < 4; beat++) {
    send(statusPacket(track.deck, true, index * 16 + beat), 50002);
    send(beatPacket(track.deck, track.simulated_bpm, (beat % 4) + 1), 50001);
    await Bun.sleep(period);
  }
}

async function runReplay() {
  // The listener is intentionally started only when LINK is selected and retries
  // every two seconds, so allow one full bind interval.
  await Bun.sleep(2300);
  for (const [index, track] of fixture.tracks.slice(0, 3).entries()) {
    console.log(`replay: deck ${track.deck} · ${track.artist} — ${track.title}`);
    await replayTrack(track, index);
  }
  await Bun.sleep(700);
  if (verified < 3) fail(`only observed ${verified}/3 track clocks`);
  ws.send(JSON.stringify({ type: "set_config", config: originalConfig }));
  await Bun.sleep(150);
  udp.close();
  ws.close();
  console.log(`PIONEER LINK PASS: ${verified} master clocks from ${fixture.title}`);
  process.exit(0);
}

ws.onopen = () => {
  ws.send(JSON.stringify({ type: "hello", name: "pioneer-e2e", client_id: "pioneer-e2e", token: "" }));
  ws.send(JSON.stringify({ type: "get_state" }));
};

ws.onmessage = (event) => {
  if (typeof event.data !== "string") return;
  const message = JSON.parse(event.data);
  if (message.type === "state" && !configured) {
    originalConfig = structuredClone(message.config);
    const config = structuredClone(message.config);
    config.render.manual_bpm = null;
    config.rhythm = {
      ...config.rhythm,
      source: "pro_dj_link",
      pro_dj_link_player: 0,
      latency_ms: 0,
      fallback_to_audio: false,
    };
    ws.send(JSON.stringify({ type: "set_config", config }));
    configured = true;
    if (!replayStarted) {
      replayStarted = true;
      void runReplay();
    }
  }
  if ((message.type === "status" || message.type === "state") && expected) {
    const clock = message.status.rhythm;
    const devices = message.status.pro_dj_link_devices ?? [];
    if (
      clock?.active &&
      clock.source === "pro_dj_link" &&
      Math.abs(clock.bpm - expected.simulated_bpm) < 0.2 &&
      devices.some((device: { number: number }) => device.number === expected!.deck)
    ) {
      verified++;
      expected = null;
    }
  }
};

ws.onerror = () => fail(`cannot connect to ${wsUrl}`);
setTimeout(() => fail(`timeout; verified=${verified}`), 20_000);
