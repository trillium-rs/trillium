#![forbid(unsafe_code)]
#![warn(
    rustdoc::missing_crate_level_docs,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

//! Welcome to the trillium logger!
pub use crate::formatters::{apache_combined, apache_common, dev_formatter};
use std::{fmt::Display, io::IsTerminal, sync::Arc};
use trillium::{Conn, Handler, Info};
/// Components with which common log formats can be constructed
pub mod formatters;

/// A configuration option that determines if format will be colorful.
///
/// The default is [`ColorMode::Auto`], which only enables color if stdout
/// is detected to be a shell terminal (tty). If this detection is
/// incorrect, you can explicitly set it to [`ColorMode::On`] or
/// [`ColorMode::Off`]
///
/// **Note**: The actual colorization of output is determined by the log
/// formatters, so it is possible for this to be correctly enabled but for
/// the output to have no colored components.

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum ColorMode {
    /// detect if stdout is a tty
    Auto,
    /// always enable colorful output
    On,
    /// always disable colorful output
    Off,
}

impl ColorMode {
    pub(crate) fn is_enabled(&self) -> bool {
        match self {
            ColorMode::Auto => std::io::stdout().is_terminal(),
            ColorMode::On => true,
            ColorMode::Off => false,
        }
    }
}

impl Default for ColorMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Specifies where the logger output should be sent
///
/// The default is [`Target::Stdout`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Target {
    /// Send trillium logger output to a log crate backend. See
    /// [`log`] for output options
    Logger(log::Level),

    /// Send trillium logger output to stdout
    Stdout,
}

/// A trait for log targets. Implemented for [`Target`] and for all
/// `Fn(String) + Send + Sync + 'static`.
pub trait Targetable: Send + Sync + 'static {
    /// write a log line
    fn write(&self, data: String);
}

impl Targetable for Target {
    fn write(&self, data: String) {
        match self {
            Target::Logger(level) => {
                log::log!(*level, "{}", data);
            }

            Target::Stdout => {
                println!("{data}");
            }
        }
    }
}

impl<F> Targetable for F
where
    F: Fn(String) + Send + Sync + 'static,
{
    fn write(&self, data: String) {
        self(data);
    }
}

impl Default for Target {
    fn default() -> Self {
        Self::Stdout
    }
}

/// The interface to format a &[`Conn`] as a [`Display`]-able output
///
/// In general, the included loggers provide a mechanism for composing
/// these, so top level formats like [`dev_formatter`], [`apache_common`]
/// and [`apache_combined`] are composed in terms of component formatters
/// like [`formatters::method`], [`formatters::ip`],
/// [`formatters::timestamp`], and many others (see [`formatters`] for a
/// full list)
///
/// When implementing this trait, note that [`Display::fmt`] is called on
/// [`LogFormatter::Output`] _after_ the response has been fully sent, but
/// that the [`LogFormatter::format`] is called _before_ the response has
/// been sent. If you need to perform timing-sensitive calculations that
/// represent the full http cycle, move whatever data is needed to make
/// the calculation into a new type that implements Display, ensuring that
/// it is calculated at the right time.
///
///
/// ## Implementations
///
/// ### Tuples
///
/// LogFormatter is implemented for all tuples of other LogFormatter
/// types, from 2-26 formatters long. The output of these formatters is
/// concatenated with no space between.
///
/// ### `&'static str`
///
/// LogFormatter is implemented for &'static str, allowing for
/// interspersing spaces and other static formatting details into tuples.
///
/// ```rust
/// use trillium_logger::{formatters, Logger};
/// let handler = Logger::new().with_formatter(("-> ", formatters::method, " ", formatters::url));
/// ```
///
/// ### `Fn(&Conn, bool) -> impl Display`
///
/// LogFormatter is implemented for all functions that conform to this signature.
///
/// ```rust
/// # use trillium_logger::{Logger, dev_formatter};
/// # use trillium::Conn;
/// # use std::borrow::Cow;
/// # struct User(String); impl User { fn name(&self) -> &str { &self.0 } }
/// fn user(conn: &Conn, color: bool) -> Cow<'static, str> {
///     match conn.state::<User>() {
///         Some(user) => String::from(user.name()).into(),
///         None => "guest".into(),
///     }
/// }
///
/// let handler = Logger::new().with_formatter((dev_formatter, " ", user));
/// ```
pub trait LogFormatter: Send + Sync + 'static {
    /// The display type for this formatter
    ///
    /// For a simple formatter, this will likely be a String, or even
    /// better, a lightweight type that implements Display.
    type Output: Display + Send + Sync + 'static;

    /// Extract Output from this Conn
    fn format(&self, conn: &Conn, color: bool) -> Self::Output;
}

/// The trillium handler for this crate, and the core type
pub struct Logger<F> {
    format: F,
    color_mode: ColorMode,
    target: Arc<dyn Targetable>,
}

impl Logger<()> {
    /// Builds a new logger
    ///
    /// Defaults:
    ///
    /// * formatter: [`dev_formatter`]
    /// * color mode: [`ColorMode::Auto`]
    /// * target: [`Target::Stdout`]
    pub fn new() -> Logger<impl LogFormatter> {
        Logger {
            format: dev_formatter,
            color_mode: ColorMode::Auto,
            target: Arc::new(Target::Stdout),
        }
    }
}

impl<T> Logger<T> {
    /// replace the formatter with any type that implements [`LogFormatter`]
    ///
    /// see the trait documentation for [`LogFormatter`] for more details. note that this can be
    /// chained with [`Logger::with_target`] and [`Logger::with_color_mode`]
    ///
    /// ```
    /// use trillium_logger::{apache_common, Logger};
    /// Logger::new().with_formatter(apache_common("-", "-"));
    /// ```
    pub fn with_formatter<Formatter: LogFormatter>(
        self,
        formatter: Formatter,
    ) -> Logger<Formatter> {
        Logger {
            format: formatter,
            color_mode: self.color_mode,
            target: self.target,
        }
    }
}

impl<F: LogFormatter> Logger<F> {
    /// specify the color mode for this logger.
    ///
    /// see [`ColorMode`] for more details. note that this can be chained
    /// with [`Logger::with_target`] and [`Logger::with_formatter`]
    /// ```
    /// use trillium_logger::{ColorMode, Logger};
    /// Logger::new().with_color_mode(ColorMode::On);
    /// ```
    pub fn with_color_mode(mut self, color_mode: ColorMode) -> Self {
        self.color_mode = color_mode;
        self
    }

    /// specify the logger target
    ///
    /// see [`Target`] for more details. note that this can be chained
    /// with [`Logger::with_color_mode`] and [`Logger::with_formatter`]
    ///
    /// ```
    /// use trillium_logger::{Logger, Target};
    /// Logger::new().with_target(Target::Logger(log::Level::Info));
    /// ```
    pub fn with_target(mut self, target: impl Targetable) -> Self {
        self.target = Arc::new(target);
        self
    }
}

struct LoggerWasRun;

impl<F> Handler for Logger<F>
where
    F: LogFormatter,
{
    async fn init(&mut self, info: &mut Info) {
        self.target.write(format!(
            "
🌱🦀🌱 {} started
Listening at {}{}

Control-C to quit",
            info.server_description(),
            info.listener_description(),
            info.tcp_socket_addr()
                .map(|s| format!(" (bound as tcp://{s})"))
                .unwrap_or_default(),
        ));
    }

    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(LoggerWasRun)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if conn.state::<LoggerWasRun>().is_some() {
            let target = self.target.clone();
            let output = self.format.format(&conn, self.color_mode.is_enabled());
            conn.inner_mut()
                .after_send(move |_| target.write(output.to_string()));
        }

        conn
    }
}

/// Convenience alias for [`Logger::new`]
pub fn logger() -> Logger<impl LogFormatter> {
    Logger::new()
}
