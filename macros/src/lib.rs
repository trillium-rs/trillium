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
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]
#![doc = include_str!("../README.md")]

mod handler;
/// see crate docs
#[proc_macro_derive(Handler, attributes(handler))]
pub fn derive_handler(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    handler::derive_handler(input)
}

mod transport;
///
#[proc_macro_derive(Transport, attributes(transport))]
pub fn derive_transport(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    transport::derive_transport(input)
}

mod async_read;
///
#[proc_macro_derive(AsyncRead, attributes(async_read, async_io))]
pub fn derive_async_read(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    async_read::derive_async_read(input)
}

mod async_write;
///
#[proc_macro_derive(AsyncWrite, attributes(async_write, async_io))]
pub fn derive_async_write(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    async_write::derive_async_write(input)
}
