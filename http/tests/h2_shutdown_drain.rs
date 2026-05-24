//! Regression test for a h2 graceful-shutdown drain deadlock
//!
//! - **Client** races its in-flight request future against a deadline and *drops* it on expiry (an
//!   external `runtime().timeout(..)`, exactly grpc's `with_deadline` → `race_against_deadline` —
//!   not trillium-client's own timeout). The drop tears down the `Conn`/`H2Transport` mid-request.
//! - **Server** handler is slow: it races its work against the same deadline and writes a late
//!   response on expiry, so the response submit coincides with shutdown.
//!
//! The bug was that the server driver could reach `Drained` and finish while the handler was still
//! in flight (its stream half-closed-remote, no response yet), orphaning the response `SubmitSend`
//! — the handler then blocks forever holding a swansong guard, and `shut_down()` never returns. A
//! watchdog turns that hang into a test failure. Runs a handful of iterations because the race is
//! scheduling-sensitive.

mod common;

use common::h2c::H2cServer;
use std::{future::IntoFuture, time::Duration};
use test_harness::test;
use trillium_client::{Client, Version};
use trillium_http::Status;
use trillium_testing::{Runtime, TestResult, harness, runtime};

const DEADLINE: Duration = Duration::from_millis(50);
const SLOW_WORK: Duration = Duration::from_millis(500);
const SHUTDOWN_WATCHDOG: Duration = Duration::from_secs(5);
const ITERATIONS: usize = 20;

#[test(harness)]
async fn deadline_dropped_request_then_shutdown_does_not_hang() -> TestResult {
    let rt: Runtime = runtime().into();

    for i in 0..ITERATIONS {
        let server = H2cServer::new(|mut conn| async move {
            let rt: Runtime = runtime().into();
            // Drain the request body (mirrors grpc's `read_one`).
            let _ = conn.request_body().read_string().await;
            // Slow handler: race the work against the deadline, write a late response on expiry —
            // so the response submit coincides with the shutdown drain.
            let delay_rt = rt.clone();
            match rt
                .timeout(DEADLINE, async move { delay_rt.delay(SLOW_WORK).await })
                .await
            {
                Some(()) => conn.with_status(Status::Ok).with_response_body("ok"),
                None => conn.with_status(Status::ServiceUnavailable),
            }
        })
        .await;

        let client = Client::new(trillium_smol::ClientConfig::default());
        let request = client
            .post(server.base_url())
            .with_http_version(Version::Http2)
            .with_body("hello");

        // Drop the in-flight request future on the deadline (external timeout, not the client's
        // own). On expiry the future — parked awaiting the response — is dropped, resetting the
        // client's stream while its driver task keeps running.
        drop(rt.timeout(DEADLINE, request.into_future()).await);

        rt.timeout(SHUTDOWN_WATCHDOG, server.shut_down())
            .await
            .unwrap_or_else(|| panic!("server shut_down hung on iteration {i}"));
    }

    Ok(())
}
