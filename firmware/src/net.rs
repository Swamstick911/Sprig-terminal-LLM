//! Networking: CYW43 WiFi bring-up + streaming HTTPS to the LLM (OpenRouter).
//!
//! This module owns everything between the on-device keyboard and the LLM:
//!
//!   1. [`init`] powers up the CYW43439 over PIO-SPI, loads its firmware + CLM
//!      blobs, joins the configured WPA2 network, brings up the embassy-net
//!      stack via DHCP, and hands back a [`Net`] handle (the `&'static Stack`
//!      plus the WiFi `Control`). It spawns the two required background tasks
//!      (`cyw43_runner_task`, `net_task`).
//!
//!   2. [`Net::send_chat`] opens a TLS 1.3 connection to `openrouter.ai`, POSTs a
//!      streaming `/api/v1/chat/completions` request whose body is built by the
//!      core crate's `OpenRouter` provider, and pumps the `text/event-stream`
//!      response back one SSE line at a time. Each decoded text delta is handed
//!      to the caller's callback so the UI can append it live, and the final
//!      token-usage count is returned. The full response is NEVER buffered
//!      (264 KiB RAM budget).
//!
//! The core crate does all JSON building and SSE classification; this layer is
//! purely the transport + glue.
//!
//! ## TLS posture (SECURITY NOTE)
//! reqwless 0.12's [`TlsVerify`] only offers `None` and `Psk` — there is no
//! server-certificate / pinned-root-CA verification path in this version of the
//! embedded-tls integration. We therefore connect with [`TlsVerify::None`]:
//! the channel is encrypted (TLS 1.3) but the server identity is **not**
//! authenticated, so a man-in-the-middle could impersonate `openrouter.ai`. Full
//! verification needs a newer reqwless/embedded-tls (a stack-wide upgrade) — see
//! the README's Limitations.

use cyw43::{Control, NetDriver, PowerManagementMode, Runner as Cyw43Runner, State};
use cyw43_pio::PioSpi;
use embassy_executor::Spawner;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Config as NetConfig, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use embedded_io_async::Read;
use heapless::{String, Vec};
use rand_core::RngCore;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::headers::ContentType;
use reqwless::request::{Method, RequestBuilder};
use static_cell::StaticCell;

use sprig_llm_core::provider::{LlmProvider, OpenRouter, Role};
use sprig_llm_core::sse::{process_openai_line, usage_total, SseOut};

use crate::config;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

/// CYW43 firmware + country/regulatory blob, vendored from the embassy repo's
/// `cyw43-firmware/` dir and committed under `firmware/cyw43-firmware/`.
static FW: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
static CLM: &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

/// How many concurrent sockets the embassy-net stack pre-allocates. The HTTPS
/// client needs one TCP socket + DNS uses the stack's internal facility, so a
/// small count suffices.
const SOCKETS: usize = 4;

/// TLS record buffers. embedded-tls needs room for a full TLS 1.3 record
/// (handshake certs can be large). 8 KiB each is the smallest that reliably
/// completes the api.anthropic.com handshake; the streamed body is read in
/// small chunks on top of these.
const TLS_RX: usize = 8 * 1024;
const TLS_TX: usize = 8 * 1024;

/// Per-call HTTP buffer: holds the request line + headers + the request body
/// (the JSON from `Claude::build_body`) and is reused for incoming header
/// parsing. The streamed SSE body is read separately in small chunks.
const HTTP_BUF: usize = 4 * 1024;

/// TCP socket buffers (carry the raw TLS records). Sized to a TLS record so a
/// full handshake flight fits. Order in `TcpClientState<N, TX, RX>`.
const TCP_TX: usize = 4 * 1024;
const TCP_RX: usize = 8 * 1024;

/// Largest prompt body we will build (JSON-escaped). Comfortably covers a
/// full-screen draft plus the Expand instruction prefix.
const BODY_CAP: usize = 4096;

/// Errors surfaced to the UI. Kept coarse — the screen only needs a short label.
#[derive(Debug, Clone, Copy, defmt::Format)]
pub enum NetError {
    /// Building the request JSON overflowed the body buffer.
    BodyTooLarge,
    /// DNS / TCP / TLS / HTTP transport failure.
    Transport,
    /// Non-2xx HTTP status (e.g. 401 bad/expired key, 429 rate-limited).
    Http(u16),
    /// The SSE stream reported an `error` event.
    StreamError,
}

/// CYW43 driver runner — must run forever to service the WiFi chip.
#[embassy_executor::task]
async fn cyw43_runner_task(
    runner: Cyw43Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

/// embassy-net stack runner — must run forever to process network events.
#[embassy_executor::task]
async fn net_task(stack: &'static Stack<NetDriver<'static>>) -> ! {
    stack.run().await
}

/// The peripherals [`init`] needs. Passing them explicitly keeps `main.rs` in
/// charge of the (verified, non-conflicting) Pico W internal wiring.
pub struct NetPins {
    pub pwr: PIN_23,
    pub cs: PIN_25,
    pub dio: PIN_24,
    pub clk: PIN_29,
    pub pio: PIO0,
    pub dma: DMA_CH0,
}

/// A live network handle: the WiFi control channel plus the running stack.
pub struct Net {
    control: Control<'static>,
    stack: &'static Stack<NetDriver<'static>>,
}

/// Bring up WiFi + the network stack. Spawns the two background tasks and
/// returns once an IP address has been acquired via DHCP.
///
/// Returns `Err(())` only if joining the AP fails; DHCP is waited on with a
/// generous loop. Firmware-blob loading and stack creation are infallible here.
pub async fn init(spawner: Spawner, pins: NetPins) -> Result<Net, ()> {
    let mut rng = RoscRng;

    // --- PIO-SPI to the CYW43439 (Pico W internal wiring). ---
    let pwr = Output::new(pins.pwr, Level::Low);
    let cs = Output::new(pins.cs, Level::High);
    let mut pio = Pio::new(pins.pio, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        pins.dio,
        pins.clk,
        pins.dma,
    );

    // --- cyw43 driver: state lives forever in a StaticCell. ---
    static STATE: StaticCell<State> = StaticCell::new();
    let state = STATE.init(State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, FW).await;
    spawner.spawn(cyw43_runner_task(runner)).ok();

    control.init(CLM).await;
    control
        .set_power_management(PowerManagementMode::PowerSave)
        .await;

    // --- Join the configured WPA2 network. ---
    loop {
        match control
            .join_wpa2(config::WIFI_SSID, config::WIFI_PASSWORD)
            .await
        {
            Ok(()) => break,
            Err(e) => {
                defmt::warn!("WiFi join failed (status={}), retrying", e.status);
                Timer::after(Duration::from_secs(2)).await;
            }
        }
    }
    defmt::info!("WiFi joined");

    // --- embassy-net stack (DHCP). ---
    let net_config = NetConfig::dhcpv4(Default::default());
    let seed = rng.next_u64();

    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    static STACK: StaticCell<Stack<NetDriver<'static>>> = StaticCell::new();
    let stack = STACK.init(Stack::new(
        net_device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    ));
    spawner.spawn(net_task(stack)).ok();

    // Wait for DHCP to assign an address before any sockets are opened.
    loop {
        if stack.is_config_up() {
            break;
        }
        Timer::after(Duration::from_millis(200)).await;
    }
    if let Some(cfg) = stack.config_v4() {
        defmt::info!("DHCP up: {}", cfg.address);
    }

    Ok(Net { control, stack })
}

impl Net {
    /// Turn the on-board LED on/off (handy as a "sending" indicator). The CYW43
    /// drives the Pico W user LED, so this lives on the WiFi control channel.
    pub async fn set_led(&mut self, on: bool) {
        self.control.gpio_set(0, on).await;
    }

    /// Stream a single OpenRouter completion for `prompt` (single-turn).
    ///
    /// Builds the request body with the core [`OpenRouter`] provider (the default
    /// `config::MODEL` and 1024 max tokens), POSTs it over TLS 1.3, and feeds the
    /// streamed SSE body to the core classifier. `on_delta` is called with each
    /// new text fragment as it arrives; it must not block.
    ///
    /// Returns the total token count reported by the stream (0 if none) on a
    /// clean stop event, or a [`NetError`]. Implemented as a thin wrapper over
    /// [`send_chat`] so both share one request/stream path.
    #[allow(dead_code)] // kept as the single-turn public convenience API
    pub async fn send_prompt<F>(&mut self, prompt: &str, on_delta: F) -> Result<u32, NetError>
    where
        F: FnMut(&str),
    {
        self.send_chat(
            config::MODEL,
            1024,
            None,
            &[(Role::User, prompt)],
            on_delta,
        )
        .await
    }

    /// Stream a multi-turn chat completion.
    ///
    /// Builds the body from `model` + `max_tokens` + an optional `system` prompt
    /// + the ordered `turns` via [`OpenRouter::build_chat_body`], then POSTs and
    /// streams it exactly like [`send_prompt`] used to. `on_delta` receives each
    /// text fragment as it arrives; it must not block. Returns the total token
    /// count reported by the stream (0 if none) on a clean stop, or a
    /// [`NetError`].
    pub async fn send_chat<F>(
        &mut self,
        model: &str,
        max_tokens: u32,
        system: Option<&str>,
        turns: &[(Role, &str)],
        on_delta: F,
    ) -> Result<u32, NetError>
    where
        F: FnMut(&str),
    {
        // --- Build the JSON body via the core crate (escaping + "stream":true). ---
        let mut provider = OpenRouter::new(model);
        provider.max_tokens = max_tokens;
        let mut body: String<BODY_CAP> = String::new();
        provider
            .build_chat_body(system, turns, &mut body)
            .map_err(|_| NetError::BodyTooLarge)?;

        self.post_and_stream(provider.host(), provider.path(), &body, on_delta)
            .await
    }

    /// Shared transport: open TLS to `host`, POST `body` to `path`, and stream
    /// the SSE response, handing each text delta to `on_delta`. Both
    /// [`send_prompt`] and [`send_chat`] funnel through here so the three stream
    /// fixes (status check, UTF-8 line decode, over-long-line skip) live once.
    async fn post_and_stream<F>(
        &mut self,
        host: &str,
        path: &str,
        body: &str,
        mut on_delta: F,
    ) -> Result<u32, NetError>
    where
        F: FnMut(&str),
    {
        // --- TLS + TCP + DNS clients over the embassy-net stack. ---
        let mut tls_rx = [0u8; TLS_RX];
        let mut tls_tx = [0u8; TLS_TX];
        let seed = RoscRng.next_u64();
        // SECURITY TODO(M2): reqwless 0.12's TlsVerify has no server-cert /
        // pinned-CA option (only None | Psk). We use TlsVerify::None: the link
        // is encrypted but the server is NOT authenticated (MITM-able). Upgrade
        // to a reqwless/embedded-tls release with cert verification, or pin the
        // Anthropic root CA, before trusting this on an untrusted network.
        let tls = TlsConfig::new(seed, &mut tls_rx, &mut tls_tx, TlsVerify::None);

        let tcp_state: TcpClientState<1, TCP_TX, TCP_RX> = TcpClientState::new();
        let tcp = TcpClient::new(self.stack, &tcp_state);
        let dns = DnsSocket::new(self.stack);
        let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

        // Full URL = scheme://host/path. host()/path() come from the provider.
        let mut url: String<96> = String::new();
        url.push_str("https://").map_err(|_| NetError::Transport)?;
        url.push_str(host).map_err(|_| NetError::Transport)?;
        url.push_str(path).map_err(|_| NetError::Transport)?;

        let mut http_buf = [0u8; HTTP_BUF];

        // OpenRouter uses bearer auth (content-type set via .content_type()).
        let mut auth: String<128> = String::new();
        auth.push_str("Bearer ").map_err(|_| NetError::Transport)?;
        auth.push_str(config::API_KEY).map_err(|_| NetError::Transport)?;
        let headers = [("Authorization", auth.as_str())];

        let mut req = client
            .request(Method::POST, &url)
            .await
            .map_err(|_| NetError::Transport)?
            .headers(&headers)
            .content_type(ContentType::ApplicationJson)
            .body(body.as_bytes());

        let response = req.send(&mut http_buf).await.map_err(|_| {
            defmt::error!("HTTP send failed");
            NetError::Transport
        })?;

        // reqwless returns Ok for any well-formed response, including 401/429/4xx
        // (which arrive as a JSON error body, not an SSE stream). Surface them so
        // a bad key or rate-limit shows an error instead of a blank "Done".
        let code = response.status.0;
        if !(200..300).contains(&code) {
            defmt::error!("HTTP status {}", code);
            return Err(NetError::Http(code));
        }

        // --- Stream the SSE body. Accumulate raw bytes into a line buffer and
        // hand only COMPLETE '\n'-delimited lines (decoded as UTF-8) to the core
        // classifier, so multi-byte text and lines split across reads survive. ---
        let mut reader = response.body().reader();
        let mut chunk = [0u8; 256];
        let mut line: Vec<u8, 512> = Vec::new();
        let mut delta: String<256> = String::new();
        // True while discarding the rest of an over-long line up to the next '\n'.
        let mut skipping = false;
        // Latest `usage.total_tokens` seen (the final chunk reports it); 0 if the
        // stream never carried a usage object.
        let mut total: u32 = 0;

        loop {
            let n = reader.read(&mut chunk).await.map_err(|_| NetError::Transport)?;
            if n == 0 {
                break; // connection/body finished
            }
            for &b in &chunk[..n] {
                if b == b'\n' {
                    if !skipping {
                        if let Ok(text) = core::str::from_utf8(&line) {
                            match process_openai_line(text, &mut delta) {
                                SseOut::Delta => on_delta(&delta),
                                SseOut::Stop => return Ok(total),
                                SseOut::Error => return Err(NetError::StreamError),
                                SseOut::None => {}
                            }
                            // The usage chunk arrives alongside (or just before)
                            // the terminator; stash the latest count seen.
                            if let Some(t) = usage_total(text) {
                                total = t;
                            }
                        }
                    }
                    line.clear();
                    skipping = false;
                } else if !skipping && line.push(b).is_err() {
                    // Line longer than the buffer: drop the remainder up to the
                    // next newline rather than misframe a fragment.
                    line.clear();
                    skipping = true;
                }
            }
        }

        // Process any trailing partial line (no final newline).
        if !skipping && !line.is_empty() {
            if let Ok(text) = core::str::from_utf8(&line) {
                match process_openai_line(text, &mut delta) {
                    SseOut::Delta => on_delta(&delta),
                    SseOut::Error => return Err(NetError::StreamError),
                    _ => {}
                }
                if let Some(t) = usage_total(text) {
                    total = t;
                }
            }
        }
        Ok(total)
    }
}
