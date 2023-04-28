// originally from https://github.com/http-rs/http-types/blob/main/src/method.rs
use std::{
    fmt::{self, Display},
    str::FromStr,
};

/// HTTP request methods.
///
/// See also [Mozilla's documentation][Mozilla docs], the [RFC7231, Section 4][] and
/// [IANA's Hypertext Transfer Protocol (HTTP) Method Registry][HTTP Method Registry].
///
/// [Mozilla docs]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods
/// [RFC7231, Section 4]: https://tools.ietf.org/html/rfc7231#section-4
/// [HTTP Method Registry]: https://www.iana.org/assignments/http-methods/http-methods.xhtml
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Method {
    /// The ACL method modifies the access control list (which can be read via the DAV:acl
    /// property) of a resource.
    ///
    /// See [RFC3744, Section 8.1][].
    ///
    /// [RFC3744, Section 8.1]: https://tools.ietf.org/html/rfc3744#section-8.1
    Acl,

    /// A collection can be placed under baseline control with a BASELINE-CONTROL request.
    ///
    /// See [RFC3253, Section 12.6][].
    ///
    /// [RFC3253, Section 12.6]: https://tools.ietf.org/html/rfc3253#section-12.6
    BaselineControl,

    /// The BIND method modifies the collection identified by the Request- URI, by adding a new
    /// binding from the segment specified in the BIND body to the resource identified in the BIND
    /// body.
    ///
    /// See [RFC5842, Section 4][].
    ///
    /// [RFC5842, Section 4]: https://tools.ietf.org/html/rfc5842#section-4
    Bind,

    /// A CHECKIN request can be applied to a checked-out version-controlled resource to produce a
    /// new version whose content and dead properties are copied from the checked-out resource.
    ///
    /// See [RFC3253, Section 4.4][] and [RFC3253, Section 9.4][].
    ///
    /// [RFC3253, Section 4.4]: https://tools.ietf.org/html/rfc3253#section-4.4
    /// [RFC3253, Section 9.4]: https://tools.ietf.org/html/rfc3253#section-9.4
    Checkin,

    /// A CHECKOUT request can be applied to a checked-in version-controlled resource to allow
    /// modifications to the content and dead properties of that version-controlled resource.
    ///
    /// See [RFC3253, Section 4.3][] and [RFC3253, Section 8.8][].
    ///
    /// [RFC3253, Section 4.3]: https://tools.ietf.org/html/rfc3253#section-4.3
    /// [RFC3253, Section 8.8]: https://tools.ietf.org/html/rfc3253#section-8.8
    Checkout,

    /// The CONNECT method requests that the recipient establish a tunnel to the destination origin
    /// server identified by the request-target and, if successful, thereafter restrict its
    /// behavior to blind forwarding of packets, in both directions, until the tunnel is closed.
    ///
    /// See [RFC7231, Section 4.3.6][].
    ///
    /// [RFC7231, Section 4.3.6]: https://tools.ietf.org/html/rfc7231#section-4.3.6
    Connect,

    /// The COPY method creates a duplicate of the source resource identified by the Request-URI,
    /// in the destination resource identified by the URI in the Destination header.
    ///
    /// See [RFC4918, Section 9.8][].
    ///
    /// [RFC4918, Section 9.8]: https://tools.ietf.org/html/rfc4918#section-9.8
    Copy,

    /// The DELETE method requests that the origin server remove the association between the target
    /// resource and its current functionality.
    ///
    /// See [RFC7231, Section 4.3.5][].
    ///
    /// [RFC7231, Section 4.3.5]: https://tools.ietf.org/html/rfc7231#section-4.3.5
    Delete,

    /// The GET method requests transfer of a current selected representation for the target
    /// resource.
    ///
    /// See [RFC7231, Section 4.3.1][].
    ///
    /// [RFC7231, Section 4.3.1]: https://tools.ietf.org/html/rfc7231#section-4.3.1
    Get,

    /// The HEAD method is identical to GET except that the server MUST NOT send a message body in
    /// the response.
    ///
    /// See [RFC7231, Section 4.3.2][].
    ///
    /// [RFC7231, Section 4.3.2]: https://tools.ietf.org/html/rfc7231#section-4.3.2
    Head,

    /// A LABEL request can be applied to a version to modify the labels that select that version.
    ///
    /// See [RFC3253, Section 8.2][].
    ///
    /// [RFC3253, Section 8.2]: https://tools.ietf.org/html/rfc3253#section-8.2
    Label,

    /// The LINK method establishes one or more Link relationships between the existing resource
    /// identified by the Request-URI and other existing resources.
    ///
    /// See [RFC2068, Section 19.6.1.2][].
    ///
    /// [RFC2068, Section 19.6.1.2]: https://tools.ietf.org/html/rfc2068#section-19.6.1.2
    Link,

    /// The LOCK method is used to take out a lock of any access type and to refresh an existing
    /// lock.
    ///
    /// See [RFC4918, Section 9.10][].
    ///
    /// [RFC4918, Section 9.10]: https://tools.ietf.org/html/rfc4918#section-9.10
    Lock,

    /// The MERGE method performs the logical merge of a specified version (the "merge source")
    /// into a specified version-controlled resource (the "merge target").
    ///
    /// See [RFC3253, Section 11.2][].
    ///
    /// [RFC3253, Section 11.2]: https://tools.ietf.org/html/rfc3253#section-11.2
    Merge,

    /// A MKACTIVITY request creates a new activity resource.
    ///
    /// See [RFC3253, Section 13.5].
    ///
    /// [RFC3253, Section 13.5]: https://tools.ietf.org/html/rfc3253#section-13.5
    MkActivity,

    /// An HTTP request using the MKCALENDAR method creates a new calendar collection resource.
    ///
    /// See [RFC4791, Section 5.3.1][] and [RFC8144, Section 2.3][].
    ///
    /// [RFC4791, Section 5.3.1]: https://tools.ietf.org/html/rfc4791#section-5.3.1
    /// [RFC8144, Section 2.3]: https://tools.ietf.org/html/rfc8144#section-2.3
    MkCalendar,

    /// MKCOL creates a new collection resource at the location specified by the Request-URI.
    ///
    /// See [RFC4918, Section 9.3][], [RFC5689, Section 3][] and [RFC8144, Section 2.3][].
    ///
    /// [RFC4918, Section 9.3]: https://tools.ietf.org/html/rfc4918#section-9.3
    /// [RFC5689, Section 3]: https://tools.ietf.org/html/rfc5689#section-3
    /// [RFC8144, Section 2.3]: https://tools.ietf.org/html/rfc5689#section-3
    MkCol,

    /// The MKREDIRECTREF method requests the creation of a redirect reference resource.
    ///
    /// See [RFC4437, Section 6][].
    ///
    /// [RFC4437, Section 6]: https://tools.ietf.org/html/rfc4437#section-6
    MkRedirectRef,

    /// A MKWORKSPACE request creates a new workspace resource.
    ///
    /// See [RFC3253, Section 6.3][].
    ///
    /// [RFC3253, Section 6.3]: https://tools.ietf.org/html/rfc3253#section-6.3
    MkWorkspace,

    /// The MOVE operation on a non-collection resource is the logical equivalent of a copy (COPY),
    /// followed by consistency maintenance processing, followed by a delete of the source, where
    /// all three actions are performed in a single operation.
    ///
    /// See [RFC4918, Section 9.9][].
    ///
    /// [RFC4918, Section 9.9]: https://tools.ietf.org/html/rfc4918#section-9.9
    Move,

    /// The OPTIONS method requests information about the communication options available for the
    /// target resource, at either the origin server or an intervening intermediary.
    ///
    /// See [RFC7231, Section 4.3.7][].
    ///
    /// [RFC7231, Section 4.3.7]: https://tools.ietf.org/html/rfc7231#section-4.3.7
    Options,

    /// The ORDERPATCH method is used to change the ordering semantics of a collection, to change
    /// the order of the collection's members in the ordering, or both.
    ///
    /// See [RFC3648, Section 7][].
    ///
    /// [RFC3648, Section 7]: https://tools.ietf.org/html/rfc3648#section-7
    OrderPatch,

    /// The PATCH method requests that a set of changes described in the request entity be applied
    /// to the resource identified by the Request- URI.
    ///
    /// See [RFC5789, Section 2][].
    ///
    /// [RFC5789, Section 2]: https://tools.ietf.org/html/rfc5789#section-2
    Patch,

    /// The POST method requests that the target resource process the representation enclosed in
    /// the request according to the resource's own specific semantics.
    ///
    /// For example, POST is used for the following functions (among others):
    ///
    ///   - Providing a block of data, such as the fields entered into an HTML form, to a
    ///     data-handling process;
    ///   - Posting a message to a bulletin board, newsgroup, mailing list, blog, or similar group
    ///     of articles;
    ///   - Creating a new resource that has yet to be identified by the origin server; and
    ///   - Appending data to a resource's existing representation(s).
    ///
    /// See [RFC7231, Section 4.3.3][].
    ///
    /// [RFC7231, Section 4.3.3]: https://tools.ietf.org/html/rfc7231#section-4.3.3
    Post,

    /// This method is never used by an actual client. This method will appear to be used when an
    /// HTTP/1.1 server or intermediary attempts to parse an HTTP/2 connection preface.
    ///
    /// See [RFC7540, Section 3.5][] and [RFC7540, Section 11.6][]
    ///
    /// [RFC7540, Section 3.5]: https://tools.ietf.org/html/rfc7540#section-3.5
    /// [RFC7540, Section 11.6]: https://tools.ietf.org/html/rfc7540#section-11.6
    Pri,

    /// The PROPFIND method retrieves properties defined on the resource identified by the
    /// Request-URI.
    ///
    /// See [RFC4918, Section 9.1][] and [RFC8144, Section 2.1][].
    ///
    /// [RFC4918, Section 9.1]: https://tools.ietf.org/html/rfc4918#section-9.1
    /// [RFC8144, Section 2.1]: https://tools.ietf.org/html/rfc8144#section-2.1
    PropFind,

    /// The PROPPATCH method processes instructions specified in the request body to set and/or
    /// remove properties defined on the resource identified by the Request-URI.
    ///
    /// See [RFC4918, Section 9.2][] and [RFC8144, Section 2.2][].
    ///
    /// [RFC4918, Section 9.2]: https://tools.ietf.org/html/rfc4918#section-9.2
    /// [RFC8144, Section 2.2]: https://tools.ietf.org/html/rfc8144#section-2.2
    PropPatch,

    /// The PUT method requests that the state of the target resource be created or replaced with
    /// the state defined by the representation enclosed in the request message payload.
    ///
    /// See [RFC7231, Section 4.3.4][].
    ///
    /// [RFC7231, Section 4.3.4]: https://tools.ietf.org/html/rfc7231#section-4.3.4
    Put,

    /// The REBIND method removes a binding to a resource from a collection, and adds a binding to
    /// that resource into the collection identified by the Request-URI.
    ///
    /// See [RFC5842, Section 6][].
    ///
    /// [RFC5842, Section 6]: https://tools.ietf.org/html/rfc5842#section-6
    Rebind,

    /// A REPORT request is an extensible mechanism for obtaining information about a resource.
    ///
    /// See [RFC3253, Section 3.6][] and [RFC8144, Section 2.1][].
    ///
    /// [RFC3253, Section 3.6]: https://tools.ietf.org/html/rfc3253#section-3.6
    /// [RFC8144, Section 2.1]: https://tools.ietf.org/html/rfc8144#section-2.1
    Report,

    /// The client invokes the SEARCH method to initiate a server-side search. The body of the
    /// request defines the query.
    ///
    /// See [RFC5323, Section 2][].
    ///
    /// [RFC5323, Section 2]: https://tools.ietf.org/html/rfc5323#section-2
    Search,

    /// The TRACE method requests a remote, application-level loop-back of the request message.
    ///
    /// See [RFC7231, Section 4.3.8][].
    ///
    /// [RFC7231, Section 4.3.8]: https://tools.ietf.org/html/rfc7231#section-4.3.8
    Trace,

    /// The UNBIND method modifies the collection identified by the Request- URI by removing the
    /// binding identified by the segment specified in the UNBIND body.
    ///
    /// See [RFC5842, Section 5][].
    ///
    /// [RFC5842, Section 5]: https://tools.ietf.org/html/rfc5842#section-5
    Unbind,

    /// An UNCHECKOUT request can be applied to a checked-out version-controlled resource to cancel
    /// the CHECKOUT and restore the pre-CHECKOUT state of the version-controlled resource.
    ///
    /// See [RFC3253, Section 4.5][].
    ///
    /// [RFC3253, Section 4.5]: https://tools.ietf.org/html/rfc3253#section-4.5
    Uncheckout,

    /// The UNLINK method removes one or more Link relationships from the existing resource
    /// identified by the Request-URI.
    ///
    /// See [RFC2068, Section 19.6.1.3][].
    ///
    /// [RFC2068, Section 19.6.1.3]: https://tools.ietf.org/html/rfc2068#section-19.6.1.3
    Unlink,

    /// The UNLOCK method removes the lock identified by the lock token in the Lock-Token request
    /// header.
    ///
    /// See [RFC4918, Section 9.11][].
    ///
    /// [RFC4918, Section 9.11]: https://tools.ietf.org/html/rfc4918#section-9.11
    Unlock,

    /// The UPDATE method modifies the content and dead properties of a checked-in
    /// version-controlled resource (the "update target") to be those of a specified version (the
    /// "update source") from the version history of that version-controlled resource.
    ///
    /// See [RFC3253, Section 7.1][].
    ///
    /// [RFC3253, Section 7.1]: https://tools.ietf.org/html/rfc3253#section-7.1
    Update,

    /// The UPDATEREDIRECTREF method requests the update of a redirect reference resource.
    ///
    /// See [RFC4437, Section 7][].
    ///
    /// [RFC4437, Section 7]: https://tools.ietf.org/html/rfc4437#section-7
    UpdateRedirectRef,

    /// A VERSION-CONTROL request can be used to create a version-controlled resource at the
    /// request-URL.
    ///
    /// See [RFC3253, Section 3.5].
    ///
    /// [RFC3253, Section 3.5]: https://tools.ietf.org/html/rfc3253#section-3.5
    VersionControl,
}

impl Method {
    /// Predicate that returns whether the method is "safe."
    ///
    /// > Request methods are considered "safe" if their defined semantics are
    /// > essentially read-only; i.e., the client does not request, and does
    /// > not expect, any state change on the origin server as a result of
    /// > applying a safe method to a target resource.
    ///
    /// -- [rfc7231§4.2.1](https://tools.ietf.org/html/rfc7231#section-4.2.1)
    pub const fn is_safe(&self) -> bool {
        matches!(
            self,
            Method::Get
                | Method::Head
                | Method::Options
                | Method::Pri
                | Method::PropFind
                | Method::Report
                | Method::Search
                | Method::Trace
        )
    }

    /// predicate that returns whether this method is considered "idempotent".
    ///
    /// > A request method is considered "idempotent" if the intended effect on
    /// > the server of multiple identical requests with that method is the
    /// > same as the effect for a single such request.
    ///
    /// -- [rfc7231§4.2.2](https://tools.ietf.org/html/rfc7231#section-4.2.2)
    pub const fn is_idempotent(&self) -> bool {
        self.is_safe() || matches!(self, Method::Put | Method::Delete)
    }

    /// returns the static str representation of this method
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Acl => "ACL",
            Self::BaselineControl => "BASELINE-CONTROL",
            Self::Bind => "BIND",
            Self::Checkin => "CHECKIN",
            Self::Checkout => "CHECKOUT",
            Self::Connect => "CONNECT",
            Self::Copy => "COPY",
            Self::Delete => "DELETE",
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Label => "LABEL",
            Self::Link => "LINK",
            Self::Lock => "LOCK",
            Self::Merge => "MERGE",
            Self::MkActivity => "MKACTIVITY",
            Self::MkCalendar => "MKCALENDAR",
            Self::MkCol => "MKCOL",
            Self::MkRedirectRef => "MKREDIRECTREF",
            Self::MkWorkspace => "MKWORKSPACE",
            Self::Move => "MOVE",
            Self::Options => "OPTIONS",
            Self::OrderPatch => "ORDERPATCH",
            Self::Patch => "PATCH",
            Self::Post => "POST",
            Self::Pri => "PRI",
            Self::PropFind => "PROPFIND",
            Self::PropPatch => "PROPPATCH",
            Self::Put => "PUT",
            Self::Rebind => "REBIND",
            Self::Report => "REPORT",
            Self::Search => "SEARCH",
            Self::Trace => "TRACE",
            Self::Unbind => "UNBIND",
            Self::Uncheckout => "UNCHECKOUT",
            Self::Unlink => "UNLINK",
            Self::Unlock => "UNLOCK",
            Self::Update => "UPDATE",
            Self::UpdateRedirectRef => "UPDATEREDIRECTREF",
            Self::VersionControl => "VERSION-CONTROL",
        }
    }
}

impl Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl FromStr for Method {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &*s.to_ascii_uppercase() {
            "ACL" => Ok(Self::Acl),
            "BASELINE-CONTROL" => Ok(Self::BaselineControl),
            "BIND" => Ok(Self::Bind),
            "CHECKIN" => Ok(Self::Checkin),
            "CHECKOUT" => Ok(Self::Checkout),
            "CONNECT" => Ok(Self::Connect),
            "COPY" => Ok(Self::Copy),
            "DELETE" => Ok(Self::Delete),
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            "LABEL" => Ok(Self::Label),
            "LINK" => Ok(Self::Link),
            "LOCK" => Ok(Self::Lock),
            "MERGE" => Ok(Self::Merge),
            "MKACTIVITY" => Ok(Self::MkActivity),
            "MKCALENDAR" => Ok(Self::MkCalendar),
            "MKCOL" => Ok(Self::MkCol),
            "MKREDIRECTREF" => Ok(Self::MkRedirectRef),
            "MKWORKSPACE" => Ok(Self::MkWorkspace),
            "MOVE" => Ok(Self::Move),
            "OPTIONS" => Ok(Self::Options),
            "ORDERPATCH" => Ok(Self::OrderPatch),
            "PATCH" => Ok(Self::Patch),
            "POST" => Ok(Self::Post),
            "PRI" => Ok(Self::Pri),
            "PROPFIND" => Ok(Self::PropFind),
            "PROPPATCH" => Ok(Self::PropPatch),
            "PUT" => Ok(Self::Put),
            "REBIND" => Ok(Self::Rebind),
            "REPORT" => Ok(Self::Report),
            "SEARCH" => Ok(Self::Search),
            "TRACE" => Ok(Self::Trace),
            "UNBIND" => Ok(Self::Unbind),
            "UNCHECKOUT" => Ok(Self::Uncheckout),
            "UNLINK" => Ok(Self::Unlink),
            "UNLOCK" => Ok(Self::Unlock),
            "UPDATE" => Ok(Self::Update),
            "UPDATEREDIRECTREF" => Ok(Self::UpdateRedirectRef),
            "VERSION-CONTROL" => Ok(Self::VersionControl),
            _ => Err(crate::Error::UnrecognizedMethod(
                "Invalid HTTP method".into(),
            )),
        }
    }
}

impl<'a> TryFrom<&'a str> for Method {
    type Error = crate::Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl AsRef<str> for Method {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod test {
    use super::Method;
    use std::collections::HashSet;

    #[test]
    fn names() -> Result<(), crate::Error> {
        let method_names = [
            "ACL",
            "BASELINE-CONTROL",
            "BIND",
            "CHECKIN",
            "CHECKOUT",
            "CONNECT",
            "COPY",
            "DELETE",
            "GET",
            "HEAD",
            "LABEL",
            "LINK",
            "LOCK",
            "MERGE",
            "MKACTIVITY",
            "MKCALENDAR",
            "MKCOL",
            "MKREDIRECTREF",
            "MKWORKSPACE",
            "MOVE",
            "OPTIONS",
            "ORDERPATCH",
            "PATCH",
            "POST",
            "PRI",
            "PROPFIND",
            "PROPPATCH",
            "PUT",
            "REBIND",
            "REPORT",
            "SEARCH",
            "TRACE",
            "UNBIND",
            "UNCHECKOUT",
            "UNLINK",
            "UNLOCK",
            "UPDATE",
            "UPDATEREDIRECTREF",
            "VERSION-CONTROL",
        ];

        let methods = method_names
            .iter()
            .map(|s| s.parse::<Method>())
            .collect::<Result<HashSet<_>, _>>()?;

        // check that we didn't accidentally map two methods to the same variant
        assert_eq!(methods.len(), method_names.len());

        // check that a method's name and the name it is parsed from match
        for method in methods {
            assert_eq!(method.as_ref().parse::<Method>()?, method);
        }

        Ok(())
    }
}
