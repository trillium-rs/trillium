#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
use colored::*;
use size::{Base, Size, Style};
use std::time::Instant;
use trillium::{async_trait, http_types::StatusCode, Conn, Handler};

#[derive(Debug)]
struct Start(Instant);

impl Start {
    pub fn now() -> Self {
        Self(Instant::now())
    }
}

pub struct DevLogger;

#[async_trait]
impl Handler for DevLogger {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(Start::now())
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(start) = conn.take_state::<Start>() {
            let method = conn.method();
            let status = conn.get_status().unwrap_or(&StatusCode::NotFound);

            let len = conn
                .response_len()
                .map(|l| {
                    Size::to_string(&Size::Bytes(l), Base::Base10, Style::Smart).replace(" ", "")
                })
                .unwrap_or_else(|| String::from("-"));

            log::info!(
                r#"{method} {url} {status} {response_time:?} {len}"#,
                response_time = std::time::Instant::now() - start.0,
                method = method,
                url = conn.path(),
                status = (*status as u16).to_string().color(match *status as u16 {
                    200..=299 => "green",
                    300..=399 => "cyan",
                    400..=499 => "yellow",
                    500..=599 => "red",
                    _ => "white",
                }),
                len = len,
            );
        }
        conn
    }
}
