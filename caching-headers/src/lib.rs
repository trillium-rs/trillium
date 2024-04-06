/*!
# Trillium handlers for etag and last-modified-since headers.

This crate provides three handlers: [`Etag`], [`Modified`], and
[`CachingHeaders`], as well as a [`CachingHeadersExt`] that extends
[`trillium::Headers`] with some accessors.

Unless you are sure that you _don't_ want either etag or last-modified
behavior, please use the combined [`CachingHeaders`] handler.

 */
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

mod etag;
pub use crate::etag::Etag;
pub use ::etag::EntityTag;

mod modified;
pub use modified::Modified;

mod caching_conn_ext;
pub use caching_conn_ext::CachingHeadersExt;

mod cache_control;
pub use cache_control::{cache_control, CacheControlDirective, CacheControlHeader};

/**
A combined handler that provides both [`Etag`] and [`Modified`]
behavior.
*/
#[derive(Debug, Clone, Copy, Default)]
pub struct CachingHeaders {
    inner: (Modified, Etag),
}
trillium::delegate_handler!(CachingHeaders => inner);

impl CachingHeaders {
    /// Constructs a new combination modified and etag handler
    pub fn new() -> Self {
        Self::default()
    }
}

/// alias for [`CachingHeaders::new`]
pub fn caching_headers() -> CachingHeaders {
    CachingHeaders::new()
}
