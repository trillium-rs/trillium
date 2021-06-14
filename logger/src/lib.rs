#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
Development-mode logger for trillium

stability note: this is basically just a proof of concept, and the
interface will likely change quite a bit before stabilizing
*/

use colored::*;
use size::{Base, Size, Style};
use std::time::Instant;
use trillium::{async_trait, http_types::StatusCode, Conn, Handler, Info};

/**
Development-mode logger for trillium

stability note: this is basically just a proof of concept, and the
interface will likely change quite a bit before stabilizing
*/
#[derive(Clone, Copy, Debug, Default)]
pub struct Logger(());
impl Logger {
    /// construct a new logger
    pub fn new() -> Self {
        Self(())
    }
}

struct LoggerRan;

#[async_trait]
impl Handler for Logger {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(LoggerRan)
    }

    async fn init(&mut self, info: &mut Info) {
        log::info!(
            "
🌱🦀🌱 {} started
Listening at {}{}

Control-C to quit",
            info.server_description(),
            info.listener_description(),
            info.tcp_socket_addr()
                .map(|s| format!(" (bound as tcp://{})", s))
                .unwrap_or_default()
        );
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if conn.take_state::<LoggerRan>().is_some() {
            let start_time = conn.inner().start_time();
            let method = conn.method();
            let status = conn.status().unwrap_or(StatusCode::NotFound);
            let len = conn
                .response_len()
                .map(|l| {
                    Size::to_string(&Size::Bytes(l), Base::Base10, Style::Smart).replace(" ", "")
                })
                .unwrap_or_else(|| String::from("-"));

            let url = String::from(conn.path());

            let status_string = (status as u16).to_string().color(match status as u16 {
                200..=299 => "green",
                300..=399 => "cyan",
                400..=499 => "yellow",
                500..=599 => "red",
                _ => "white",
            });

            conn.inner_mut().after_send(move |_| {
                log::info!(
                    r#"{method} {url} {status} {response_time:?} {len}"#,
                    response_time = Instant::now() - start_time,
                    method = method,
                    url = url,
                    status = status_string,
                    len = len,
                );
            });
        }
        conn
    }
}
