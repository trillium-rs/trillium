use opentelemetry::{
    global::set_meter_provider,
    runtime::Tokio,
    sdk::{
        export::metrics::aggregation::stateless_temporality_selector,
        metrics::selectors::simple::histogram,
    },
};
use opentelemetry_otlp::{new_exporter, new_pipeline};
use trillium_opentelemetry_metrics::Metrics;
use trillium_router::{router, RouterConnExt};

fn set_up_collector() {
    // you probably don't want to copy this, it's just some random
    // configuration to get data flowing into otelcol
    let buckets: Vec<f64> = vec![0.0, 10.0, 20.0, 50.0, 100.0, 500.0];
    set_meter_provider(
        new_pipeline()
            .metrics(histogram(buckets), stateless_temporality_selector(), Tokio)
            .with_exporter(new_exporter().tonic())
            .build()
            .unwrap(),
    );
}

#[tokio::main]
pub async fn main() {
    set_up_collector();

    trillium_tokio::run_async((
        Metrics::new("example-app").with_route(|conn| conn.route().map(|r| r.to_string())),
        router().get("/some/:path", "ok"),
    ))
    .await;
}
