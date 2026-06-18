//! Wire-level tests for [`H2Driver`], driven through the `DriverFixture` harness in
//! [`fixture`]: write frames as "the peer", tick the driver, assert on the bytes it emits
//! and (where relevant) its internal stream/driver state.
//!
//! These are deliberately implementation-agnostic at the protocol layer — they assert
//! wire-observable behavior, so they survive the planned stream-state redesign unchanged
//! and act as its safety net. The enumerated matrix these grow toward lives in the
//! `h2-stream-lifecycle-test-gaps` memory (+ the `internal/h2-stream-state-redesign.md`
//! design doc).
//!
//! Split by area: [`fixture`] (harness + sanity), [`lifecycle`] (§5.1 / upgrade close
//! orderings), [`shutdown`] (GOAWAY / Closing→Drained), [`flow_control`] (windows + recv
//! cap).

mod client;
mod fixture;
mod flow_control;
mod lifecycle;
mod priority;
mod recv_headers;
mod send;
mod shutdown;
mod two_driver;
