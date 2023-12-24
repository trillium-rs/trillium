//! # Conversion between [`http`] and `trillium-http` types

pub use headers::HeaderConversionError; // for semver

mod version {
    use http1 as http;
    impl TryFrom<http::Version> for crate::Version {
        type Error = String;
        fn try_from(version: http::Version) -> Result<Self, Self::Error> {
            match version {
                http::Version::HTTP_09 => Ok(crate::Version::Http0_9),
                http::Version::HTTP_10 => Ok(crate::Version::Http1_0),
                http::Version::HTTP_11 => Ok(crate::Version::Http1_1),
                http::Version::HTTP_2 => Ok(crate::Version::Http2_0),
                http::Version::HTTP_3 => Ok(crate::Version::Http3_0),
                other => Err(format!("unsupported version {other:?}")),
            }
        }
    }

    impl From<crate::Version> for http::Version {
        fn from(version: crate::Version) -> Self {
            match version {
                crate::Version::Http0_9 => http::Version::HTTP_09,
                crate::Version::Http1_0 => http::Version::HTTP_10,
                crate::Version::Http1_1 => http::Version::HTTP_11,
                crate::Version::Http2_0 => http::Version::HTTP_2,
                crate::Version::Http3_0 => http::Version::HTTP_3,
            }
        }
    }

    impl PartialEq<crate::Version> for http::Version {
        fn eq(&self, other: &crate::Version) -> bool {
            match TryInto::<crate::Version>::try_into(*self) {
                Ok(v) => v.eq(other),
                Err(_) => false,
            }
        }
    }

    impl PartialEq<http::Version> for crate::Version {
        fn eq(&self, other: &http::Version) -> bool {
            Into::<http::Version>::into(*self).eq(other)
        }
    }
}

mod method {
    use http1 as http;

    use std::str::FromStr;

    impl TryFrom<http::Method> for crate::Method {
        type Error = <crate::Method as FromStr>::Err;
        fn try_from(http_method: http::Method) -> Result<Self, Self::Error> {
            http_method.as_str().parse()
        }
    }

    impl TryFrom<&http::Method> for crate::Method {
        type Error = <crate::Method as FromStr>::Err;
        fn try_from(http_method: &http::Method) -> Result<Self, Self::Error> {
            http_method.as_str().parse()
        }
    }

    impl TryFrom<crate::Method> for http::Method {
        type Error = <http::Method as FromStr>::Err;

        fn try_from(trillium_method: crate::Method) -> Result<Self, Self::Error> {
            trillium_method.as_ref().parse()
        }
    }

    impl PartialEq<crate::Method> for http::Method {
        fn eq(&self, other: &crate::Method) -> bool {
            TryInto::<crate::Method>::try_into(self).map_or(false, |m| m.eq(other))
        }
    }

    impl PartialEq<http::Method> for crate::Method {
        fn eq(&self, other: &http::Method) -> bool {
            TryInto::<http::Method>::try_into(*self).map_or(false, |m| m.eq(other))
        }
    }
}

mod status {
    use http1 as http;

    impl TryFrom<http::StatusCode> for crate::Status {
        type Error = <crate::Status as TryFrom<u16>>::Error;

        fn try_from(http_status_code: http::StatusCode) -> Result<Self, Self::Error> {
            http_status_code.as_u16().try_into()
        }
    }

    impl TryFrom<crate::Status> for http::StatusCode {
        type Error = http::status::InvalidStatusCode;

        fn try_from(trillium_status: crate::Status) -> Result<Self, Self::Error> {
            http::StatusCode::from_u16(trillium_status as u16)
        }
    }

    impl PartialEq<crate::Status> for http::StatusCode {
        fn eq(&self, other: &crate::Status) -> bool {
            self.as_u16() == (*other as u16)
        }
    }

    impl PartialEq<http::StatusCode> for crate::Status {
        fn eq(&self, other: &http::StatusCode) -> bool {
            (*self as u16) == other.as_u16()
        }
    }
}

mod header_name {
    use http1 as http;

    impl From<http::HeaderName> for crate::HeaderName<'static> {
        fn from(http_header_name: http::header::HeaderName) -> Self {
            http_header_name.as_str().to_owned().into()
        }
    }

    impl TryFrom<crate::HeaderName<'_>> for http::HeaderName {
        type Error = http::header::InvalidHeaderName;

        fn try_from(trillium_header_name: crate::HeaderName) -> Result<Self, Self::Error> {
            http::header::HeaderName::from_bytes(trillium_header_name.as_ref().as_bytes())
        }
    }

    impl PartialEq<http::HeaderName> for crate::HeaderName<'_> {
        fn eq(&self, other: &http::HeaderName) -> bool {
            AsRef::<str>::as_ref(self) == AsRef::<str>::as_ref(other)
        }
    }

    impl PartialEq<crate::HeaderName<'_>> for http::HeaderName {
        fn eq(&self, other: &crate::HeaderName<'_>) -> bool {
            AsRef::<str>::as_ref(other) == AsRef::<str>::as_ref(self)
        }
    }
}

mod headers {
    use http1 as http;

    use thiserror::Error;

    impl From<http::HeaderMap> for crate::Headers {
        fn from(http_header_map: http::HeaderMap) -> Self {
            let mut trillium_headers = crate::Headers::default();
            let mut current_header_name = None;
            for (http_header_name, http_header_value) in http_header_map {
                current_header_name = http_header_name.or(current_header_name);

                if let Some(http_header_name) = current_header_name.as_ref() {
                    trillium_headers.append(
                        http_header_name.clone(),
                        crate::HeaderValue::from(http_header_value),
                    );
                }
            }

            trillium_headers
        }
    }

    /// An error enum that represents failures to convert [`Headers`] into
    /// a [`http::HeaderMap`]
    #[derive(Debug, Error)]
    pub enum HeaderConversionError {
        /// A header that was valid in trillium was not valid as a
        /// [`http::header::HeaderName`].
        ///
        /// Please consider filing an issue with trillium, as there are
        /// not currently known examples of this.
        #[error(transparent)]
        InvalidHeaderName(#[from] http::header::InvalidHeaderName),

        /// A header that was valid in trillium was not valid as a
        /// [`http::header::HeaderValue`].
        ///
        /// Please consider filing an issue with trillium, as there are
        /// not currently known examples of this.
        #[error(transparent)]
        InvalidHeaderValue(#[from] http::header::InvalidHeaderValue),
    }

    impl TryFrom<crate::Headers> for http::HeaderMap {
        type Error = HeaderConversionError;
        fn try_from(trillium_headers: crate::Headers) -> Result<Self, Self::Error> {
            let mut http_header_map = http::HeaderMap::default();
            for (trillium_header_name, trillium_header_values) in trillium_headers {
                let http_header_name = http::header::HeaderName::try_from(trillium_header_name)?;
                for trillium_header_value in trillium_header_values {
                    let http_header_value =
                        http::header::HeaderValue::try_from(trillium_header_value)?;
                    http_header_map.append(http_header_name.clone(), http_header_value);
                }
            }
            Ok(http_header_map)
        }
    }
}

mod header_values {
    use http1 as http;

    impl From<http::HeaderValue> for crate::HeaderValue {
        fn from(http_header_value: http::HeaderValue) -> Self {
            http_header_value.as_bytes().to_owned().into()
        }
    }

    impl TryFrom<crate::HeaderValue> for http::HeaderValue {
        type Error = http::header::InvalidHeaderValue;

        fn try_from(trillium_header_value: crate::HeaderValue) -> Result<Self, Self::Error> {
            http::HeaderValue::from_bytes(trillium_header_value.as_ref())
        }
    }

    impl PartialEq<crate::HeaderValue> for http::HeaderValue {
        fn eq(&self, other: &crate::HeaderValue) -> bool {
            other.as_ref() == self.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use http1 as http;

    #[test]
    fn versions() {
        assert_eq!(
            http::Version::from(crate::Version::Http2_0),
            http::Version::HTTP_2
        );
        assert_eq!(
            crate::Version::try_from(http::Version::HTTP_09).unwrap(),
            crate::Version::Http0_9
        );

        assert_eq!(crate::Version::Http1_1, http::Version::HTTP_11);
        assert_eq!(http::Version::HTTP_3, crate::Version::Http3_0);
    }

    #[test]
    fn method_round_trip() {
        assert_eq!(
            http::Method::try_from(crate::Method::Delete).unwrap(),
            http::Method::DELETE
        );
        assert_eq!(
            crate::Method::try_from(http::Method::DELETE).unwrap(),
            crate::Method::Delete
        );
        assert_eq!(
            http::Method::try_from(crate::Method::BaselineControl).unwrap(),
            "BASELINE-CONTROL"
        );

        assert_eq!(crate::Method::Post, http::Method::POST);
        assert_eq!(http::Method::PATCH, crate::Method::Patch);
    }

    #[test]
    fn status_round_trip() {
        assert_eq!(
            http::StatusCode::try_from(crate::Status::Ok).unwrap(),
            http::StatusCode::OK
        );
        assert_eq!(
            crate::Status::try_from(http::StatusCode::OK).unwrap(),
            crate::Status::Ok
        );

        assert_eq!(
            http::StatusCode::try_from(crate::Status::ImATeapot).unwrap(),
            418
        );

        assert_eq!(crate::Status::Ok, http::StatusCode::OK);
        assert_eq!(http::StatusCode::CONFLICT, crate::Status::Conflict);
    }

    #[test]
    fn headers_round_trip() {
        let trillium_headers: crate::Headers = [
            (crate::KnownHeaderName::Host, "foo.bar".to_string()),
            (crate::KnownHeaderName::Cookie, "cookie 1".to_string()),
            (crate::KnownHeaderName::Cookie, "cookie 2".to_string()),
        ]
        .into_iter()
        .collect();

        let http_header_map: http::HeaderMap = trillium_headers.clone().try_into().unwrap();
        assert_eq!(http_header_map.get(http::header::HOST).unwrap(), "foo.bar");
        assert_eq!(
            http_header_map
                .get_all(http::header::COOKIE)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["cookie 1", "cookie 2"]
        );

        let new_trillium_headers = crate::Headers::from(http_header_map);
        assert_eq!(&trillium_headers, &new_trillium_headers);
        assert_eq!(
            new_trillium_headers
                .get(crate::KnownHeaderName::Host)
                .unwrap(),
            "foo.bar"
        );
        assert_eq!(
            new_trillium_headers
                .get_values(crate::KnownHeaderName::Cookie)
                .unwrap(),
            &crate::HeaderValues::from(vec!["cookie 1", "cookie 2"])
        );
    }
}
