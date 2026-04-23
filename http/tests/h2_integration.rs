//! Integration tests for `trillium-http`'s HTTP/2 implementation, speaking to hyper's `h2` crate as
//! a conformant peer over an in-memory duplex.
//!
//! Tests grow per phase. Phase 1 covers preface + SETTINGS handshake + PING + GOAWAY (added once
//! `H2Connection` exists). For now this file exists only so the harness / dev-deps are wired.

#[allow(unused_imports)]
use h2 as _;
#[allow(unused_imports)]
use tokio as _;
