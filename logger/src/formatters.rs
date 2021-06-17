use crate::LogFormatter;
use colored::{ColoredString, Colorize};
use size::{Base, Size, Style};
use std::{borrow::Cow, fmt::Display, time::Instant};
use trillium::{
    http_types::{Method, StatusCode},
    Conn,
};

pub fn apache_combined<RequestId, UserId>(
    request_id: RequestId,
    user_id: UserId,
) -> impl LogFormatter + 'static
where
    RequestId: LogFormatter + Send + Sync + 'static,
    UserId: LogFormatter + Send + Sync + 'static,
{
    let formatter = (
        apache_common(request_id, user_id),
        " ",
        header("referrer"),
        " ",
        header("user-agent"),
    );

    move |conn: &Conn, color: bool| formatter.format(conn, color)
}

pub fn method(conn: &Conn, _color: bool) -> Method {
    conn.method()
}

pub fn dev_formatter(conn: &Conn, color: bool) -> impl Display + Send + 'static {
    (method, " ", url, " ", response_time, " ", status).format(conn, color)
}

pub fn ip(conn: &Conn, _color: bool) -> Cow<'static, str> {
    match conn.inner().peer_ip() {
        Some(peer) => format!("{:?}", peer).into(),
        None => "-".into(),
    }
}

#[derive(Copy, Clone)]
pub struct StatusOutput(StatusCode, bool);
impl Display for StatusOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let StatusOutput(status, color) = *self;
        let status_string = (status as u16).to_string();
        if color {
            f.write_fmt(format_args!(
                "{}",
                status_string.color(match status as u16 {
                    200..=299 => "green",
                    300..=399 => "cyan",
                    400..=499 => "yellow",
                    500..=599 => "red",
                    _ => "white",
                })
            ))
        } else {
            f.write_str(&status_string)
        }
    }
}

pub fn status(conn: &Conn, color: bool) -> StatusOutput {
    StatusOutput(conn.status().unwrap_or(StatusCode::NotFound), color)
}

pub fn header(header_name: &'static str) -> impl LogFormatter + 'static {
    move |conn: &Conn, _color: bool| {
        format!(
            "{:?}",
            conn.headers()
                .get(header_name)
                .map(|h| h.as_str())
                .unwrap_or("")
        )
    }
}

pub fn timestamp(_conn: &Conn, _color: bool) -> String {
    chrono::offset::Local::now()
        .format("%d/%b/%Y:%H:%M:%S %z")
        .to_string()
}

pub fn body_len_human(conn: &Conn, _color: bool) -> Cow<'static, str> {
    conn.response_len()
        .map(|l| Size::to_string(&Size::Bytes(l), Base::Base10, Style::Smart).into())
        .unwrap_or_else(|| Cow::from("-"))
}

pub fn apache_common<RequestId, UserId>(
    request_id: RequestId,
    user_id: UserId,
) -> impl LogFormatter + 'static
where
    RequestId: LogFormatter + Send + Sync + 'static,
    UserId: LogFormatter + Send + Sync + 'static,
{
    let formatter = (
        ip, " ", request_id, " ", user_id, " [", timestamp, "] \"", method, " ", url, " ", version,
        "\" ", status, " ", bytes,
    );

    move |conn: &Conn, color: bool| formatter.format(conn, color)
}

pub fn bytes(conn: &trillium::Conn, _color: bool) -> u64 {
    conn.response_len().unwrap_or_default()
}

pub fn url(conn: &trillium::Conn, _color: bool) -> String {
    match conn.querystring() {
        "" => conn.path().into(),
        query => format!("{}?{}", conn.path(), query),
    }
}

pub struct ResponseTimeOutput(Instant);
impl Display for ResponseTimeOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", Instant::now() - self.0))
    }
}

pub fn response_time(conn: &trillium::Conn, _color: bool) -> ResponseTimeOutput {
    ResponseTimeOutput(conn.inner().start_time())
}

pub fn version(conn: &trillium::Conn, _color: bool) -> trillium::http_types::Version {
    conn.inner().http_version()
}

impl LogFormatter for &'static str {
    type Output = Self;
    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        self
    }
}

impl LogFormatter for ColoredString {
    type Output = String;
    fn format(&self, _conn: &Conn, color: bool) -> Self::Output {
        if color {
            self.to_string()
        } else {
            (&**self).to_string()
        }
    }
}

impl<F, O> LogFormatter for F
where
    F: Fn(&Conn, bool) -> O + Send + Sync + 'static,
    O: Display + Send + Sync + 'static,
{
    type Output = O;
    fn format(&self, conn: &Conn, color: bool) -> Self::Output {
        self(conn, color)
    }
}

pub struct TupleOutput<O>(O);
macro_rules! impl_formatter_tuple {
    ($($name:ident)+) => (
        #[allow(non_snake_case)]
        impl<$($name,)*> Display for TupleOutput<($($name,)*)> where $($name: Display + Send + Sync + 'static,)* {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let ($(ref $name,)*) = self.0;
                f.write_fmt(format_args!(
                    concat!($(
                        concat!("{",stringify!($name) ,":}")
                    ),*),
                    $($name = ($name)),*
                ))
            }
        }

        #[allow(non_snake_case)]
        impl<$($name),*> LogFormatter for ($($name,)*) where $($name: LogFormatter),* {
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
