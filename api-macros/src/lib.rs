#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::nursery, clippy::cargo)]
#![allow(
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::option_if_let_else
)]
#![doc = include_str!("../README.md")]

mod attributes;
#[cfg(test)]
mod coverage_tests;
mod handler;
mod try_from_conn;

/// Derives [`trillium_api::TryFromConn`] for a struct.
///
/// The struct must carry an `#[api(...)]` attribute selecting the extraction strategy:
///
/// - `state` — extract `Self` from conn state via `take_state`
/// - `state, clone` — extract a clone of `Self` from conn state, leaving it in place
/// - `json` — deserialize `Self` from a JSON request body
/// - `body` — deserialize `Self` from the request body using content negotiation
///
/// `err = Type` overrides the associated `Error` type. The provided type must implement `Default`,
/// and (for `json` / `body`) the original error is discarded via `map_err`.
///
/// # Examples
///
/// ```
/// use trillium_api::{Halt, TryFromConn};
///
/// #[derive(Clone, TryFromConn)]
/// #[api(state, clone, err = Halt)]
/// struct CurrentUser {
///     name: String,
/// }
/// ```
#[proc_macro_derive(TryFromConn, attributes(api))]
pub fn derive_try_from_conn(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    try_from_conn::derive(input)
}

/// Derives [`trillium::Handler`] for a struct, where the handler implementation reflects an
/// `#[api(...)]` strategy:
///
/// - `state` — `conn.with_state(self.clone())` (requires `Self: Clone`)
/// - `json` — `conn.with_json(&*self)` (requires `Self: Serialize`)
/// - `body` — content-negotiated `conn.serialize(&*self)` (requires `Self: Serialize`)
///
/// This is a different `Handler` derive than the field-delegating one in `trillium-macros`; pick
/// whichever fits the use case. (When both crates' derives are imported, alias one of them.)
#[proc_macro_derive(Handler, attributes(api))]
pub fn derive_handler(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    handler::derive(input)
}
