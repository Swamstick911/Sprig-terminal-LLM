//! USB HID keyboard: "type" text into a connected PC.
//!
//! The RP2040's native USB peripheral (`p.USB`) is otherwise unused here — WiFi
//! lives on the CYW43 radio over PIO0 and logging goes out RTT (defmt-rtt), not
//! USB — so this module can own `p.USB` exclusively and present the device to a
//! host computer as a standard boot-protocol HID keyboard.
//!
//! Flow:
//!   * [`init`] builds an [`embassy_usb`] device with a single HID keyboard
//!     interface (boot keyboard report descriptor from [`usbd_hid`]), spawns the
//!     [`usb_device_task`] that drives enumeration + transfers forever, and hands
//!     back a [`UsbKeyboard`] owning the HID writer.
//!   * [`UsbKeyboard::type_text`] walks a `&str`, maps each char to a US-layout
//!     `(keycode, shift?)` via [`char_to_key`], and emits a key-down then key-up
//!     report for each so repeats register on the host. Reports are written with
//!     a short timeout, so a host that hasn't enumerated us (or stops polling)
//!     makes `type_text` bail out instead of hanging.
//!
//! This is allocation-free: the USB buffers/state live in `&'static` [`StaticCell`]
//! storage (mirroring how `net.rs` parks the cyw43 `State`), and `type_text`
//! builds each 8-byte report on the stack.

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{with_timeout, Duration, Timer};
use embassy_usb::class::hid::{self, HidWriter, State};
use embassy_usb::{Builder, Config as UsbConfig, UsbDevice};
use static_cell::StaticCell;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

// The RP2040 USB controller IRQ. This is a distinct interrupt from the PIO0 one
// net.rs binds and the PIO1 one main.rs binds, so a separate `bind_interrupts!`
// here does not collide with theirs.
bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

/// USB Vendor/Product IDs.
///
/// `0x1209` is the [pid.codes](https://pid.codes) community VID; `0x0001` under
/// it is the block explicitly reserved for development / testing / hobby use, so
/// it is safe to ship on a one-off device without registering a product. Swap in
/// an allocated PID before distributing hardware.
const VID: u16 = 0x1209;
const PID: u16 = 0x0001;

/// HID interrupt-IN polling interval (ms). 8 ms (125 Hz) is a typical keyboard
/// rate and is well within full-speed limits.
const POLL_MS: u8 = 8;

/// Boot keyboard report size, in bytes: modifier + reserved + 6 keycodes.
const REPORT_LEN: usize = 8;

/// Per-report write timeout. If the host is not enumerated / not polling the IN
/// endpoint, `write_serialize` would otherwise await forever; bailing out after
/// this keeps [`UsbKeyboard::type_text`] from hanging the caller.
const WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// Inter-report delay so the host registers each press/release as a distinct
/// event (and consecutive identical chars repeat). ~12 ms ensures at least
/// one host poll (typically every 8 ms) sees each state.
const KEY_DELAY: Duration = Duration::from_millis(12);

/// Left-Shift modifier bit in a HID keyboard report's `modifier` byte.
const MOD_LSHIFT: u8 = 0x02;

/// Owns the HID writer half of the keyboard. The matching device task
/// ([`usb_device_task`]) was spawned by [`init`] and runs independently.
pub struct UsbKeyboard {
    writer: HidWriter<'static, Driver<'static, USB>, REPORT_LEN>,
}

/// Drives the USB device: enumeration, control transfers, and endpoint I/O. Must
/// run forever; `UsbDevice::run` never returns.
#[embassy_executor::task]
async fn usb_device_task(mut device: UsbDevice<'static, Driver<'static, USB>>) -> ! {
    device.run().await
}

/// Bring up the USB HID keyboard on `usb` (the RP2040 USB peripheral), spawn the
/// device task, and return the writer handle.
///
/// All descriptor/control buffers and the HID `State` are parked in `&'static`
/// `StaticCell`s so the spawned [`usb_device_task`] (which requires `'static`)
/// can own the resulting [`UsbDevice`]. Call this exactly once.
pub fn init(spawner: Spawner, usb: USB) -> UsbKeyboard {
    let driver = Driver::new(usb, Irqs);

    // --- Device descriptor / identification. ---
    let mut config = UsbConfig::new(VID, PID);
    config.manufacturer = Some("Sprig");
    config.product = Some("Sprig LLM Terminal");
    config.serial_number = Some("0001");
    config.max_power = 100; // mA, bus-powered
    config.max_packet_size_0 = 64;

    // --- &'static buffers the Builder needs for the descriptors + EP0. ---
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static HID_STATE: StaticCell<State> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );

    // --- HID keyboard class (boot keyboard report descriptor, write-only). ---
    let hid_config = hid::Config {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: POLL_MS,
        max_packet_size: REPORT_LEN as u16,
    };
    let writer =
        HidWriter::<_, REPORT_LEN>::new(&mut builder, HID_STATE.init(State::new()), hid_config);

    // Spawn the device task to drive enumeration/transfers forever. `.ok()`:
    // spawning only fails if the task is already running, which can't happen
    // since `init` is called once.
    let device = builder.build();
    spawner.spawn(usb_device_task(device)).ok();

    UsbKeyboard { writer }
}

impl UsbKeyboard {
    /// Send a single 8-byte boot-keyboard report, with a timeout. Returns `false`
    /// if the host is not ready (timed out) or the endpoint errored, so callers
    /// can stop early instead of blocking.
    async fn send(&mut self, report: &KeyboardReport) -> bool {
        matches!(
            with_timeout(WRITE_TIMEOUT, self.writer.write_serialize(report)).await,
            Ok(Ok(()))
        )
    }

    /// Type `text` into the focused application on the connected host.
    ///
    /// Each mappable char becomes a key-down report (with Left-Shift when needed)
    /// followed by an empty key-up report, with a short delay between reports so
    /// the host registers distinct presses and repeats. Unmappable chars are
    /// skipped. If a write fails (host not enumerated / not polling), this returns
    /// early rather than hanging. Allocation-free.
    pub async fn type_text(&mut self, text: &str) {
        const RELEASE: KeyboardReport = KeyboardReport {
            modifier: 0,
            reserved: 0,
            leds: 0,
            keycodes: [0; 6],
        };

        for c in text.chars() {
            let Some((keycode, shift)) = char_to_key(c) else {
                continue; // unmappable on a US layout — skip it
            };
            let press = KeyboardReport {
                modifier: if shift { MOD_LSHIFT } else { 0 },
                reserved: 0,
                leds: 0,
                keycodes: [keycode, 0, 0, 0, 0, 0],
            };

            if !self.send(&press).await {
                return;
            }
            Timer::after(KEY_DELAY).await;
            if !self.send(&RELEASE).await {
                return;
            }
            Timer::after(KEY_DELAY).await;
        }
    }
}

/// Map a char to a `(HID usage keycode, needs-shift)` pair for the US keyboard
/// layout. Covers a-z, A-Z, 0-9, space, Enter (newline), Tab, and the common
/// ASCII punctuation reachable on a US layout. Returns `None` for anything else
/// (e.g. non-ASCII), so the caller skips it.
///
/// Keycodes are USB HID Usage IDs (HID Usage Tables, "Keyboard/Keypad" page).
fn char_to_key(c: char) -> Option<(u8, bool)> {
    let pair = match c {
        // Letters: 'a' = 0x04 .. 'z' = 0x1D. Uppercase = same code + shift.
        'a'..='z' => (0x04 + (c as u8 - b'a'), false),
        'A'..='Z' => (0x04 + (c as u8 - b'A'), true),

        // Top-row digits: '1'..'9' = 0x1E..0x26, '0' = 0x27.
        '1'..='9' => (0x1E + (c as u8 - b'1'), false),
        '0' => (0x27, false),

        // Shifted top-row symbols (same keys as the digits above).
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),

        // Whitespace / control.
        ' ' => (0x2C, false),  // Space
        '\n' => (0x28, false), // Enter / Return
        '\t' => (0x2B, false), // Tab

        // Punctuation keys (unshifted, then their shifted partner).
        '-' => (0x2D, false),
        '_' => (0x2D, true),
        '=' => (0x2E, false),
        '+' => (0x2E, true),
        '[' => (0x2F, false),
        '{' => (0x2F, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),

        _ => return None,
    };
    Some(pair)
}
