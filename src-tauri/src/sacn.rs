//! Minimal, allocation-free sACN (ANSI E1.31) sender. Hand-rolled: the packet format
//! is small, stable, and well specified; this avoids an external protocol dependency.
//!
//! Efficiency: getting frames from the GPU onto the wire is a primary feature, so all
//! packets are prebuilt once per configuration — headers, destinations, and slot
//! offsets — and each frame only (1) LUT-copies pixel bytes straight into the resident
//! packet buffers, (2) bumps sequence numbers, (3) calls send_to. Zero heap traffic in
//! the steady state.
//!
//! Multi-homed machines: the socket binds to `output.interface` (when set) and that
//! address is also installed as the IPv4 multicast egress interface — otherwise
//! Windows/Linux send multicast out the default-route NIC, which is usually NOT the
//! lighting network.
//!
//! Frame coherence: when `output.sync_universe` != 0, every data packet carries that
//! sync address and one E1.31 synchronization packet is sent per frame; receivers that
//! support universe sync (PixLite Mk4 does) hold data and latch all universes at once.
//!
//! Source lifecycle: the CID comes from config and is persistent (see
//! `OutputConfig::cid`), so restarts and instance handovers look like the *same*
//! source to every receiver. When a stream ends deliberately — output switched off,
//! app exit, or universes dropped by a reconfigure — it is closed with E1.31
//! stream-termination packets rather than left to the receiver's 2.5 s source-loss
//! timeout, which would otherwise hold the last frame lit.
//!
//! Discovery: while transmitting, the universe list is advertised on the E1.31
//! discovery universe every 10 s, so sACNView and controller UIs can see this source.
//!
//! Universe layout: each spoke occupies `universes_per_spoke` consecutive universes
//! starting at `start_universe + spoke * universes_per_spoke`, `pixels_per_universe`
//! RGB pixels per universe, channel 1 = red of the spoke's outermost pixel.

use crate::config::{GeometryConfig, OutputConfig};
use crate::geometry;
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

const SACN_PORT: u16 = 5568;
const ACN_ID: [u8; 12] = *b"ASC-E1.17\0\0\0";
/// Offset of the sync-address field within a data packet's framing layer.
const SYNC_ADDR_OFFSET: usize = 109;
/// Offset of the sequence-number byte within a data packet.
const SEQ_OFFSET: usize = 111;
/// Offset of the options byte within a data packet (bit 6 = Stream_Terminated).
const OPTIONS_OFFSET: usize = 112;
/// Offset of the first DMX slot (after the start code).
const SLOTS_OFFSET: usize = 126;
/// Offset of the sequence byte within a sync packet.
const SYNC_PKT_SEQ_OFFSET: usize = 44;
/// Options bit 6: this is the last packet of the stream for this universe.
const OPT_STREAM_TERMINATED: u8 = 0x40;
/// E1.31 spec: a terminating source sends three such packets per universe.
const TERMINATE_REPEATS: usize = 3;
/// Reserved universe for universe-discovery packets (239.255.250.214).
const DISCOVERY_UNIVERSE: u16 = 64214;
/// E131_UNIVERSE_DISCOVERY_INTERVAL.
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(10);

fn multicast_group(universe: u16) -> Ipv4Addr {
    Ipv4Addr::new(239, 255, (universe >> 8) as u8, (universe & 0xff) as u8)
}

struct UniversePlan {
    universe: u16,
    /// Prebuilt packet: headers + start code; slots filled per frame.
    packet: Vec<u8>,
    /// Byte offset of this universe's pixel data within the frame RGB buffer.
    src_offset: usize,
    /// Number of pixel bytes carried by this universe.
    src_len: usize,
    unicast: Option<SocketAddrV4>,
    multicast: Option<SocketAddrV4>,
    sequence: u8,
}

pub struct SacnSender {
    socket: UdpSocket,
    bound_interface: String,
    /// Set when binding the configured interface failed (e.g. the saved IP
    /// belongs to another network). Sending continues on the previous socket,
    /// which may be the wrong NIC — the UI must surface this, not just the log.
    pub bind_error: Option<String>,
    cid: [u8; 16],
    source_name: [u8; 64],
    plan: Vec<UniversePlan>,
    /// Prebuilt E1.31 sync packet + its destinations; None when sync is disabled.
    sync: Option<(Vec<u8>, Vec<SocketAddrV4>)>,
    sync_sequence: u8,
    /// Prebuilt universe-discovery pages + destination; None when discovery is off.
    discovery: Option<(Vec<Vec<u8>>, SocketAddrV4)>,
    next_discovery: Instant,
    /// True between the first data packet and its stream termination. Gates the
    /// terminate paths so we never announce the end of a stream we never started.
    streaming: bool,
    gamma_lut: [u8; 256],
    lut_gamma: f32,
    send_errors: u64,
}

fn make_socket(interface: &str) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let iface: Ipv4Addr = interface.parse().unwrap_or(Ipv4Addr::UNSPECIFIED);
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.bind(&SocketAddrV4::new(iface, 0).into())?;
    // Route multicast out the chosen NIC instead of the default-route one.
    socket.set_multicast_if_v4(&iface)?;
    socket.set_multicast_ttl_v4(1)?;
    Ok(socket.into())
}

impl SacnSender {
    pub fn new() -> std::io::Result<Self> {
        // Identity comes from config; `configure` always runs before any send.
        Ok(Self {
            socket: make_socket("")?,
            bound_interface: String::new(),
            bind_error: None,
            cid: [0; 16],
            source_name: [0; 64],
            plan: Vec::new(),
            sync: None,
            sync_sequence: 0,
            discovery: None,
            next_discovery: Instant::now(),
            streaming: false,
            gamma_lut: [0; 256],
            lut_gamma: 0.0,
            send_errors: 0,
        })
    }

    /// (Re)build the socket, packet templates, and destinations. Call on config
    /// changes, not per frame.
    pub fn configure(&mut self, geo: &GeometryConfig, out: &OutputConfig) {
        let cid = cid_bytes(&out.cid);
        let source_name = pad_source_name(&out.source_name);

        let mut plan = Vec::new();
        let ppu = out.pixels_per_universe.max(1) as u32;
        let ups = geometry::universes_per_spoke(geo, out) as u32;

        for spoke in 0..geo.spokes {
            // Destination mode is exclusive. The old UI exposed multicast and
            // controller IPs independently, which sent every packet twice when
            // both were configured (same sequence number, wasted wire traffic).
            let unicast = (!out.multicast)
                .then(|| geometry::controller_for_spoke(out, spoke))
                .flatten()
                .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
                .map(|ip| SocketAddrV4::new(ip, SACN_PORT));
            for u in 0..ups {
                let first_pixel = u * ppu;
                if first_pixel >= geo.pixels_per_spoke {
                    break;
                }
                let count = ppu.min(geo.pixels_per_spoke - first_pixel);
                let universe = out.start_universe + (spoke * ups + u) as u16;
                let multicast = out
                    .multicast
                    .then(|| SocketAddrV4::new(multicast_group(universe), SACN_PORT));
                plan.push(UniversePlan {
                    universe,
                    packet: build_data_template(
                        &cid,
                        &source_name,
                        out.priority,
                        out.sync_universe,
                        universe,
                        (count * 3) as usize,
                    ),
                    src_offset: ((spoke * geo.pixels_per_spoke + first_pixel) * 3) as usize,
                    src_len: (count * 3) as usize,
                    unicast,
                    multicast,
                    sequence: 0,
                });
            }
        }

        // Universes the new plan drops (smaller geometry, moved start universe, or a
        // changed CID — which retires the whole old identity) must be told the stream
        // ended. Otherwise the receiver holds their last frame lit through the source-
        // loss timeout and, on hold-last-look controllers, indefinitely after that.
        // Sent on the OLD socket, before any rebind, so they leave the same NIC the
        // data did.
        if self.streaming {
            let keep: HashSet<u16> = if cid == self.cid {
                plan.iter().map(|p| p.universe).collect()
            } else {
                HashSet::new() // new identity: the old stream ends entirely
            };
            let (kept, mut stale): (Vec<UniversePlan>, Vec<UniversePlan>) =
                self.plan.drain(..).partition(|p| keep.contains(&p.universe));

            // Carry sequence numbers across the rebuild. Restarting at 0 makes a
            // receiver DROP the next packets whenever the old count was still low —
            // E1.31 discards a sequence delta in [-20, 0] as an out-of-order repeat.
            let carried: HashMap<u16, u8> =
                kept.iter().map(|p| (p.universe, p.sequence)).collect();
            for p in plan.iter_mut() {
                if let Some(&seq) = carried.get(&p.universe) {
                    p.sequence = seq;
                }
            }

            if !stale.is_empty() {
                log::info!("sACN: terminating {} universe(s) dropped by reconfigure", stale.len());
                terminate_plans(&self.socket, &mut stale);
            }
        }

        if out.interface != self.bound_interface {
            match make_socket(&out.interface) {
                Ok(s) => {
                    self.socket = s;
                    self.bound_interface = out.interface.clone();
                    self.bind_error = None;
                    log::info!(
                        "sACN socket bound to interface '{}'",
                        if out.interface.is_empty() { "default" } else { &out.interface }
                    );
                }
                Err(e) => {
                    let msg = format!(
                        "cannot bind interface '{}' ({e}) — sending via the default \
                         interface instead; re-pick the sACN interface in Settings",
                        out.interface
                    );
                    log::error!("sACN: {msg}");
                    self.bind_error = Some(msg);
                }
            }
        } else {
            self.bind_error = None;
        }

        self.cid = cid;
        self.source_name = source_name;
        self.plan = plan;

        self.discovery = out.discovery.then(|| {
            let mut universes: Vec<u16> = self.plan.iter().map(|p| p.universe).collect();
            universes.sort_unstable();
            universes.dedup();
            (
                build_discovery_pages(&self.cid, &self.source_name, &universes),
                SocketAddrV4::new(multicast_group(DISCOVERY_UNIVERSE), SACN_PORT),
            )
        });
        // Re-advertise promptly after a change rather than up to 10 s later.
        self.next_discovery = Instant::now();

        self.sync = (out.sync_universe != 0).then(|| {
            let mut dests = Vec::new();
            if out.multicast {
                dests.push(SocketAddrV4::new(multicast_group(out.sync_universe), SACN_PORT));
            }
            let mut controller_ips: Vec<SocketAddrV4> = self
                .plan
                .iter()
                .filter_map(|p| p.unicast)
                .collect();
            controller_ips.sort();
            controller_ips.dedup();
            dests.extend(controller_ips);
            (build_sync_template(&self.cid, out.sync_universe), dests)
        });

        if (self.lut_gamma - out.led_gamma).abs() > 1e-3 {
            for (i, v) in self.gamma_lut.iter_mut().enumerate() {
                let x = i as f32 / 255.0;
                *v = (x.powf(out.led_gamma) * 255.0 + 0.5) as u8;
            }
            self.lut_gamma = out.led_gamma;
        }
    }

    /// Send one full frame of perceptual RGB data (as produced by the engine).
    /// LED gamma is applied here, scattering directly into the resident packets;
    /// the preview shows the raw values. Individual send failures are counted, not
    /// fatal — one unreachable controller must not black out the rest.
    pub fn send_frame(&mut self, rgb: &[u8]) -> usize {
        let mut packets = 0usize;
        self.streaming = true;
        for plan in &mut self.plan {
            let Some(src) = rgb.get(plan.src_offset..plan.src_offset + plan.src_len) else {
                continue; // frame size changed mid-reconfigure; skip until re-plan
            };
            let dst = &mut plan.packet[SLOTS_OFFSET..SLOTS_OFFSET + plan.src_len];
            for (d, s) in dst.iter_mut().zip(src) {
                *d = self.gamma_lut[*s as usize];
            }
            plan.sequence = plan.sequence.wrapping_add(1);
            plan.packet[SEQ_OFFSET] = plan.sequence;

            for dest in [plan.unicast, plan.multicast].into_iter().flatten() {
                match self.socket.send_to(&plan.packet, dest) {
                    Ok(_) => packets += 1,
                    Err(e) => {
                        self.send_errors += 1;
                        if self.send_errors.is_power_of_two() {
                            log::warn!("sACN send to {dest} failed ({} total): {e}", self.send_errors);
                        }
                    }
                }
            }
        }

        if let Some((packet, dests)) = &mut self.sync {
            self.sync_sequence = self.sync_sequence.wrapping_add(1);
            packet[SYNC_PKT_SEQ_OFFSET] = self.sync_sequence;
            for dest in dests.iter() {
                if self.socket.send_to(packet, *dest).is_ok() {
                    packets += 1;
                }
            }
        }

        // Universe discovery rides along on the frame path — it is only ever due
        // while we are transmitting, and this keeps its cadence independent of the
        // frame rate. One `Instant::now()` per frame, no allocation: the pages are
        // prebuilt in `configure`.
        if let Some((pages, dest)) = &self.discovery {
            let now = Instant::now();
            if now >= self.next_discovery {
                self.next_discovery = now + DISCOVERY_INTERVAL;
                for page in pages {
                    if self.socket.send_to(page, *dest).is_ok() {
                        packets += 1;
                    }
                }
            }
        }
        packets
    }

    /// Close the stream deliberately: three data packets per universe with the
    /// Stream_Terminated option set, so receivers release the universe at once
    /// instead of holding the last frame for the 2.5 s source-loss timeout (and,
    /// on hold-last-look controllers, forever after that).
    ///
    /// Call on output disable and on app exit — but NOT on a handover, where the
    /// successor continues the same CID's stream and a termination would blink the
    /// rig between instances.
    pub fn send_terminate(&mut self) -> usize {
        if !self.streaming {
            return 0;
        }
        self.streaming = false;
        log::info!("sACN: terminating {} universe(s)", self.plan.len());
        terminate_plans(&self.socket, &mut self.plan)
    }

    pub fn universe_count(&self) -> u16 {
        self.plan.len() as u16
    }
}

/// Send stream-termination packets for `plans` and leave their templates clean, so
/// a universe that comes back later resumes as an ordinary stream.
fn terminate_plans(socket: &UdpSocket, plans: &mut [UniversePlan]) -> usize {
    let mut sent = 0usize;
    for _ in 0..TERMINATE_REPEATS {
        for plan in plans.iter_mut() {
            plan.packet[OPTIONS_OFFSET] = OPT_STREAM_TERMINATED;
            plan.sequence = plan.sequence.wrapping_add(1);
            plan.packet[SEQ_OFFSET] = plan.sequence;
            for dest in [plan.unicast, plan.multicast].into_iter().flatten() {
                if socket.send_to(&plan.packet, dest).is_ok() {
                    sent += 1;
                }
            }
        }
    }
    for plan in plans.iter_mut() {
        plan.packet[OPTIONS_OFFSET] = 0;
    }
    sent
}

fn build_data_template(
    cid: &[u8; 16],
    source_name: &[u8; 64],
    priority: u8,
    sync_universe: u16,
    universe: u16,
    data_len: usize,
) -> Vec<u8> {
    debug_assert!(data_len <= 512);
    let property_count = 1 + data_len as u16; // start code + slots
    let total = SLOTS_OFFSET + data_len;
    let mut p = Vec::with_capacity(total);

    // --- Root layer ---
    p.extend_from_slice(&0x0010u16.to_be_bytes()); // preamble size
    p.extend_from_slice(&0x0000u16.to_be_bytes()); // post-amble size
    p.extend_from_slice(&ACN_ID);
    p.extend_from_slice(&flags_len(total - 16));
    p.extend_from_slice(&0x0000_0004u32.to_be_bytes()); // VECTOR_ROOT_E131_DATA
    p.extend_from_slice(cid);

    // --- Framing layer ---
    p.extend_from_slice(&flags_len(total - 38));
    p.extend_from_slice(&0x0000_0002u32.to_be_bytes()); // VECTOR_E131_DATA_PACKET
    p.extend_from_slice(source_name);
    p.push(priority);
    debug_assert_eq!(p.len(), SYNC_ADDR_OFFSET);
    p.extend_from_slice(&sync_universe.to_be_bytes());
    debug_assert_eq!(p.len(), SEQ_OFFSET);
    p.push(0); // sequence, rewritten per frame
    p.push(0); // options
    p.extend_from_slice(&universe.to_be_bytes());

    // --- DMP layer ---
    p.extend_from_slice(&flags_len(total - 115));
    p.push(0x02); // VECTOR_DMP_SET_PROPERTY
    p.push(0xa1); // address & data type
    p.extend_from_slice(&0u16.to_be_bytes()); // first property address
    p.extend_from_slice(&1u16.to_be_bytes()); // address increment
    p.extend_from_slice(&property_count.to_be_bytes());
    p.push(0x00); // DMX start code
    debug_assert_eq!(p.len(), SLOTS_OFFSET);
    p.resize(total, 0);
    p
}

/// E1.31-2016 universe synchronization packet (49 bytes).
fn build_sync_template(cid: &[u8; 16], sync_universe: u16) -> Vec<u8> {
    let total = 49usize;
    let mut p = Vec::with_capacity(total);

    // --- Root layer ---
    p.extend_from_slice(&0x0010u16.to_be_bytes());
    p.extend_from_slice(&0x0000u16.to_be_bytes());
    p.extend_from_slice(&ACN_ID);
    p.extend_from_slice(&flags_len(total - 16));
    p.extend_from_slice(&0x0000_0008u32.to_be_bytes()); // VECTOR_ROOT_E131_EXTENDED
    p.extend_from_slice(cid);

    // --- Framing layer ---
    p.extend_from_slice(&flags_len(total - 38));
    p.extend_from_slice(&0x0000_0001u32.to_be_bytes()); // VECTOR_E131_EXTENDED_SYNCHRONIZATION
    debug_assert_eq!(p.len(), SYNC_PKT_SEQ_OFFSET);
    p.push(0); // sequence, rewritten per frame
    p.extend_from_slice(&sync_universe.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes()); // reserved
    debug_assert_eq!(p.len(), total);
    p
}

/// E1.31-2016 universe discovery packets: "this CID transmits these universes".
/// Up to 512 universes per page, ascending; every page carries the last page's
/// index so a receiver knows when the list is complete.
fn build_discovery_pages(
    cid: &[u8; 16],
    source_name: &[u8; 64],
    universes: &[u16],
) -> Vec<Vec<u8>> {
    const PER_PAGE: usize = 512;
    /// Offset of the universe list; also the size of a page carrying none.
    const LIST_OFFSET: usize = 120;

    let mut chunks: Vec<&[u16]> = universes.chunks(PER_PAGE).collect();
    if chunks.is_empty() {
        chunks.push(&[]); // "transmitting nothing" is still a valid advertisement
    }
    let last_page = (chunks.len() - 1) as u8;

    chunks
        .iter()
        .enumerate()
        .map(|(page, list)| {
            let total = LIST_OFFSET + list.len() * 2;
            let mut p = Vec::with_capacity(total);

            // --- Root layer ---
            p.extend_from_slice(&0x0010u16.to_be_bytes());
            p.extend_from_slice(&0x0000u16.to_be_bytes());
            p.extend_from_slice(&ACN_ID);
            p.extend_from_slice(&flags_len(total - 16));
            p.extend_from_slice(&0x0000_0008u32.to_be_bytes()); // VECTOR_ROOT_E131_EXTENDED
            p.extend_from_slice(cid);

            // --- Framing layer (discovery packets carry no sequence number) ---
            p.extend_from_slice(&flags_len(total - 38));
            p.extend_from_slice(&0x0000_0002u32.to_be_bytes()); // VECTOR_E131_EXTENDED_DISCOVERY
            p.extend_from_slice(source_name);
            p.extend_from_slice(&0u32.to_be_bytes()); // reserved

            // --- Universe discovery layer ---
            p.extend_from_slice(&flags_len(total - 112));
            p.extend_from_slice(&0x0000_0001u32.to_be_bytes()); // VECTOR_UNIVERSE_DISCOVERY_UNIVERSE_LIST
            p.push(page as u8);
            p.push(last_page);
            debug_assert_eq!(p.len(), LIST_OFFSET);
            for u in list.iter() {
                p.extend_from_slice(&u.to_be_bytes());
            }
            debug_assert_eq!(p.len(), total);
            p
        })
        .collect()
}

/// The configured CID as wire bytes (RFC 4122 network order). A malformed value is
/// loud but not fatal — we fall back to a fresh UUID so output still works, at the
/// cost of this run looking like a new source.
fn cid_bytes(s: &str) -> [u8; 16] {
    match uuid::Uuid::parse_str(s) {
        Ok(u) => *u.as_bytes(),
        Err(_) => {
            let u = uuid::Uuid::new_v4();
            log::error!(
                "sACN: configured CID '{s}' is not a UUID; using a temporary one ({u}). \
                 Fix output.cid in the config to keep a stable source identity."
            );
            *u.as_bytes()
        }
    }
}

/// 64-byte null-terminated UTF-8 source name, truncated on a char boundary so a
/// long name can never emit a mangled code point.
fn pad_source_name(name: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    let mut end = name.len().min(63);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&name.as_bytes()[..end]);
    out
}

fn flags_len(len: usize) -> [u8; 2] {
    (0x7000u16 | (len as u16 & 0x0fff)).to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(b: &[u8]) -> u16 {
        u16::from_be_bytes([b[0], b[1]])
    }
    fn be32(b: &[u8]) -> u32 {
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    const CID: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef,
    ];

    #[test]
    fn discovery_universe_maps_to_the_reserved_group() {
        assert_eq!(multicast_group(DISCOVERY_UNIVERSE), Ipv4Addr::new(239, 255, 250, 214));
        assert_eq!(multicast_group(1), Ipv4Addr::new(239, 255, 0, 1));
    }

    #[test]
    fn discovery_page_layout_matches_e131() {
        let name = pad_source_name("Empyrean Gate");
        let pages = build_discovery_pages(&CID, &name, &[1, 2, 300]);
        assert_eq!(pages.len(), 1);
        let p = &pages[0];

        assert_eq!(p.len(), 120 + 3 * 2);
        assert_eq!(be16(&p[0..2]), 0x0010); // preamble
        assert_eq!(be16(&p[2..4]), 0x0000); // post-amble
        assert_eq!(&p[4..16], &ACN_ID);
        assert_eq!(be16(&p[16..18]), 0x7000 | (p.len() - 16) as u16);
        assert_eq!(be32(&p[18..22]), 0x0000_0008); // VECTOR_ROOT_E131_EXTENDED
        assert_eq!(&p[22..38], &CID);

        assert_eq!(be16(&p[38..40]), 0x7000 | (p.len() - 38) as u16);
        assert_eq!(be32(&p[40..44]), 0x0000_0002); // VECTOR_E131_EXTENDED_DISCOVERY
        assert_eq!(&p[44..108], &name[..]);
        assert_eq!(&p[108..112], &[0, 0, 0, 0]); // reserved

        assert_eq!(be16(&p[112..114]), 0x7000 | (p.len() - 112) as u16);
        assert_eq!(be32(&p[114..118]), 0x0000_0001); // VECTOR_UNIVERSE_DISCOVERY_UNIVERSE_LIST
        assert_eq!(p[118], 0); // page
        assert_eq!(p[119], 0); // last page
        assert_eq!(be16(&p[120..122]), 1);
        assert_eq!(be16(&p[122..124]), 2);
        assert_eq!(be16(&p[124..126]), 300);
    }

    #[test]
    fn discovery_splits_into_pages_of_512() {
        let universes: Vec<u16> = (1..=600).collect();
        let pages = build_discovery_pages(&CID, &pad_source_name("x"), &universes);
        assert_eq!(pages.len(), 2);
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(p[118], i as u8, "page number");
            assert_eq!(p[119], 1, "last page index, same on every page");
        }
        assert_eq!(pages[0].len(), 120 + 512 * 2);
        assert_eq!(pages[1].len(), 120 + 88 * 2);
        // Page 2 continues where page 1 stopped.
        assert_eq!(be16(&pages[1][120..122]), 513);
    }

    #[test]
    fn a_source_with_no_universes_still_advertises() {
        let pages = build_discovery_pages(&CID, &pad_source_name("x"), &[]);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].len(), 120);
    }

    #[test]
    fn terminate_sets_the_option_bit_three_times_and_leaves_the_template_clean() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let dest = match receiver.local_addr().unwrap() {
            std::net::SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };

        let mut plans = vec![UniversePlan {
            universe: 7,
            packet: build_data_template(&CID, &pad_source_name("x"), 100, 0, 7, 6),
            src_offset: 0,
            src_len: 6,
            unicast: Some(dest),
            multicast: None,
            sequence: 41,
        }];
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert_eq!(terminate_plans(&socket, &mut plans), TERMINATE_REPEATS);

        let mut buf = [0u8; 1500];
        for expected_seq in 42..=44u8 {
            let n = receiver.recv(&mut buf).unwrap();
            assert_eq!(n, SLOTS_OFFSET + 6);
            assert_eq!(buf[OPTIONS_OFFSET], OPT_STREAM_TERMINATED);
            assert_eq!(buf[SEQ_OFFSET], expected_seq);
            assert_eq!(be16(&buf[113..115]), 7, "universe");
        }
        // A universe that comes back must resume as an ordinary stream.
        assert_eq!(plans[0].packet[OPTIONS_OFFSET], 0);
    }

    #[test]
    fn source_name_truncates_on_a_char_boundary_and_stays_terminated() {
        let long = "é".repeat(40); // 80 bytes: must cut at 62, not mid-code-point
        let out = pad_source_name(&long);
        assert_eq!(out[62], 0, "null terminator inside the 64-byte field");
        assert_eq!(out[63], 0);
        let text = std::str::from_utf8(&out[..62]).expect("never cut mid-code-point");
        assert_eq!(text.chars().count(), 31);
    }

    /// Offline sender: no destinations, so `send_frame`/`configure` touch no network
    /// but still run the full planning, sequencing and termination logic.
    fn offline(spokes: u32) -> (GeometryConfig, OutputConfig) {
        let geo = GeometryConfig {
            spokes,
            pixels_per_spoke: 4,
            ..Default::default()
        };
        let out = OutputConfig {
            cid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            pixels_per_universe: 4,
            multicast: false,  // no controllers configured either → no destinations
            discovery: false,  // keep the test off the wire entirely
            ..Default::default()
        };
        (geo, out)
    }

    #[test]
    fn reconfigure_carries_sequence_numbers_and_drops_dead_universes() {
        let mut s = SacnSender::new().unwrap();
        let (geo, out) = offline(2);
        s.configure(&geo, &out);
        assert_eq!(s.plan.len(), 2, "one universe per spoke");

        let rgb = vec![0u8; 2 * 4 * 3];
        for _ in 0..5 {
            s.send_frame(&rgb);
        }
        assert_eq!(s.plan[0].sequence, 5);
        assert!(s.streaming);

        // Shrink to one spoke: universe 1 survives, universe 2 is retired.
        let (geo, out) = offline(1);
        s.configure(&geo, &out);
        assert_eq!(s.plan.len(), 1);
        assert_eq!(s.plan[0].universe, 1);
        assert_eq!(
            s.plan[0].sequence, 5,
            "a surviving universe must not restart its sequence — a receiver would \
             discard the next packets as out-of-order"
        );
    }

    #[test]
    fn a_changed_cid_retires_every_universe() {
        let mut s = SacnSender::new().unwrap();
        let (geo, out) = offline(2);
        s.configure(&geo, &out);
        s.send_frame(&[0u8; 2 * 4 * 3]);

        let (geo, mut out) = offline(2);
        out.cid = "ffffffff-89ab-cdef-0123-456789abcdef".into();
        s.configure(&geo, &out);
        // Same universes, but a new identity: sequences start over with the new CID.
        assert_eq!(s.plan.len(), 2);
        assert_eq!(s.plan[0].sequence, 0);
    }

    #[test]
    fn cid_parses_to_network_byte_order() {
        let bytes = cid_bytes("01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(bytes, CID);
        // Garbage is survivable, not fatal — but it must not silently be zeros.
        assert_ne!(cid_bytes("not-a-uuid"), [0u8; 16]);
    }

    #[test]
    fn destination_mode_never_duplicates_multicast_and_unicast() {
        let mut sender = SacnSender::new().unwrap();
        let (geo, mut out) = offline(1);
        out.controllers = vec!["127.0.0.1".into()];

        out.multicast = true;
        sender.configure(&geo, &out);
        assert!(sender.plan[0].multicast.is_some());
        assert!(sender.plan[0].unicast.is_none());

        out.multicast = false;
        sender.configure(&geo, &out);
        assert!(sender.plan[0].multicast.is_none());
        assert!(sender.plan[0].unicast.is_some());
    }
}
