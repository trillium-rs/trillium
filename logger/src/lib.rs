// #![forbid(unsafe_code)]
// #![warn(
//     missing_crate_level_docs,
//     missing_debug_implementations,
//     nonstandard_style,
//     unused_qualifications
// )]

pub use crate::formatters::{apache_combined, apache_common, dev_formatter};
use std::fmt::Display;
use trillium::{async_trait, Conn, Handler, Info};
pub mod formatters;

#[derive(Clone, Copy, Debug)]
pub enum ColorMode {
    Auto,
    On,
    Off,
}

impl ColorMode {
    pub fn is_enabled(&self) -> bool {
        match self {
            ColorMode::Auto => atty::is(atty::Stream::Stdout),
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

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Target {
    Logger(log::Level),
    Stdout,
}

impl Target {
    pub fn write(&self, data: impl Display) {
        match self {
            Target::Logger(level) => {
                log::log!(*level, "{}", data);
            }

            Target::Stdout => {
                println!("{}", data);
            }
        }
    }
}

impl Default for Target {
    fn default() -> Self {
        Self::Logger(log::Level::Info)
    }
}

pub trait LogFormatter: Send + Sync {
    type Output: Display + Send + Sync + 'static;
    fn format(&self, conn: &Conn, color: bool) -> Self::Output;
}

pub struct Logger<F> {
    format: F,
    color_mode: ColorMode,
    target: Target,
}

impl Logger<()> {
    pub fn new() -> Logger<impl LogFormatter> {
        Logger {
            format: dev_formatter,
            color_mode: ColorMode::Auto,
            target: Target::Logger(log::Level::Info),
        }
    }
}

impl<T> Logger<T> {
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
    pub fn with_color_mode(mut self, color_mode: ColorMode) -> Self {
        self.color_mode = color_mode;
        self
    }

    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
}

struct LoggerWasRun;

#[async_trait]
impl<F> Handler for Logger<F>
where
    F: LogFormatter + 'static,
{
    async fn init(&mut self, info: &mut Info) {
        self.target.write(&format!(
            "
🌱🦀🌱 {} started
Listening at {}{}

Control-C to quit",
            info.server_description(),
            info.listener_description(),
            info.tcp_socket_addr()
                .map(|s| format!(" (bound as tcp://{})", s))
                .unwrap_or_default(),
        ));
    }
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(LoggerWasRun)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if conn.state::<LoggerWasRun>().is_some() {
            let target = self.target;
            let output = self.format.format(&conn, self.color_mode.is_enabled());
            conn.inner_mut().after_send(move |_| target.write(output));
        }

        conn
    }
}
