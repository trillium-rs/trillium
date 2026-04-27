use clap::Parser;
use std::path::PathBuf;
use std::sync::OnceLock;
use trillium::{Conn, HttpConfig, Method};
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Parser, Debug)]
#[command(about = "trillium h1/h2/h3 bench server")]
struct Args {
    #[arg(long, default_value_t = 8443)]
    port: u16,

    #[arg(
        long,
        default_value = "/home/ubuntu/trillium-h2-bench.tanuki-sunfish.ts.net.crt"
    )]
    cert: PathBuf,

    #[arg(
        long,
        default_value = "/home/ubuntu/trillium-h2-bench.tanuki-sunfish.ts.net.key"
    )]
    key: PathBuf,

    /// Disable HTTP/3 / QUIC. Default: HTTP/3 enabled on the same port (UDP).
    #[arg(long)]
    no_quic: bool,

    // ── HttpConfig knobs ──────────────────────────────────────────────────────
    #[arg(long)]
    head_max_len: Option<usize>,
    #[arg(long)]
    received_body_max_len: Option<u64>,
    #[arg(long)]
    response_buffer_len: Option<usize>,
    #[arg(long)]
    response_buffer_max_len: Option<usize>,
    #[arg(long)]
    request_buffer_initial_len: Option<usize>,
    #[arg(long)]
    response_header_initial_capacity: Option<usize>,
    #[arg(long)]
    copy_loops_per_yield: Option<usize>,
    #[arg(long)]
    received_body_initial_len: Option<usize>,
    #[arg(long)]
    received_body_max_preallocate: Option<usize>,

    #[arg(long)]
    h3_max_field_section_size: Option<u64>,
    #[arg(long)]
    h3_max_table_capacity: Option<usize>,
    #[arg(long)]
    h3_blocked_streams: Option<usize>,
    #[arg(long)]
    h3_qpack_header_observer_half_life_sections: Option<u32>,
    #[arg(long)]
    h3_qpack_header_observer_max_entries: Option<u32>,
    #[arg(long)]
    h3_datagrams_enabled: Option<bool>,

    #[arg(long)]
    h2_initial_stream_window_size: Option<u32>,
    #[arg(long)]
    h2_max_stream_recv_window_size: Option<u32>,
    #[arg(long)]
    h2_initial_connection_window_size: Option<u32>,
    #[arg(long)]
    h2_max_concurrent_streams: Option<u32>,
    #[arg(long)]
    h2_max_frame_size: Option<u32>,
    #[arg(long)]
    h2_max_header_list_size: Option<u32>,

    #[arg(long)]
    webtransport_enabled: Option<bool>,
    #[arg(long)]
    extended_connect_enabled: Option<bool>,
    #[arg(long)]
    panic_on_invalid_response_headers: Option<bool>,
}

fn http_config_from(args: &Args) -> HttpConfig {
    let mut c = HttpConfig::default();
    if let Some(v) = args.head_max_len { c.set_head_max_len(v); }
    if let Some(v) = args.received_body_max_len { c.set_received_body_max_len(v); }
    if let Some(v) = args.response_buffer_len { c.set_response_buffer_len(v); }
    if let Some(v) = args.response_buffer_max_len { c.set_response_buffer_max_len(v); }
    if let Some(v) = args.request_buffer_initial_len { c.set_request_buffer_initial_len(v); }
    if let Some(v) = args.response_header_initial_capacity { c.set_response_header_initial_capacity(v); }
    if let Some(v) = args.copy_loops_per_yield { c.set_copy_loops_per_yield(v); }
    if let Some(v) = args.received_body_initial_len { c.set_received_body_initial_len(v); }
    if let Some(v) = args.received_body_max_preallocate { c.set_received_body_max_preallocate(v); }

    if let Some(v) = args.h3_max_field_section_size { c.set_h3_max_field_section_size(v); }
    if let Some(v) = args.h3_max_table_capacity { c.set_h3_max_table_capacity(v); }
    if let Some(v) = args.h3_blocked_streams { c.set_h3_blocked_streams(v); }
    if let Some(v) = args.h3_qpack_header_observer_half_life_sections {
        c.set_h3_qpack_header_observer_half_life_sections(v);
    }
    if let Some(v) = args.h3_qpack_header_observer_max_entries {
        c.set_h3_qpack_header_observer_max_entries(v);
    }
    if let Some(v) = args.h3_datagrams_enabled { c.set_h3_datagrams_enabled(v); }

    if let Some(v) = args.h2_initial_stream_window_size { c.set_h2_initial_stream_window_size(v); }
    if let Some(v) = args.h2_max_stream_recv_window_size { c.set_h2_max_stream_recv_window_size(v); }
    if let Some(v) = args.h2_initial_connection_window_size { c.set_h2_initial_connection_window_size(v); }
    if let Some(v) = args.h2_max_concurrent_streams { c.set_h2_max_concurrent_streams(v); }
    if let Some(v) = args.h2_max_frame_size { c.set_h2_max_frame_size(v); }
    if let Some(v) = args.h2_max_header_list_size { c.set_h2_max_header_list_size(v); }

    if let Some(v) = args.webtransport_enabled { c.set_webtransport_enabled(v); }
    if let Some(v) = args.extended_connect_enabled { c.set_extended_connect_enabled(v); }
    if let Some(v) = args.panic_on_invalid_response_headers {
        c.set_panic_on_invalid_response_headers(v);
    }
    c
}

struct Bodies {
    one_k: &'static [u8],
    sixteen_k: &'static [u8],
    sixty_four_k: &'static [u8],
    one_m: &'static [u8],
    ten_m: &'static [u8],
}

fn bodies() -> &'static Bodies {
    static B: OnceLock<Bodies> = OnceLock::new();
    B.get_or_init(|| {
        let make = |n: usize| -> &'static [u8] { Box::leak(vec![b'x'; n].into_boxed_slice()) };
        Bodies {
            one_k: make(1024),
            sixteen_k: make(16 * 1024),
            sixty_four_k: make(64 * 1024),
            one_m: make(1024 * 1024),
            ten_m: make(10 * 1024 * 1024),
        }
    })
}

fn body_for(size: &str) -> Option<&'static [u8]> {
    let b = bodies();
    Some(match size {
        "1k" => b.one_k,
        "16k" => b.sixteen_k,
        "64k" => b.sixty_four_k,
        "1m" => b.one_m,
        "10m" => b.ten_m,
        _ => return None,
    })
}

async fn dispatch(mut conn: Conn) -> Conn {
    let method = conn.method();
    let path = conn.path();
    match (method, path) {
        (Method::Get, "/tiny") => conn.ok("ok"),
        (Method::Get, "/small") => conn.ok(bodies().one_k),
        (Method::Get, p) if p.starts_with("/large/") => match body_for(&p[7..]) {
            Some(body) => conn.ok(body),
            None => conn.with_status(404),
        },
        (Method::Post, "/echo") => match conn.request_body().read_bytes().await {
            Ok(body) => conn.ok(body),
            Err(_) => conn.with_status(400),
        },
        (Method::Post, "/recv") => match conn.request_body().read_bytes().await {
            Ok(body) => conn.ok(body.len().to_string()),
            Err(_) => conn.with_status(400),
        },
        _ => conn.with_status(404),
    }
}

fn main() {
    env_logger::init();
    let args = Args::parse();
    let cert = std::fs::read(&args.cert).expect("read cert");
    let key = std::fs::read(&args.key).expect("read key");

    bodies();

    let config = trillium_tokio::config()
        .with_host("0.0.0.0")
        .with_port(args.port)
        .with_nodelay()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .with_http_config(http_config_from(&args));

    if args.no_quic {
        config.run(dispatch);
    } else {
        config
            .with_quic(QuicConfig::from_single_cert(&cert, &key))
            .run(dispatch);
    }
}
