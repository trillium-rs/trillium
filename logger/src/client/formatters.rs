//! Building-block formatters for [`ClientLogger`][super::ClientLogger].
//!
//! Compose them into tuples to build a full log line. The default top-level format is
//! [`dev_formatter`].

use super::{ClientLogFormatter, RequestStart};
use colored::{ColoredString, Colorize};
use size::{Base, Size};
use std::{borrow::Cow, fmt::Display, sync::Arc, time::Instant};
use trillium_client::{Conn, ConnExt, HeaderName, KnownHeaderName, Method, Status, Version};

/// The default development-mode formatter.
///
/// Composed of:
///
/// `"`[`version`] [`method`] [`url()`] [`status`] [`response_time`][`error`]`"`
///
/// The [`error()`] component is empty on success. When the transport failed, it renders as
/// ` <error message>` — the leading space is part of the formatter, so the format string is
/// concatenation, not separator-joined.
pub fn dev_formatter(conn: &Conn, color: bool) -> impl Display + Send + 'static + use<> {
    (
        version,
        " ",
        method,
        " ",
        url,
        " ",
        status,
        " ",
        response_time,
        error,
    )
        .format(conn, color)
}

/// Formatter for the conn's HTTP method.
pub fn method(conn: &Conn, _color: bool) -> Method {
    conn.method()
}

/// Formatter for the full request URL (scheme, host, path, query).
pub fn url(conn: &Conn, _color: bool) -> String {
    conn.url().to_string()
}

/// Formatter for the HTTP version used on the wire.
///
/// Because log output renders after the request executes, this reflects the version actually
/// negotiated — an h2→h3 upgrade via `Alt-Svc` shows up here, not the originally-requested
/// version.
pub fn version(conn: &Conn, _color: bool) -> Version {
    conn.http_version()
}

mod status_mod {
    use super::*;
    /// Display output for [`status`].
    #[derive(Copy, Clone)]
    pub struct StatusOutput(Option<Status>, bool);

    impl Display for StatusOutput {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let StatusOutput(status, color) = *self;
            let Some(status) = status else {
                return f.write_str("---");
            };
            let s = (status as u16).to_string();
            if color {
                f.write_fmt(format_args!(
                    "{}",
                    s.color(match status as u16 {
                        200..=299 => "green",
                        300..=399 => "cyan",
                        400..=499 => "yellow",
                        500..=599 => "red",
                        _ => "white",
                    })
                ))
            } else {
                f.write_str(&s)
            }
        }
    }

    /// Formatter for the HTTP response status.
    ///
    /// Displays the numeric status code, or `---` if no response was received. With color enabled,
    /// 2xx is green, 3xx cyan, 4xx yellow, 5xx red.
    pub fn status(conn: &Conn, color: bool) -> StatusOutput {
        StatusOutput(conn.status(), color)
    }
}

pub use status_mod::status;

/// Formatter-builder for a particular request header, wrapped in quotes. Produces `""` if the
/// header is not present.
pub fn request_header(header_name: impl Into<HeaderName<'static>>) -> impl ClientLogFormatter {
    let header_name = header_name.into();
    move |conn: &Conn, _color: bool| {
        format!(
            "{:?}",
            conn.request_headers()
                .get_str(header_name.clone())
                .unwrap_or("")
        )
    }
}

/// Formatter-builder for a particular response header, wrapped in quotes. Produces `""` if the
/// header is not present.
pub fn response_header(header_name: impl Into<HeaderName<'static>>) -> impl ClientLogFormatter {
    let header_name = header_name.into();
    move |conn: &Conn, _color: bool| {
        format!(
            "{:?}",
            conn.response_headers()
                .get_str(header_name.clone())
                .unwrap_or("")
        )
    }
}

mod timestamp_mod {
    use super::*;
    use time::{OffsetDateTime, macros::format_description};

    /// Display output for [`timestamp`].
    pub struct Now;

    /// Formatter for the current timestamp at log-write time (apache format,
    /// `10/Oct/2000:13:55:36 -0700`).
    pub fn timestamp(_conn: &Conn, _color: bool) -> Now {
        Now
    }

    impl Display for Now {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let now = OffsetDateTime::now_local()
                .unwrap_or_else(|_| OffsetDateTime::now_utc())
                .format(format_description!(
                    version = 2,
                    "[day]/[month repr:short]/[year repr:full]:[hour repr:24]:[minute]:[second] \
                     [offset_hour sign:mandatory][offset_minute]"
                ))
                .unwrap();
            f.write_str(&now)
        }
    }
}

pub use timestamp_mod::timestamp;

/// Formatter for the response Content-Length as a human-readable string (`5 bytes`, `10.1 kb`).
/// Produces `-` if no Content-Length is set.
pub fn body_len_human(conn: &Conn, _color: bool) -> Cow<'static, str> {
    response_content_length(conn)
        .map(|l| {
            Size::from_bytes(l)
                .format()
                .with_base(Base::Base10)
                .to_string()
                .into()
        })
        .unwrap_or_else(|| Cow::from("-"))
}

/// Formatter for the response Content-Length as a raw byte count, `0` if unknown.
pub fn bytes(conn: &Conn, _color: bool) -> u64 {
    response_content_length(conn).unwrap_or_default()
}

fn response_content_length(conn: &Conn) -> Option<u64> {
    conn.response_headers()
        .get_str(KnownHeaderName::ContentLength)
        .and_then(|s| s.parse().ok())
}

/// Formatter for whether the request used a TLS-bearing scheme (https/wss).
pub fn secure(conn: &Conn, _color: bool) -> &'static str {
    match conn.url().scheme() {
        "https" | "wss" => "🔒",
        _ => "  ",
    }
}

mod response_time_mod {
    use super::*;

    /// Display output for [`response_time`].
    pub struct ResponseTimeOutput(Option<Instant>);

    impl Display for ResponseTimeOutput {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self.0 {
                Some(start) => f.write_fmt(format_args!("{:?}", Instant::now() - start)),
                None => f.write_str("-"),
            }
        }
    }

    /// Formatter for the wall-clock duration between when
    /// [`ClientLogger`][super::super::ClientLogger] first ran and when the log line is rendered.
    ///
    /// If no [`ClientLogger`][super::super::ClientLogger] preceded this in the handler chain,
    /// prints `-`.
    pub fn response_time(conn: &Conn, _color: bool) -> ResponseTimeOutput {
        ResponseTimeOutput(conn.state::<RequestStart>().map(|RequestStart(i)| *i))
    }
}

pub use response_time_mod::response_time;

mod error_mod {
    use super::*;

    /// Display output for [`error`].
    pub struct ErrorOutput(Option<String>, bool);

    impl Display for ErrorOutput {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let Some(msg) = &self.0 else {
                return Ok(());
            };
            f.write_str(" ")?;
            if self.1 {
                f.write_fmt(format_args!("{}", msg.as_str().red()))
            } else {
                f.write_str(msg)
            }
        }
    }

    /// Formatter for the transport-level error stashed on the conn, if any.
    ///
    /// Renders as ` <error message>` (with a leading space) when an error is present, empty
    /// otherwise. The leading space is built in so composing into a tuple looks like
    /// concatenation, not separator-joining; place this where you want the error to land
    /// without inserting your own separator.
    ///
    /// With color enabled, the error message renders in red.
    pub fn error(conn: &Conn, color: bool) -> ErrorOutput {
        ErrorOutput(conn.error().map(ToString::to_string), color)
    }
}

pub use error_mod::error;

impl ClientLogFormatter for &'static str {
    type Output = Self;

    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        self
    }
}

impl ClientLogFormatter for Arc<str> {
    type Output = Self;

    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        Arc::clone(self)
    }
}

impl ClientLogFormatter for ColoredString {
    type Output = String;

    fn format(&self, _conn: &Conn, color: bool) -> Self::Output {
        if color {
            self.to_string()
        } else {
            (**self).to_string()
        }
    }
}

impl<F, O> ClientLogFormatter for F
where
    F: Fn(&Conn, bool) -> O + Send + Sync + 'static,
    O: Display + Send + Sync + 'static,
{
    type Output = O;

    fn format(&self, conn: &Conn, color: bool) -> Self::Output {
        self(conn, color)
    }
}

mod tuples {
    use super::*;

    /// Display output for the tuple implementation. Implements [`Display`] for 2-26-arity tuples
    /// of `Display` types.
    pub struct TupleOutput<O>(O);

    macro_rules! impl_formatter_tuple {
        ($($name:ident)+) => (
            #[allow(non_snake_case)]
            impl<$($name,)*> Display for TupleOutput<($($name,)*)>
            where
                $($name: Display + Send + Sync + 'static,)*
            {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    let ($(ref $name,)*) = self.0;
                    f.write_fmt(format_args!(
                        concat!($(concat!("{", stringify!($name), ":}")),*),
                        $($name = ($name)),*
                    ))
                }
            }

            #[allow(non_snake_case)]
            impl<$($name),*> ClientLogFormatter for ($($name,)*)
            where
                $($name: ClientLogFormatter),*
            {
                type Output = TupleOutput<($($name::Output,)*)>;
                fn format(&self, conn: &Conn, color: bool) -> Self::Output {
                    let ($(ref $name,)*) = *self;
                    TupleOutput(($(($name).format(conn, color),)*))
                }
            }
        )
    }

    impl_formatter_tuple! { A B }
    impl_formatter_tuple! { A B C }
    impl_formatter_tuple! { A B C D }
    impl_formatter_tuple! { A B C D E }
    impl_formatter_tuple! { A B C D E F }
    impl_formatter_tuple! { A B C D E F G }
    impl_formatter_tuple! { A B C D E F G H }
    impl_formatter_tuple! { A B C D E F G H I }
    impl_formatter_tuple! { A B C D E F G H I J }
    impl_formatter_tuple! { A B C D E F G H I J K }
    impl_formatter_tuple! { A B C D E F G H I J K L }
    impl_formatter_tuple! { A B C D E F G H I J K L M }
    impl_formatter_tuple! { A B C D E F G H I J K L M N }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U V }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U V W }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U V W X }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U V W X Y }
    impl_formatter_tuple! { A B C D E F G H I J K L M N O P Q R S T U V W X Y Z }
}
