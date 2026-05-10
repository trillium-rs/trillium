//! Request/response logging for [`trillium-client`][trillium_client].
//!
//! This module is gated behind the `client` cargo feature. It provides [`ClientLogger`], a
//! [`ClientHandler`] that emits a log line per request, with a composable formatter system that
//! mirrors the server-side [`Logger`][crate::Logger] in spirit.
//!
//! # Lifecycle
//!
//! `ClientLogger` records a request-start instant the first time it runs and emits the
//! formatted line after the response is available. Its position in the handler chain
//! determines the scope of [`response_time`][formatters::response_time] — only handlers running
//! after `ClientLogger` are timed.
//!
//! A log line is emitted for every request, regardless of outcome: successful responses,
//! responses synthesized by an upstream handler (cache hit, mock), and transport-layer
//! failures (connection refused, TLS error, timeout) all land in the log. The [`dev_formatter`]
//! renders the transport error inline when one is stashed on the conn; custom formatters can
//! pick it up via the [`error`][formatters::error] component.
//!
//! # Formatters
//!
//! See [`ClientLogFormatter`] for the trait, and the [`formatters`] submodule for the building
//! blocks. The default is [`dev_formatter`].
//!
//! # Example
//!
//! ```no_run
//! use trillium_client::Client;
//! use trillium_logger::client::{ClientLogger, formatters};
//! # use trillium_testing::client_config;
//!
//! let client = Client::new(client_config()).with_handler(ClientLogger::new().with_formatter((
//!     formatters::method,
//!     " ",
//!     formatters::url,
//!     " -> ",
//!     formatters::status,
//! )));
//! ```

use crate::{ColorMode, Target, Targetable};
use std::{borrow::Cow, fmt::Display, sync::Arc, time::Instant};
use trillium_client::{ClientHandler, Conn, Result};

pub mod formatters;
pub use formatters::dev_formatter;

/// The interface to format a [`client::Conn`][Conn] as a [`Display`]-able output.
///
/// Mirrors the server-side [`LogFormatter`][crate::LogFormatter] trait, but takes a
/// [`trillium_client::Conn`] rather than a [`trillium::Conn`].
///
/// ## Implementations
///
/// `ClientLogFormatter` is implemented for:
///
/// - all 2-26-arity tuples of `ClientLogFormatter`s, output concatenated with no separator
/// - `&'static str` and `Arc<str>`, for interspersing static text
/// - `Fn(&Conn, bool) -> impl Display`, the most common way to write a custom formatter
///
/// ```rust
/// use std::borrow::Cow;
/// use trillium_client::Conn;
/// use trillium_logger::client::{ClientLogger, formatters};
///
/// fn marker(_conn: &Conn, _color: bool) -> Cow<'static, str> {
///     "[client] ".into()
/// }
///
/// ClientLogger::new().with_formatter((marker, formatters::method, " ", formatters::url));
/// ```
pub trait ClientLogFormatter: Send + Sync + 'static {
    /// The display type for this formatter.
    ///
    /// For a simple formatter, this will likely be a `String`, or even better, a lightweight type
    /// that implements [`Display`].
    type Output: Display + Send + Sync + 'static;

    /// Extract `Output` from this `Conn`.
    fn format(&self, conn: &Conn, color: bool) -> Self::Output;
}

/// Internal state inserted by [`ClientLogger::run`] and read by [`formatters::response_time`].
#[derive(Copy, Clone, Debug)]
pub(crate) struct RequestStart(pub(crate) Instant);

/// The [`ClientHandler`] that emits one log line per request.
pub struct ClientLogger<F> {
    format: F,
    color_mode: ColorMode,
    target: Arc<dyn Targetable>,
}

impl<F> std::fmt::Debug for ClientLogger<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientLogger")
            .field("color_mode", &self.color_mode)
            .finish_non_exhaustive()
    }
}

impl ClientLogger<()> {
    /// Builds a new client logger.
    ///
    /// Defaults:
    ///
    /// * formatter: [`dev_formatter`]
    /// * color mode: [`ColorMode::Auto`]
    /// * target: [`Target::Stdout`]
    pub fn new() -> ClientLogger<impl ClientLogFormatter> {
        ClientLogger {
            format: dev_formatter,
            color_mode: ColorMode::Auto,
            target: Arc::new(Target::Stdout),
        }
    }
}

impl<T> ClientLogger<T> {
    /// Replace the formatter with any type that implements [`ClientLogFormatter`].
    ///
    /// ```
    /// use trillium_logger::client::{ClientLogger, formatters};
    /// ClientLogger::new().with_formatter((formatters::method, " ", formatters::url));
    /// ```
    pub fn with_formatter<Formatter: ClientLogFormatter>(
        self,
        formatter: Formatter,
    ) -> ClientLogger<Formatter> {
        ClientLogger {
            format: formatter,
            color_mode: self.color_mode,
            target: self.target,
        }
    }
}

impl<F: ClientLogFormatter> ClientLogger<F> {
    /// Specify the color mode for this logger. See [`ColorMode`] for details.
    pub fn with_color_mode(mut self, color_mode: ColorMode) -> Self {
        self.color_mode = color_mode;
        self
    }

    /// Specify the logger target. See [`Target`] and [`Targetable`].
    pub fn with_target(mut self, target: impl Targetable) -> Self {
        self.target = Arc::new(target);
        self
    }
}

impl<F: ClientLogFormatter> ClientHandler for ClientLogger<F> {
    async fn run(&self, conn: &mut Conn) -> Result<()> {
        conn.insert_state(RequestStart(Instant::now()));
        Ok(())
    }

    async fn after_response(&self, conn: &mut Conn) -> Result<()> {
        let output = self.format.format(conn, self.color_mode.is_enabled());
        self.target.write(output.to_string());
        Ok(())
    }

    fn name(&self) -> Cow<'static, str> {
        "trillium-logger ClientLogger".into()
    }
}

/// Convenience alias for [`ClientLogger::new`].
pub fn client_logger() -> ClientLogger<impl ClientLogFormatter> {
    ClientLogger::new()
}
