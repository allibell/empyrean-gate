//! External musical-clock inputs. Audio analysis owns energy/spectrum; this module
//! owns authoritative timing signals that can be overlaid on every layer.

use crate::config::RhythmSource;
use crate::state::SharedState;
use midir::{Ignore, MidiInput, MidiInputConnection};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const CLOCKS_PER_BEAT: f64 = 24.0;
const CLOCK_TIMEOUT: Duration = Duration::from_millis(500);
const RESCAN_EVERY: Duration = Duration::from_secs(2);
const PRO_DJ_LINK_MAGIC: &[u8; 10] = b"Qspt1WmJOL";
const PRO_DJ_LINK_BEAT_PORT: u16 = 50_001;
const PRO_DJ_LINK_STATUS_PORT: u16 = 50_002;

#[derive(Debug, Clone, Copy, Default)]
pub struct ClockSnapshot {
    pub usable: bool,
    pub running: bool,
    pub bpm: f32,
    pub beat_phase: f32,
    pub beat_count: u64,
    pub age_ms: f32,
}

/// MIDI callback state. Kept independent of the MIDI connection so the engine can
/// read a tiny snapshot without ever touching an OS MIDI API.
#[derive(Debug)]
pub struct MidiClockState {
    ticks: u64,
    bpm: f32,
    running: bool,
    transport_seen: bool,
    last_tick: Option<Instant>,
}

impl Default for MidiClockState {
    fn default() -> Self {
        Self {
            ticks: 0,
            bpm: 0.0,
            running: false,
            transport_seen: false,
            last_tick: None,
        }
    }
}

impl MidiClockState {
    fn message_at(&mut self, message: &[u8], now: Instant) {
        let Some(status) = message.first().copied() else {
            return;
        };
        match status {
            // Timing Clock: 24 pulses per quarter note.
            0xf8 => {
                let continuing = self.last_tick.is_some();
                if let Some(last) = self.last_tick {
                    let dt = now.duration_since(last).as_secs_f32();
                    let instant_bpm = 60.0 / (dt * CLOCKS_PER_BEAT as f32);
                    if (20.0..=400.0).contains(&instant_bpm) {
                        self.bpm = if self.bpm <= 0.0 {
                            instant_bpm
                        } else {
                            // Enough smoothing to reject USB scheduling jitter while
                            // still following a DJ tempo bend promptly.
                            self.bpm * 0.85 + instant_bpm * 0.15
                        };
                    }
                }
                self.last_tick = Some(now);
                // The first pulse after Start is the downbeat (tick zero), not
                // tick one. Subsequent pulses advance the 24-PPQN position.
                if continuing {
                    self.ticks = self.ticks.wrapping_add(1);
                }
                // Many clock senders omit transport messages. In that common case,
                // clock presence itself means running. An explicit Stop wins.
                if !self.transport_seen {
                    self.running = true;
                }
            }
            // Start / Continue / Stop.
            0xfa => {
                self.transport_seen = true;
                self.running = true;
                self.ticks = 0;
                self.last_tick = None;
            }
            0xfb => {
                self.transport_seen = true;
                self.running = true;
            }
            0xfc => {
                self.transport_seen = true;
                self.running = false;
            }
            // Song Position Pointer is counted in MIDI beats (six clocks).
            0xf2 if message.len() >= 3 => {
                let position = u16::from(message[1] & 0x7f) | (u16::from(message[2] & 0x7f) << 7);
                self.ticks = u64::from(position) * 6;
            }
            _ => {}
        }
    }

    pub fn snapshot(&self, now: Instant, latency_ms: f32) -> ClockSnapshot {
        let Some(last) = self.last_tick else {
            return ClockSnapshot::default();
        };
        let age = now.duration_since(last);
        let usable = age <= CLOCK_TIMEOUT && self.bpm > 0.0 && self.running;
        let offset_beats =
            (age.as_secs_f64() - f64::from(latency_ms) / 1000.0) * f64::from(self.bpm) / 60.0;
        let total_beats = self.ticks as f64 / CLOCKS_PER_BEAT + offset_beats;
        ClockSnapshot {
            usable,
            running: self.running,
            bpm: self.bpm,
            beat_phase: total_beats.rem_euclid(1.0) as f32,
            beat_count: total_beats.max(0.0).floor() as u64,
            age_ms: age.as_secs_f32() * 1000.0,
        }
    }

    fn disconnect(&mut self) {
        self.running = false;
        self.last_tick = None;
        self.bpm = 0.0;
        self.ticks = 0;
    }
}

#[derive(Debug, Clone)]
pub struct PioneerDevice {
    pub number: u8,
    pub name: String,
}

/// Receive-only PRO DJ LINK clock state. We deliberately do not announce a
/// virtual CDJ, claim a device number, or send sync/master commands.
#[derive(Debug, Default)]
pub struct PioneerClockState {
    bpm: f32,
    beat_count: u64,
    last_beat: Option<Instant>,
    player: u8,
    player_name: String,
    master_player: Option<u8>,
    devices: HashMap<u8, (String, Instant)>,
    beat_numbers: HashMap<u8, u64>,
    listen_error: String,
}

impl PioneerClockState {
    pub fn snapshot(&self, now: Instant, latency_ms: f32) -> ClockSnapshot {
        let Some(last) = self.last_beat else {
            return ClockSnapshot::default();
        };
        let age = now.duration_since(last);
        // Beat packets arrive once per beat, so the timeout must scale for slow
        // music. Two and a half periods tolerates a lost UDP packet without
        // abandoning the master deck during a mix.
        let timeout = Duration::from_secs_f32((150.0 / self.bpm.max(20.0)).clamp(0.75, 4.0));
        let usable = age <= timeout && self.bpm > 0.0;
        let offset_beats =
            (age.as_secs_f64() - f64::from(latency_ms) / 1000.0) * f64::from(self.bpm) / 60.0;
        let total_beats = self.beat_count as f64 + offset_beats;
        ClockSnapshot {
            usable,
            running: usable,
            bpm: self.bpm,
            beat_phase: total_beats.rem_euclid(1.0) as f32,
            beat_count: total_beats.max(0.0).floor() as u64,
            age_ms: age.as_secs_f32() * 1000.0,
        }
    }

    pub fn player_label(&self) -> String {
        if self.player == 0 {
            String::new()
        } else if self.player_name.is_empty() {
            format!("player {}", self.player)
        } else {
            format!("{} · player {}", self.player_name, self.player)
        }
    }

    pub fn devices(&self, now: Instant) -> Vec<PioneerDevice> {
        let mut out: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, (_, seen))| now.duration_since(*seen) < Duration::from_secs(10))
            .map(|(number, (name, _))| PioneerDevice {
                number: *number,
                name: name.clone(),
            })
            .collect();
        out.sort_by_key(|d| d.number);
        out
    }

    pub fn listen_error(&self) -> &str {
        &self.listen_error
    }

    fn set_listen_error(&mut self, error: String) {
        self.listen_error = error;
    }

    fn disconnect(&mut self) {
        self.bpm = 0.0;
        self.last_beat = None;
        self.player = 0;
        self.player_name.clear();
        self.master_player = None;
        self.listen_error.clear();
    }

    fn receive_status(&mut self, packet: &[u8], now: Instant) {
        if !valid_link_packet(packet, 0x0a) || packet.len() < 0xa7 {
            return;
        }
        let player = packet[0x21];
        let name = link_name(packet);
        self.devices.insert(player, (name, now));
        let flags = packet[0x89];
        if flags & 0x20 != 0 {
            self.master_player = Some(player);
        } else if self.master_player == Some(player) {
            self.master_player = None;
        }
        let raw_beat = u32::from_be_bytes(packet[0xa0..0xa4].try_into().unwrap());
        if raw_beat != u32::MAX {
            self.beat_numbers.insert(player, u64::from(raw_beat));
        }
    }

    fn receive_beat(&mut self, packet: &[u8], configured_player: u8, now: Instant) {
        let Some(beat) = parse_link_beat(packet) else {
            return;
        };
        self.devices.insert(beat.player, (beat.name.clone(), now));
        let wanted = if configured_player > 0 {
            Some(configured_player)
        } else {
            self.master_player
        };
        if wanted.is_some_and(|player| player != beat.player) {
            return;
        }
        // Without visible master status, stay on the current deck while its beats
        // remain healthy instead of flip-flopping during a two-deck crossfade.
        if wanted.is_none()
            && self.player != 0
            && self.player != beat.player
            && self
                .last_beat
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(1200))
        {
            return;
        }
        let changed_player = self.player != beat.player;
        self.player = beat.player;
        self.player_name = beat.name;
        self.bpm = beat.bpm;
        self.last_beat = Some(now);
        if let Some(number) = self.beat_numbers.get(&beat.player).copied() {
            self.beat_count = number;
        } else if changed_player && (1..=4).contains(&beat.beat_within_bar) {
            let bar_base = self.beat_count.saturating_sub(self.beat_count % 4);
            self.beat_count = bar_base + u64::from(beat.beat_within_bar - 1);
        } else {
            self.beat_count = self.beat_count.wrapping_add(1);
        }
    }
}

struct LinkBeat {
    player: u8,
    name: String,
    bpm: f32,
    beat_within_bar: u8,
}

fn valid_link_packet(packet: &[u8], kind: u8) -> bool {
    packet.len() > 0x0a && &packet[..10] == PRO_DJ_LINK_MAGIC && packet[0x0a] == kind
}

fn link_name(packet: &[u8]) -> String {
    let end = packet[0x0b..0x1f]
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(20);
    String::from_utf8_lossy(&packet[0x0b..0x0b + end]).into_owned()
}

fn parse_link_beat(packet: &[u8]) -> Option<LinkBeat> {
    if !valid_link_packet(packet, 0x28) || packet.len() < 0x60 {
        return None;
    }
    let raw_pitch = u32::from_be_bytes([0, packet[0x55], packet[0x56], packet[0x57]]);
    let base_bpm = f32::from(u16::from_be_bytes([packet[0x5a], packet[0x5b]])) / 100.0;
    let bpm = base_bpm * raw_pitch as f32 / 0x10_0000 as f32;
    if !(20.0..=400.0).contains(&bpm) {
        return None;
    }
    Some(LinkBeat {
        player: packet[0x21],
        name: link_name(packet),
        bpm,
        beat_within_bar: packet[0x5c],
    })
}

pub fn spawn(state: Arc<SharedState>) -> std::thread::JoinHandle<()> {
    let pioneer_state = state.clone();
    std::thread::Builder::new()
        .name("pro-dj-link".into())
        .spawn(move || pioneer_thread(pioneer_state))
        .expect("spawn PRO DJ LINK thread");
    std::thread::Builder::new()
        .name("midi-clock".into())
        .spawn(move || midi_thread(state))
        .expect("spawn MIDI clock thread")
}

fn pioneer_thread(state: Arc<SharedState>) {
    let mut beat_socket: Option<UdpSocket> = None;
    let mut status_socket: Option<UdpSocket> = None;
    let mut last_bind_attempt = Instant::now() - RESCAN_EVERY;
    let mut buffer = [0u8; 2048];

    while !state.shutdown.load(Ordering::Relaxed) {
        let (enabled, configured_player) = {
            let cfg = state.config.read();
            (
                cfg.rhythm.source == RhythmSource::ProDjLink,
                cfg.rhythm.pro_dj_link_player,
            )
        };
        if !enabled {
            beat_socket = None;
            status_socket = None;
            state.pioneer_clock.lock().disconnect();
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        if (beat_socket.is_none() || status_socket.is_none())
            && last_bind_attempt.elapsed() >= RESCAN_EVERY
        {
            last_bind_attempt = Instant::now();
            if beat_socket.is_none() {
                match bind_link_socket(PRO_DJ_LINK_BEAT_PORT) {
                    Ok(socket) => {
                        beat_socket = Some(socket);
                        state.pioneer_clock.lock().set_listen_error(String::new());
                        log::info!("passively listening for PRO DJ LINK beats on UDP 50001");
                    }
                    Err(e) => state
                        .pioneer_clock
                        .lock()
                        .set_listen_error(format!("cannot listen on UDP 50001: {e}")),
                }
            }
            if status_socket.is_none() {
                match bind_link_socket(PRO_DJ_LINK_STATUS_PORT) {
                    Ok(socket) => status_socket = Some(socket),
                    Err(e) => log::warn!(
                        "PRO DJ LINK status port 50002 unavailable ({e}); beat input still works, select a player number if auto-master cannot be seen"
                    ),
                }
            }
        }

        if let Some(socket) = beat_socket.as_ref() {
            loop {
                match socket.recv_from(&mut buffer) {
                    Ok((length, _)) => state.pioneer_clock.lock().receive_beat(
                        &buffer[..length],
                        configured_player,
                        Instant::now(),
                    ),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        state
                            .pioneer_clock
                            .lock()
                            .set_listen_error(format!("UDP 50001 receive failed: {e}"));
                        beat_socket = None;
                        break;
                    }
                }
            }
        }
        if let Some(socket) = status_socket.as_ref() {
            loop {
                match socket.recv_from(&mut buffer) {
                    Ok((length, _)) => state
                        .pioneer_clock
                        .lock()
                        .receive_status(&buffer[..length], Instant::now()),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        status_socket = None;
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn bind_link_socket(port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

fn midi_thread(state: Arc<SharedState>) {
    let mut connection: Option<MidiInputConnection<()>> = None;
    let mut connected_name: Option<String> = None;
    let mut last_scan = Instant::now() - RESCAN_EVERY;

    while !state.shutdown.load(Ordering::Relaxed) {
        if last_scan.elapsed() < RESCAN_EVERY {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        last_scan = Instant::now();

        let desired = {
            let cfg = state.config.read();
            (cfg.rhythm.source, cfg.rhythm.midi_port.clone())
        };
        let (mut input, ports) = match enumerate_ports() {
            Ok(v) => v,
            Err(e) => {
                state.status.lock().midi_ports.clear();
                log::warn!("cannot enumerate MIDI inputs: {e}");
                continue;
            }
        };
        state.status.lock().midi_ports = ports.iter().map(|(name, _)| name.clone()).collect();

        let wanted_name = if desired.0 == RhythmSource::MidiClock {
            desired.1
        } else {
            None
        };
        let still_present = connected_name.as_ref().is_some_and(|name| {
            wanted_name.as_ref() == Some(name) && ports.iter().any(|p| &p.0 == name)
        });
        if connection.is_some() && !still_present {
            connection = None;
            connected_name = None;
            state.midi_clock.lock().disconnect();
        }
        if connection.is_some() {
            continue;
        }

        let Some(wanted) = wanted_name else {
            continue;
        };
        let Some((_, port)) = ports.into_iter().find(|(name, _)| *name == wanted) else {
            continue;
        };
        input.ignore(Ignore::None);
        let callback_state = state.clone();
        match input.connect(
            &port,
            "empyrean-gate-clock",
            move |_stamp, message, _| {
                callback_state
                    .midi_clock
                    .lock()
                    .message_at(message, Instant::now());
            },
            (),
        ) {
            Ok(c) => {
                log::info!("MIDI clock connected to '{wanted}'");
                connection = Some(c);
                connected_name = Some(wanted);
            }
            Err(e) => log::warn!("cannot connect MIDI input '{wanted}': {e}"),
        }
    }
}

fn enumerate_ports() -> Result<(MidiInput, Vec<(String, midir::MidiInputPort)>), midir::InitError> {
    let input = MidiInput::new("Empyrean Gate")?;
    let ports = input
        .ports()
        .into_iter()
        .map(|port| {
            let name = input
                .port_name(&port)
                .unwrap_or_else(|_| "Unknown MIDI input".into());
            (name, port)
        })
        .collect();
    Ok((input, ports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_beat(player: u8, bpm: f32, beat_in_bar: u8) -> Vec<u8> {
        let mut packet = vec![0u8; 0x60];
        packet[..10].copy_from_slice(PRO_DJ_LINK_MAGIC);
        packet[0x0a] = 0x28;
        packet[0x0b..0x13].copy_from_slice(b"CDJ-TEST");
        packet[0x21] = player;
        packet[0x55..0x58].copy_from_slice(&[0x10, 0x00, 0x00]);
        packet[0x5a..0x5c].copy_from_slice(&((bpm * 100.0) as u16).to_be_bytes());
        packet[0x5c] = beat_in_bar;
        packet
    }

    fn link_status(player: u8, master: bool, beat_number: u32) -> Vec<u8> {
        let mut packet = vec![0u8; 0xd4];
        packet[..10].copy_from_slice(PRO_DJ_LINK_MAGIC);
        packet[0x0a] = 0x0a;
        packet[0x0b..0x13].copy_from_slice(b"CDJ-TEST");
        packet[0x21] = player;
        packet[0x89] = if master { 0x60 } else { 0x40 };
        packet[0xa0..0xa4].copy_from_slice(&beat_number.to_be_bytes());
        packet
    }

    #[test]
    fn clock_estimates_tempo_and_phase() {
        let start = Instant::now();
        let mut clock = MidiClockState::default();
        clock.message_at(&[0xfa], start);
        for tick in 0..48 {
            clock.message_at(&[0xf8], start + Duration::from_secs_f64(tick as f64 / 48.0));
        }
        let snap = clock.snapshot(start + Duration::from_secs(1), 0.0);
        assert!(snap.usable);
        assert!((snap.bpm - 120.0).abs() < 0.1, "{}", snap.bpm);
        assert!(
            snap.beat_phase < 0.06 || snap.beat_phase > 0.94,
            "{}",
            snap.beat_phase
        );
    }

    #[test]
    fn positive_latency_delays_the_visual_wrap() {
        let start = Instant::now();
        let mut clock = MidiClockState::default();
        clock.bpm = 120.0;
        clock.running = true;
        clock.last_tick = Some(start);
        let snap = clock.snapshot(start, 50.0);
        assert!((snap.beat_phase - 0.9).abs() < 0.001);
    }

    #[test]
    fn explicit_stop_makes_clock_unusable() {
        let start = Instant::now();
        let mut clock = MidiClockState::default();
        clock.bpm = 120.0;
        clock.running = true;
        clock.last_tick = Some(start);
        clock.message_at(&[0xfc], start);
        assert!(!clock.snapshot(start, 0.0).usable);
    }

    #[test]
    fn pioneer_beat_parser_applies_pitch() {
        let mut packet = link_beat(2, 120.0, 3);
        packet[0x55..0x58].copy_from_slice(&[0x11, 0x00, 0x00]); // +6.25%
        let beat = parse_link_beat(&packet).unwrap();
        assert_eq!(beat.player, 2);
        assert!((beat.bpm - 127.5).abs() < 0.01);
        assert_eq!(beat.beat_within_bar, 3);
    }

    #[test]
    fn pioneer_auto_follows_reported_master_and_handoff() {
        let start = Instant::now();
        let mut clock = PioneerClockState::default();
        clock.receive_status(&link_status(1, true, 100), start);
        clock.receive_beat(&link_beat(2, 130.0, 1), 0, start);
        assert!(!clock.snapshot(start, 0.0).usable, "non-master ignored");
        clock.receive_beat(&link_beat(1, 120.0, 1), 0, start);
        assert_eq!(clock.player, 1);
        assert_eq!(clock.beat_count, 100);

        clock.receive_status(&link_status(1, false, 101), start);
        clock.receive_status(&link_status(2, true, 44), start);
        clock.receive_beat(&link_beat(2, 130.0, 1), 0, start);
        assert_eq!(clock.player, 2);
        assert!((clock.bpm - 130.0).abs() < 0.01);
        assert_eq!(clock.beat_count, 44);
    }

    #[test]
    fn pioneer_player_override_works_without_master_status() {
        let start = Instant::now();
        let mut clock = PioneerClockState::default();
        clock.receive_beat(&link_beat(1, 120.0, 1), 2, start);
        clock.receive_beat(&link_beat(2, 128.0, 1), 2, start);
        assert_eq!(clock.player, 2);
        assert!((clock.snapshot(start, 0.0).bpm - 128.0).abs() < 0.01);
    }
}
