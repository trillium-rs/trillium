/*!
Trillium handler for redirection
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

use std::borrow::Cow;
use trillium::{Conn, Handler, KnownHeaderName::Location, Status};

/// The subset of http statuses that indicate redirection
///
/// The default is [`RedirectStatus::Found`]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum RedirectStatus {
    /// [300 Multiple Choices](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/300)
    MultipleChoices,
    /// [301 Moved Permanently](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/301)
    MovedPermanently,
    /// [302 Found](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/302)
    #[default]
    Found,
    /// [303 See Other](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/303)
    SeeOther,
    /// [307 Temporary Redirect](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/307)
    TemporaryRedirect,
    /// [308 Permanent Redirect](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/308)
    PermanentRedirect,
}

impl From<RedirectStatus> for Status {
    fn from(value: RedirectStatus) -> Self {
        match value {
            RedirectStatus::MultipleChoices => Status::MultipleChoice,
            RedirectStatus::MovedPermanently => Status::MovedPermanently,
            RedirectStatus::Found => Status::Found,
            RedirectStatus::SeeOther => Status::SeeOther,
            RedirectStatus::TemporaryRedirect => Status::TemporaryRedirect,
            RedirectStatus::PermanentRedirect => Status::PermanentRedirect,
        }
    }
}

/// A simple handler for redirection
#[derive(Clone, Debug)]
pub struct Redirect {
    to: Cow<'static, str>,
    status: RedirectStatus,
}

impl Redirect {
    /// Redirect to the provided path or url with the default redirect status
    pub fn to(to: impl Into<Cow<'static, str>>) -> Self {
        Self {
            to: to.into(),
            status: RedirectStatus::default(),
        }
    }

    /// Provide a [`RedirectStatus`] for this redirect handler
    pub fn with_redirect_status(mut self, status: RedirectStatus) -> Self {
        self.status = status;
        self
    }
}

/// Redirect to the provided path or url with the default redirect status
pub fn redirect(to: impl Into<Cow<'static, str>>) -> Redirect {
    Redirect::to(to)
}

#[trillium::async_trait]
impl Handler for Redirect {
    async fn run(&self, conn: Conn) -> Conn {
        conn.redirect_as(self.to.clone(), self.status)
    }
}

/// An extension trait for [`trillium::Conn`] for redirection
pub trait RedirectConnExt {
    /// redirect this conn with the default redirect status
    fn redirect(self, to: impl Into<Cow<'static, str>>) -> Self;
    /// redirect this conn with the provided redirect status
    fn redirect_as(self, to: impl Into<Cow<'static, str>>, status: RedirectStatus) -> Self;
}

impl RedirectConnExt for Conn {
    fn redirect(self, to: impl Into<Cow<'static, str>>) -> Self {
        self.redirect_as(to, RedirectStatus::default())
    }

    fn redirect_as(self, to: impl Into<Cow<'static, str>>, status: RedirectStatus) -> Self {
        self.with_status(status)
            .with_response_header(Location, to.into())
            .halt()
    }
}
