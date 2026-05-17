//! Shared test fixtures for trillium-http end-to-end protocol tests.
//!
//! Each submodule binds a real `127.0.0.1:0` socket and orchestrates trillium-http's
//! per-protocol drivers directly — no `trillium::Handler` bridge, no
//! `trillium-server-common` dispatch. Tests write handlers as plain
//! `Fn(Conn<T>) -> Fut`, optionally paired with an upgrade closure.

#![allow(dead_code)]

pub mod h1;
pub mod h2c;
pub mod h3;
