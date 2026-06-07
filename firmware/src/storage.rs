//! Persistent storage for settings + conversation history (last-sector flash).
//!
//! The user's three setting indices (model / persona / max_tokens) and the
//! bounded chat history survive power-off by living in the **top 4 KiB sector**
//! of the 2 MiB QSPI flash, a region the linker has been told NOT to use for
//! program code (see `memory.x`: `FLASH` length is shortened by one sector).
//!
//! ## Why this is safe on the RP2040
//!
//! Erasing or programming the external QSPI flash forces the chip out of XIP
//! (execute-in-place) mode for the duration of the bootrom routine, so **any**
//! code or interrupt that fetches from flash during that window would fault.
//! `embassy_rp::flash`'s blocking ops handle this for us: `in_ram()` copies the
//! erase/program routine into SRAM, pauses core1, enters a
//! `critical_section` (interrupts OFF), drains in-flight DMA that targets flash,
//! and only then runs the routine — all from RAM. Nothing fetches from flash
//! while XIP is down. (Source: embassy-rp 0.2.0 `src/flash.rs`, `in_ram` +
//! `blocking_erase`/`blocking_write`.)
//!
//! Two further rules the *caller* must keep (enforced at the call sites in
//! `main.rs`, not here):
//!   * NEVER call [`save`] while a network request is in flight — only at
//!     quiescent points (after a turn fully completes, after a settings change).
//!     A flash write parks the cyw43 async task for ~ms; doing it mid-stream
//!     would stall TCP and drop the SSE body.
//!   * This is a single-core app (core1 idle), so the pause-core1 step is a
//!     no-op and the only cost is the brief interrupts-off critical section.
//!
//! ## On-sector format (versioned, CRC-checked, bounded)
//!
//! ```text
//!   offset  size  field
//!   0       4     magic   = b"SPRG"
//!   4       1     version = STORAGE_VERSION
//!   5       2     payload_len (u16 LE)            — bytes of payload that follow
//!   7       4     crc32 (u32 LE) of payload[..payload_len]  (IEEE, reflected)
//!   11      ..    payload:
//!                   1  model index (u8)
//!                   1  persona index (u8)
//!                   1  max_tokens index (u8)
//!                   1  turn count (u8)
//!                   then per turn:
//!                     1  role (u8, see role_to_u8)
//!                     2  text byte length (u16 LE)
//!                     N  UTF-8 text bytes
//! ```
//!
//! An unwritten (all-`0xFF`) or corrupt sector fails the magic / version / CRC
//! checks and [`load`] returns `None` → the app starts with defaults.

use embassy_rp::flash::Blocking;
use embassy_rp::peripherals::FLASH;
use heapless::{String as HString, Vec as HVec};
use sprig_llm_core::provider::Role;

// ---------------------------------------------------------------------------
// Geometry — keep these in sync with `memory.x` and `main.rs`.
// ---------------------------------------------------------------------------

/// Physical QSPI flash size on the Pico WH (2 MiB). This is the `FLASH_SIZE`
/// const-generic the embassy `Flash` driver uses for its bounds checks, so it
/// must be the *whole chip* size, not the shortened linker `FLASH` region.
pub const FLASH_CHIP_SIZE: usize = 2 * 1024 * 1024; // 0x0020_0000

/// One flash sector = the erase granularity (`embassy_rp::flash::ERASE_SIZE`).
pub const SECTOR_SIZE: usize = 4096; // 0x1000

/// Byte offset of our storage sector **from the start of flash** (i.e. the
/// argument to `blocking_read/erase/write`, NOT an absolute address). It is the
/// last sector of the chip: `0x0020_0000 - 0x1000 = 0x001F_F000`. The matching
/// absolute XIP address is `0x1000_0000 + 0x001F_F000 = 0x101F_F000`.
///
/// `memory.x` shortens `FLASH` to `2048K - 0x100 - 0x1000`, so the program can
/// never link into this sector. (The `-0x100` is the pre-existing boot2 offset.)
pub const STORAGE_OFFSET: u32 = (FLASH_CHIP_SIZE - SECTOR_SIZE) as u32; // 0x001F_F000

/// Concrete blocking-mode flash driver type the app constructs once and hands
/// to [`load`] / [`save`]. Construct with `Flash::new_blocking(p.FLASH)`.
pub type Flash<'d> = embassy_rp::flash::Flash<'d, FLASH, Blocking, FLASH_CHIP_SIZE>;

// ---------------------------------------------------------------------------
// Format constants.
// ---------------------------------------------------------------------------

const MAGIC: [u8; 4] = *b"SPRG";
const STORAGE_VERSION: u8 = 1;

/// Fixed header bytes preceding the payload: magic(4) + version(1) +
/// payload_len(2) + crc32(4).
const HEADER_LEN: usize = 4 + 1 + 2 + 4; // = 11

/// Per-turn text cap — must match `main.rs` `TURN_CAP`.
const TURN_CAP: usize = 256;
/// Max retained turns — must match `main.rs` `MAX_TURNS`.
const MAX_TURNS: usize = 6;

// ---------------------------------------------------------------------------
// Role <-> u8 mapping. Defined ONCE here so encode/decode can never disagree.
// `provider::Role` is `System, User, Assistant` (default discriminants 0/1/2);
// we pin an explicit, stable wire mapping rather than relying on `as u8`.
// ---------------------------------------------------------------------------

const ROLE_SYSTEM: u8 = 0;
const ROLE_USER: u8 = 1;
const ROLE_ASSISTANT: u8 = 2;

/// Stable wire encoding of a chat role.
pub fn role_to_u8(role: Role) -> u8 {
    match role {
        Role::System => ROLE_SYSTEM,
        Role::User => ROLE_USER,
        Role::Assistant => ROLE_ASSISTANT,
    }
}

/// Inverse of [`role_to_u8`]; unknown bytes decode to `User` (a harmless,
/// always-valid default) so a slightly-off byte never wedges decoding.
pub fn role_from_u8(b: u8) -> Role {
    match b {
        ROLE_SYSTEM => Role::System,
        ROLE_ASSISTANT => Role::Assistant,
        _ => Role::User,
    }
}

// ---------------------------------------------------------------------------
// The decoded record.
// ---------------------------------------------------------------------------

/// A snapshot of what we persist: the three setting indices plus the bounded
/// conversation. `turns` is `(role_u8, text)` so the caller maps roles via
/// [`role_from_u8`] / [`role_to_u8`].
pub struct Persisted {
    pub model: u8,
    pub persona: u8,
    pub max_tokens: u8,
    pub turns: HVec<(u8, HString<TURN_CAP>), MAX_TURNS>,
}

// ---------------------------------------------------------------------------
// CRC-32 (IEEE 802.3, reflected — same polynomial as zlib/PNG). Tableless to
// keep the image small; it runs at most over ~1.5 KiB a handful of times.
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            // Branchless: subtract turns 1 -> 0xFFFF_FFFF, 0 -> 0.
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// Load.
// ---------------------------------------------------------------------------

/// Read the storage sector and return the decoded record, or `None` if the
/// sector is blank / wrong-version / fails the CRC (i.e. "no saved state").
///
/// Reads are plain XIP loads (they do NOT disable XIP), so this is cheap and
/// safe to call at boot. Never panics.
pub fn load(flash: &mut Flash<'_>) -> Option<Persisted> {
    let mut buf = [0u8; SECTOR_SIZE];
    if let Err(e) = flash.blocking_read(STORAGE_OFFSET, &mut buf) {
        defmt::warn!("storage: blocking_read failed: {:?}", e);
        return None;
    }

    // --- Header checks. ---
    if buf[0..4] != MAGIC {
        // Blank/garbage sector — expected on first boot. Not an error.
        return None;
    }
    if buf[4] != STORAGE_VERSION {
        defmt::info!("storage: version {} != {}, ignoring", buf[4], STORAGE_VERSION);
        return None;
    }
    let payload_len = u16::from_le_bytes([buf[5], buf[6]]) as usize;
    let crc_stored = u32::from_le_bytes([buf[7], buf[8], buf[9], buf[10]]);

    // Payload must fit the sector and hold at least the 4 fixed fields.
    if payload_len < 4 || HEADER_LEN + payload_len > SECTOR_SIZE {
        defmt::warn!("storage: bad payload_len {}", payload_len);
        return None;
    }
    let payload = &buf[HEADER_LEN..HEADER_LEN + payload_len];
    if crc32(payload) != crc_stored {
        defmt::warn!("storage: CRC mismatch, treating as empty");
        return None;
    }

    // --- Decode payload. ---
    let model = payload[0];
    let persona = payload[1];
    let max_tokens = payload[2];
    let turn_count = payload[3] as usize;

    let mut turns: HVec<(u8, HString<TURN_CAP>), MAX_TURNS> = HVec::new();
    let mut cur = 4usize;
    for _ in 0..turn_count {
        if turns.is_full() {
            break; // never trust a count larger than capacity
        }
        // Each turn header is role(1) + len(2).
        if cur + 3 > payload.len() {
            defmt::warn!("storage: truncated turn header");
            return None;
        }
        let role = payload[cur];
        let len = u16::from_le_bytes([payload[cur + 1], payload[cur + 2]]) as usize;
        cur += 3;
        if len > TURN_CAP || cur + len > payload.len() {
            defmt::warn!("storage: turn length {} out of range", len);
            return None;
        }
        let text_bytes = &payload[cur..cur + len];
        cur += len;
        let text = match core::str::from_utf8(text_bytes) {
            Ok(s) => s,
            Err(_) => {
                defmt::warn!("storage: invalid UTF-8 in turn");
                return None;
            }
        };
        let mut s: HString<TURN_CAP> = HString::new();
        // `len <= TURN_CAP` so this always fits.
        let _ = s.push_str(text);
        let _ = turns.push((role, s));
    }

    Some(Persisted {
        model,
        persona,
        max_tokens,
        turns,
    })
}

// ---------------------------------------------------------------------------
// Save.
// ---------------------------------------------------------------------------

/// Serialise `data`, erase the storage sector, and write it back. Errors are
/// logged and swallowed — a failed persist must never crash the device. Call
/// ONLY at quiescent points (no network request in flight).
pub fn save(flash: &mut Flash<'_>, data: &Persisted) {
    // Build the full sector image in RAM, pre-filled with the erased value so
    // the unused tail matches a freshly-erased sector.
    let mut buf = [0xFFu8; SECTOR_SIZE];

    // Payload starts right after the header.
    let mut cur = HEADER_LEN;
    buf[cur] = data.model;
    buf[cur + 1] = data.persona;
    buf[cur + 2] = data.max_tokens;
    let count_idx = cur + 3; // filled in after we know how many turns fit
    cur += 4;

    let mut written_turns: u8 = 0;
    for (role, text) in &data.turns {
        if written_turns as usize >= MAX_TURNS {
            break;
        }
        let bytes = text.as_bytes();
        let len = core::cmp::min(bytes.len(), TURN_CAP);
        // Stop if this turn would not fit the sector (keeps the prefix that did).
        if cur + 3 + len > SECTOR_SIZE {
            break;
        }
        buf[cur] = *role;
        let le = (len as u16).to_le_bytes();
        buf[cur + 1] = le[0];
        buf[cur + 2] = le[1];
        cur += 3;
        buf[cur..cur + len].copy_from_slice(&bytes[..len]);
        cur += len;
        written_turns += 1;
    }
    buf[count_idx] = written_turns;

    // --- Header (magic, version, payload_len, crc over the payload). ---
    let payload_len = cur - HEADER_LEN;
    let crc = crc32(&buf[HEADER_LEN..cur]);
    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = STORAGE_VERSION;
    let pl = (payload_len as u16).to_le_bytes();
    buf[5] = pl[0];
    buf[6] = pl[1];
    buf[7..11].copy_from_slice(&crc.to_le_bytes());

    // --- Commit: erase the one sector, then program it. Both run from RAM in a
    // critical section inside embassy-rp; see the module-level safety note. ---
    let from = STORAGE_OFFSET;
    let to = STORAGE_OFFSET + SECTOR_SIZE as u32;
    if let Err(e) = flash.blocking_erase(from, to) {
        defmt::error!("storage: erase failed: {:?}", e);
        return;
    }
    if let Err(e) = flash.blocking_write(STORAGE_OFFSET, &buf) {
        defmt::error!("storage: write failed: {:?}", e);
        return;
    }
    defmt::info!(
        "storage: saved ({} turns, {} payload bytes)",
        written_turns,
        payload_len
    );
}
