//! SO_REUSEPORT thread-per-core server.
//!
//! Run with `cargo run --example reuseport --features reuseport`, then `curl localhost:8080`
//! repeatedly — the response names the per-core worker that served it, so you can watch the
//! kernel fan connections out across the listener group.
//!
//! Fan-out is a Linux feature. Other Unixes (notably macOS/Darwin) accept the socket options but
//! deliver every connection to a single listener, so all requests land on one worker — the feature
//! is gated off there and this example prints a notice instead.
//!
//! `WORKERS` sets the number of per-core TCP listeners (default: available parallelism).

#[cfg(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
fn main() {
    use trillium::Conn;

    env_logger::init();
    trillium_tokio::server()
        .bind_reuseport_tcp(8080)
        .unwrap()
        .run(|conn: Conn| async move {
            let worker = std::thread::current().name().unwrap_or("?").to_owned();
            log::info!("{worker}");
            conn.ok(format!("hello from {worker}\n"))
        });
}

#[cfg(not(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
)))]
fn main() {
    eprintln!(
        "the reuseport entrypoint is unavailable on this platform: SO_REUSEPORT does not fan \
         connections out across the listener group here, so a thread-per-core listener group \
         offers no benefit over a single work-stealing runtime."
    );
}
