//! [`rkyv`] 0.8 support for the core HTTP types, gated behind the `rkyv_08` feature.
//!
//! Turning the feature on implements [`rkyv::Archive`], [`rkyv::Serialize`], and
//! [`rkyv::Deserialize`] for [`Version`], [`Method`], [`Status`], [`HeaderName`],
//! [`HeaderValue`], [`HeaderValues`], and [`Headers`], so they round-trip through
//! `rkyv::to_bytes` / `rkyv::from_bytes` directly with no wrapper types.
//!
//! The archived layout is defined here as an explicit, human-meaningful, version-stable
//! schema rather than mirroring the in-memory representation, because this format is meant
//! for durable persistence (e.g. a client cache written to disk). In particular header names
//! are stored as strings rather than the `KnownHeaderName` discriminant, whose `u8`
//! representation is explicitly documented as unstable across releases. Every field of the
//! archived form is a plain string, byte vector, or integer with a stable meaning; nothing
//! depends on a Rust-side discriminant that could shift between versions.
//!
//! The mirror types are `pub` only because they surface through the archived associated types
//! of the trait impls on the public HTTP types; the module itself is private, so they are not
//! part of the crate's public API. They exist only transiently inside a single serialize or
//! deserialize call, so the usual `Debug`/`Copy`/doc requirements don't earn their keep here.
#![allow(
    missing_docs,
    missing_debug_implementations,
    missing_copy_implementations
)]

use crate::{HeaderName, HeaderValue, HeaderValues, Headers, Method, Status, Version};
use rkyv::{
    Archive, Deserialize, Place, Serialize,
    rancor::{Fallible, Source},
};

/// Convert an archived mirror back into its public type, validating any semantic
/// constraints (a stored status code corresponds to a known [`Status`], etc.) that
/// rkyv's structural validation does not cover.
trait IntoPublic {
    type Public;
    fn into_public(self) -> Result<Self::Public, crate::Error>;
}

/// Implements the rkyv traits on `$public` by delegating to a private `$mirror` that derives
/// them. The public type never appears in the archived data; callers serialize and
/// deserialize the public type directly.
///
/// Requires `$mirror: From<&$public>` (always total — going from the real type to the mirror
/// cannot fail) and `$mirror: IntoPublic<Public = $public>` for the fallible reverse.
macro_rules! delegate_rkyv {
    ($public:ty => $mirror:ty) => {
        impl Archive for $public {
            type Archived = <$mirror as Archive>::Archived;
            type Resolver = <$mirror as Archive>::Resolver;

            fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
                Archive::resolve(&<$mirror>::from(self), resolver, out);
            }
        }

        impl<S: Fallible + ?Sized> Serialize<S> for $public
        where
            $mirror: Serialize<S>,
        {
            fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
                Serialize::serialize(&<$mirror>::from(self), serializer)
            }
        }

        impl<D: Fallible + ?Sized> Deserialize<$public, D> for <$mirror as Archive>::Archived
        where
            <$mirror as Archive>::Archived: Deserialize<$mirror, D>,
            D::Error: Source,
        {
            fn deserialize(&self, deserializer: &mut D) -> Result<$public, D::Error> {
                let mirror = Deserialize::<$mirror, D>::deserialize(self, deserializer)?;
                mirror.into_public().map_err(D::Error::new)
            }
        }
    };
}

// ---- Version -----------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct VersionMirror(u8);

impl From<&Version> for VersionMirror {
    fn from(version: &Version) -> Self {
        Self(match version {
            Version::Http0_9 => 9,
            Version::Http1_0 => 10,
            Version::Http1_1 => 11,
            Version::Http2 => 20,
            Version::Http3 => 30,
        })
    }
}

impl IntoPublic for VersionMirror {
    type Public = Version;

    fn into_public(self) -> Result<Version, crate::Error> {
        Ok(match self.0 {
            9 => Version::Http0_9,
            10 => Version::Http1_0,
            11 => Version::Http1_1,
            20 => Version::Http2,
            30 => Version::Http3,
            _ => return Err(crate::Error::InvalidVersion),
        })
    }
}

delegate_rkyv!(Version => VersionMirror);

// ---- Status ------------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct StatusMirror(u16);

impl From<&Status> for StatusMirror {
    fn from(status: &Status) -> Self {
        Self((*status).into())
    }
}

impl IntoPublic for StatusMirror {
    type Public = Status;

    fn into_public(self) -> Result<Status, crate::Error> {
        Status::try_from(self.0)
    }
}

delegate_rkyv!(Status => StatusMirror);

// ---- Method ------------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct MethodMirror(String);

impl From<&Method> for MethodMirror {
    fn from(method: &Method) -> Self {
        Self(method.as_str().to_string())
    }
}

impl IntoPublic for MethodMirror {
    type Public = Method;

    fn into_public(self) -> Result<Method, crate::Error> {
        self.0.parse()
    }
}

delegate_rkyv!(Method => MethodMirror);

// ---- HeaderName --------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct HeaderNameMirror(String);

impl From<&HeaderName<'_>> for HeaderNameMirror {
    fn from(name: &HeaderName<'_>) -> Self {
        Self(name.as_ref().to_string())
    }
}

impl IntoPublic for HeaderNameMirror {
    type Public = HeaderName<'static>;

    fn into_public(self) -> Result<HeaderName<'static>, crate::Error> {
        Ok(HeaderName::from(self.0))
    }
}

delegate_rkyv!(HeaderName<'static> => HeaderNameMirror);

// ---- HeaderValue -------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub enum HeaderValueRepr {
    Utf8(String),
    Bytes(Vec<u8>),
}

#[derive(Archive, Serialize, Deserialize)]
pub struct HeaderValueMirror {
    value: HeaderValueRepr,
    never_indexed: bool,
}

impl From<&HeaderValue> for HeaderValueMirror {
    fn from(value: &HeaderValue) -> Self {
        let repr = match value.as_str() {
            Some(utf8) => HeaderValueRepr::Utf8(utf8.to_string()),
            None => HeaderValueRepr::Bytes(value.as_ref().to_vec()),
        };
        Self {
            value: repr,
            never_indexed: value.is_never_indexed(),
        }
    }
}

impl IntoPublic for HeaderValueMirror {
    type Public = HeaderValue;

    fn into_public(self) -> Result<HeaderValue, crate::Error> {
        let mut value = match self.value {
            HeaderValueRepr::Utf8(utf8) => HeaderValue::from(utf8),
            HeaderValueRepr::Bytes(bytes) => HeaderValue::from(bytes),
        };
        value.set_never_indexed(self.never_indexed);
        Ok(value)
    }
}

delegate_rkyv!(HeaderValue => HeaderValueMirror);

// ---- HeaderValues ------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct HeaderValuesMirror(Vec<HeaderValueMirror>);

impl From<&HeaderValues> for HeaderValuesMirror {
    fn from(values: &HeaderValues) -> Self {
        Self(values.iter().map(HeaderValueMirror::from).collect())
    }
}

impl IntoPublic for HeaderValuesMirror {
    type Public = HeaderValues;

    fn into_public(self) -> Result<HeaderValues, crate::Error> {
        let values = self
            .0
            .into_iter()
            .map(IntoPublic::into_public)
            .collect::<Result<Vec<HeaderValue>, _>>()?;
        Ok(HeaderValues::from(values))
    }
}

delegate_rkyv!(HeaderValues => HeaderValuesMirror);

// ---- Headers -----------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize)]
pub struct HeadersMirror(Vec<(HeaderNameMirror, HeaderValuesMirror)>);

impl From<&Headers> for HeadersMirror {
    fn from(headers: &Headers) -> Self {
        Self(
            headers
                .iter()
                .map(|(name, values)| {
                    (
                        HeaderNameMirror::from(&name),
                        HeaderValuesMirror::from(values),
                    )
                })
                .collect(),
        )
    }
}

impl IntoPublic for HeadersMirror {
    type Public = Headers;

    fn into_public(self) -> Result<Headers, crate::Error> {
        let entries = self
            .0
            .into_iter()
            .map(|(name, values)| Ok((name.into_public()?, values.into_public()?)))
            .collect::<Result<Vec<(HeaderName<'static>, HeaderValues)>, crate::Error>>()?;
        Ok(Headers::from_iter(entries))
    }
}

delegate_rkyv!(Headers => HeadersMirror);

#[cfg(test)]
mod test {
    use crate::{HeaderValue, Headers, KnownHeaderName, Method, Status, Version};
    use rkyv::rancor::Error;

    macro_rules! assert_roundtrips {
        ($value:expr, $ty:ty) => {{
            let value: $ty = $value;
            let bytes = rkyv::to_bytes::<Error>(&value).unwrap();
            let restored = rkyv::from_bytes::<$ty, Error>(&bytes).unwrap();
            assert_eq!(restored, value);
        }};
    }

    #[test]
    fn version_roundtrips() {
        for version in [
            Version::Http0_9,
            Version::Http1_0,
            Version::Http1_1,
            Version::Http2,
            Version::Http3,
        ] {
            assert_roundtrips!(version, Version);
        }
    }

    #[test]
    fn status_roundtrips() {
        for status in [
            Status::Ok,
            Status::ImATeapot,
            Status::NotFound,
            Status::Continue,
        ] {
            assert_roundtrips!(status, Status);
        }
    }

    #[test]
    fn method_roundtrips() {
        for method in [
            Method::Get,
            Method::Post,
            Method::VersionControl,
            Method::MkCalendar,
        ] {
            assert_roundtrips!(method, Method);
        }
    }

    #[test]
    fn headers_roundtrip_all_variants() {
        let mut headers = Headers::new();
        // known header
        headers.insert(KnownHeaderName::ContentType, "text/plain");
        // unknown header with mixed casing
        headers.insert("X-Custom-Thing", "hello");
        // multiple values under one name
        headers.append(KnownHeaderName::SetCookie, "a=1");
        headers.append(KnownHeaderName::SetCookie, "b=2");
        // non-utf8 value + never-indexed bit
        let mut binary = HeaderValue::from(vec![0xff, 0xfe, 0x00, 0x01]);
        binary.set_never_indexed(true);
        headers.insert("X-Binary", binary);

        assert_roundtrips!(headers.clone(), Headers);

        // `HeaderValue`'s `PartialEq` ignores the never-indexed bit, so assert it survives
        // explicitly rather than relying on the equality check above.
        let bytes = rkyv::to_bytes::<Error>(&headers).unwrap();
        let restored = rkyv::from_bytes::<Headers, Error>(&bytes).unwrap();
        assert!(restored.get("X-Binary").unwrap().is_never_indexed());
    }
}
