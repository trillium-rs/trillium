use crate::LogFormatter;
use chrono::Local;
use colored::{ColoredString, Colorize};
use size::{Base, Size};
use std::{borrow::Cow, fmt::Display, sync::Arc, time::Instant};
use trillium::{Conn, Method, Status, Version};

/**
[apache combined log format][apache]

[apache]: https://httpd.apache.org/docs/current/logs.html#combined

This is defined as follows:

[`apache_combined`](`request_id`, `user_id`) [`header`]`("referrer")` [`header`]`("user-agent")`

where `request_id` and `user_id` are mandatory formatters provided at time of usage.


## usage with empty `request_id` and `user_id`
```
# use trillium_logger::{Logger, apache_combined};
Logger::new().with_formatter(apache_combined("-", "-"));
```

## usage with an app-specific `user_id`

```
# use trillium_logger::{Logger, apache_combined};
# use trillium::Conn;
# use std::borrow::Cow;
# struct User(String); impl User { fn name(&self) -> &str { &self.0 } }
fn user(conn: &Conn, color: bool) -> Cow<'static, str> {
     match conn.state::<User>() {
        Some(user) => String::from(user.name()).into(),
        None => "guest".into()
    }
}

Logger::new().with_formatter(apache_combined("-", user));
```
*/
pub fn apache_combined(
    request_id: impl LogFormatter,
    user_id: impl LogFormatter,
) -> impl LogFormatter {
    (
        apache_common(request_id, user_id),
        " ",
        header("referrer"),
        " ",
        header("user-agent"),
    )
}

/**
formatter for the conn's http method that delegates to [`Method`]'s
[`Display`] implementation
*/
pub fn method(conn: &Conn, _color: bool) -> Method {
    conn.method()
}

/**
simple development-mode formatter

composed of

`"`[`method`] [`url`] [`response_time`] [`status`]`"`
*/
pub fn dev_formatter(conn: &Conn, color: bool) -> impl Display + Send + 'static {
    (method, " ", url, " ", response_time, " ", status).format(conn, color)
}

/**
formatter for the peer ip address of the connection

**note**: this can be modified by handlers prior to logging, such as
when running a trillium application behind a reverse proxy or load
balancer that sets a `forwarded` or `x-forwarded-for` header. this
will display `"-"` if there is no available peer ip address, such as
when running on a runtime adapter that does not have access to this
information
*/
pub fn ip(conn: &Conn, _color: bool) -> Cow<'static, str> {
    match conn.inner().peer_ip() {
        Some(peer) => format!("{:?}", peer).into(),
        None => "-".into(),
    }
}

mod status_mod {
    use super::*;
    /**
    The display type for [`status`]
    */
    #[derive(Copy, Clone)]
    pub struct StatusOutput(Status, bool);
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

    /**
    formatter for the http status

    displays just the numeric code of the
    status. when color is enabled, it uses the following color encoding:

    | code | color  |
    |------|--------|
    | 2xx  | green  |
    | 3xx  | cyan   |
    | 4xx  | yellow |
    | 5xx  | red    |
    | ???  | white  |
    */
    pub fn status(conn: &Conn, color: bool) -> StatusOutput {
        StatusOutput(conn.status().unwrap_or(Status::NotFound), color)
    }
}

pub use status_mod::status;

/**
formatter-builder for a particular header, formatted wrapped in
quotes. `""` if the header is not present

usage:

```rust
# use trillium_logger::{Logger, formatters::header};
Logger::new().with_formatter(("user-agent: ", header("user-agent")));
```

**note**: this is not a formatter itself, but returns a formatter when
called with a header name
*/
pub fn header(header_name: &'static str) -> impl LogFormatter {
    move |conn: &Conn, _color: bool| {
        format!("{:?}", conn.headers().get_str(header_name).unwrap_or(""))
    }
}

mod timestamp_mod {
    use super::*;
    /**
    Display output for [`timestamp`]
    */
    pub struct Now;

    /**
    formatter for the current timestamp. this represents the time that the
    log is written, not the beginning timestamp of the request
    */
    pub fn timestamp(_conn: &Conn, _color: bool) -> Now {
        Now
    }

    impl Display for Now {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_fmt(format_args!(
                "{}",
                Local::now().format("%d/%b/%Y:%H:%M:%S %z")
            ))
        }
    }
}
pub use timestamp_mod::timestamp;

/**
formatter for the response body length, represented as a
human-readable string like `5 bytes` or `10.1mb`. prints `-` if there
is no response body. see [`bytes`] for the raw number of bytes
*/
pub fn body_len_human(conn: &Conn, _color: bool) -> Cow<'static, str> {
    conn.response_len()
        .map(|l| {
            Size::from_bytes(l)
                .format()
                .with_base(Base::Base10)
                .to_string()
                .into()
        })
        .unwrap_or_else(|| Cow::from("-"))
}

/**
[apache common log format][apache]

[apache]: https://httpd.apache.org/docs/current/logs.html#common

This is defined as follows:

[`ip`] `request_id` `user_id` `\[`[`timestamp`]`\]` "[`method`] [`url`] [`version`]" [`status`] [`bytes`]

where `request_id` and `user_id` are mandatory formatters provided at time of usage.

## usage without `request_id` or `user_id`
```
# use trillium_logger::{Logger, apache_common};
Logger::new().with_formatter(apache_common("-", "-"));
```

## usage with app-specific `user_id`
```
# use trillium_logger::{Logger, apache_common};
# use trillium::Conn;
# use std::borrow::Cow;
# struct User(String); impl User { fn name(&self) -> &str { &self.0 } }
fn user(conn: &Conn, color: bool) -> Cow<'static, str> {
     match conn.state::<User>() {
        Some(user) => String::from(user.name()).into(),
        None => "guest".into()
    }
}

Logger::new().with_formatter(apache_common("-", user));
```
*/
pub fn apache_common(
    request_id: impl LogFormatter,
    user_id: impl LogFormatter,
) -> impl LogFormatter {
    (
        ip, " ", request_id, " ", user_id, " [", timestamp, "] \"", method, " ", url, " ", version,
        "\" ", status, " ", bytes,
    )
}

/**
formatter that prints the number of response body bytes as a
number. see [`body_len_human`] for a human-readable response body
length with units
*/
pub fn bytes(conn: &Conn, _color: bool) -> u64 {
    conn.response_len().unwrap_or_default()
}

/**
formatter that prints an emoji if the request is secure as determined
by [`Conn::is_secure`]
*/
pub fn secure(conn: &Conn, _: bool) -> &'static str {
    if conn.is_secure() {
        "🔒"
    } else {
        "  "
    }
}

/**
formatter for the current url or path of the request, including query
*/
pub fn url(conn: &Conn, _color: bool) -> String {
    match conn.querystring() {
        "" => conn.path().into(),
        query => format!("{}?{}", conn.path(), query),
    }
}

mod response_time_mod {
    use super::*;
    /**
    display output type for the [`response_time`] formatter
    */
    pub struct ResponseTimeOutput(Instant);
    impl Display for ResponseTimeOutput {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_fmt(format_args!("{:?}", Instant::now() - self.0))
        }
    }

    /**
    formatter for the wall-time duration with units that this http
    request-response cycle took, from the first bytes read to the
    completion of the response.
    */
    pub fn response_time(conn: &Conn, _color: bool) -> ResponseTimeOutput {
        ResponseTimeOutput(conn.inner().start_time())
    }
}

pub use response_time_mod::response_time;

/**
formatter for the http version, as delegated to the display
implementation of [`Version`]
*/
pub fn version(conn: &Conn, _color: bool) -> Version {
    conn.inner().http_version()
}

impl LogFormatter for &'static str {
    type Output = Self;
    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        self
    }
}

impl LogFormatter for Arc<str> {
    type Output = Self;
    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        Arc::clone(self)
    }
}

impl LogFormatter for ColoredString {
    type Output = String;
    fn format(&self, _conn: &Conn, color: bool) -> Self::Output {
        if color {
            self.to_string()
        } else {
            (**self).to_string()
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

mod tuples {
    use super::*;
    /**
    display output for the tuple implementation

    The Display type of each tuple element is contained in this type, and
    it implements [`Display`] for 2-26-arity tuples.

    Please open an issue if you find yourself needing to do something with
    this other than [`Display`] it.
    */
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
}
