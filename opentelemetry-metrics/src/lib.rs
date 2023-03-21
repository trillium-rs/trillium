//! This crate provides opentelemetry metrics conforming to
//! [semantic conventions for http](https://opentelemetry.io/docs/reference/specification/metrics/semantic_conventions/http-metrics/).
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

use opentelemetry::{
    global,
    metrics::{Histogram, Unit},
    Context, KeyValue,
};
use std::{sync::Arc, time::Instant, fmt::{Debug, Formatter, self}};
use trillium::{async_trait, Conn, Handler, Info, KnownHeaderName, Status};

type RouteFn = dyn Fn(&Conn) -> Option<String> + Send + Sync + 'static;

/// Trillium handler that instruments http.server.duration, http.server.request.size, and http.server.response.size as per
/// [semantic conventions for http](https://opentelemetry.io/docs/reference/specification/metrics/semantic_conventions/http-metrics/).
#[derive(Clone)]
pub struct Metrics {
    route: Option<Arc<RouteFn>>,
    duration_histogram: Histogram<u64>,
    request_size_histogram: Histogram<u64>,
    response_size_histogram: Histogram<u64>,
    port: Option<u16>,
}

impl Debug for Metrics {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metrics")
            .field(
                "route",
                &match self.route {
                    Some(_) => "Some(..)",
                    _ => "None",
                },
            )
            .field("duration_histogram", &self.duration_histogram)
            .field("request_size_histogram", &self.request_size_histogram)
            .field("response_size_histogram", &self.response_size_histogram)
            .field("port", &self.port)
            .finish()
    }
}

/// constructs a [`Metrics`] handler. alias for [`Metrics::new`]
pub fn metrics(meter: &'static str) -> Metrics {
    Metrics::new(meter)
}

impl Metrics {
    /// Constructs a new [`Metrics`] handler
    pub fn new(meter: &'static str) -> Self {
        let meter = global::meter(meter);
        Self {
            route: None,
            port: None,
            duration_histogram: meter
                .u64_histogram("http.server.duration")
                .with_description("Measures the duration of inbound HTTP requests.")
                .with_unit(Unit::new("ms"))
                .init(),

            request_size_histogram: meter
                .u64_histogram("http.server.request.size")
                .with_description("Measures the size of HTTP request messages (compressed).")
                .with_unit(Unit::new("By"))
                .init(),

            response_size_histogram: meter
                .u64_histogram("http.server.response.size")
                .with_description("Measures the size of HTTP response messages (compressed).")
                .with_unit(Unit::new("By"))
                .init(),
        }
    }

    /// provides a route specification to the metrics collector.
    ///
    /// in order to avoid forcing anyone to use a particular router, this is provided as a configuration hook.
    ///
    /// for use with [`trillium-router`](https://docs.trillium.rs/trillium_router/index.html),
    /// ```
    /// use trillium_router::RouterConnExt;
    /// Metrics::new("example").with_route(|conn| conn.route().map(|r| r.to_string()));
    /// ```
    pub fn with_route<F>(mut self, route: F) -> Self
    where
        F: Fn(&Conn) -> Option<String> + Send + Sync + 'static,
    {
        self.route = Some(Arc::new(route));
        self
    }
}

struct MetricsWasRun;

#[async_trait]
impl Handler for Metrics {
    async fn init(&mut self, info: &mut Info) {
        let socket_addr = info.tcp_socket_addr();
        self.port = socket_addr.map(|s| s.port());
    }

    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(MetricsWasRun)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if conn.state::<MetricsWasRun>().is_none() {
            return conn;
        }

        let Metrics {
            route,
            duration_histogram,
            request_size_histogram,
            response_size_histogram,
            port,
        } = self.clone();

        let status = (conn.status().unwrap_or(Status::NotFound) as u16).to_string();
        let route = route.and_then(|r| r(&conn));
        let start_time = conn.inner().start_time();
        let method = conn.method().to_string();
        let request_len = conn
            .headers()
            .get_str(KnownHeaderName::ContentLength)
            .and_then(|src| src.parse::<u64>().ok());
        let response_len = conn.response_len();
        let scheme = if conn.is_secure() { "https" } else { "http" };
        let host = conn.inner().host().map(String::from);
        let version = conn.inner().http_version().as_str();

        conn.inner_mut().after_send(move |_| {
            let duration_ms = (Instant::now() - start_time)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);

            let mut attributes = vec![
                KeyValue::new("http.method", method),
                KeyValue::new("http.status_code", status),
                KeyValue::new("net.protocol.name", "http"),
                KeyValue::new("http.scheme", scheme),
                KeyValue::new(
                    "net.protocol.version",
                    version.strip_prefix("HTTP/").unwrap(),
                ),
            ];

            if let Some(route) = route {
                attributes.push(KeyValue::new("http.route", route))
            };

            if let Some(host) = host {
                attributes.push(KeyValue::new("net.host.name", host));
            }

            if let Some(port) = port {
                attributes.push(KeyValue::new("net.host.port", port.to_string()));
            }

            let context = Context::current();

            duration_histogram.record(&context, duration_ms, &attributes);

            if let Some(response_len) = response_len {
                response_size_histogram.record(&context, response_len, &attributes);
            }

            if let Some(request_len) = request_len {
                request_size_histogram.record(&context, request_len, &attributes);
            }
        });

        conn
    }
}
