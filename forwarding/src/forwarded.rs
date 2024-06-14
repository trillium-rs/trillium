use crate::parse_utils::{parse_quoted_string, parse_token};
use std::{borrow::Cow, fmt::Write, net::IpAddr};
use trillium::{
    Headers,
    KnownHeaderName::{
        Forwarded as ForwardedHeader, XforwardedBy, XforwardedFor, XforwardedHost, XforwardedProto,
        XforwardedSsl,
    },
};

/// A rust representation of the [forwarded
/// header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forwarded<'a> {
    by: Option<Cow<'a, str>>,
    forwarded_for: Vec<Cow<'a, str>>,
    host: Option<Cow<'a, str>>,
    proto: Option<Cow<'a, str>>,
}

impl<'a> Forwarded<'a> {
    /// Attempts to parse a Forwarded from headers (or a request or
    /// response). Builds a borrowed Forwarded by default. To build an
    /// owned Forwarded, use
    /// `Forwarded::from_headers(...).into_owned()`
    ///
    /// # X-Forwarded-For, -By, and -Proto compatability
    ///
    /// This implementation includes fall-back support for the
    /// historical unstandardized headers x-forwarded-for,
    /// x-forwarded-by, and x-forwarded-proto. If you do not wish to
    /// support these headers, use
    /// [`Forwarded::from_forwarded_header`]. To _only_ support these
    /// historical headers and _not_ the standardized Forwarded
    /// header, use [`Forwarded::from_x_headers`].
    ///
    /// Please note that either way, this implementation will
    /// normalize to the standardized Forwarded header, as recommended
    /// in
    /// [rfc7239§7.4](https://tools.ietf.org/html/rfc7239#section-7.4)
    ///
    /// # Examples
    /// ```rust
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # use trillium::Headers;
    /// use trillium_forwarding::Forwarded;
    ///
    /// let mut headers = Headers::new();
    /// headers.insert(
    ///     "Forwarded",
    ///     r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown;proto=https"#,
    /// );
    /// let forwarded = Forwarded::from_headers(&headers)?.unwrap();
    /// assert_eq!(forwarded.proto(), Some("https"));
    /// assert_eq!(
    ///     forwarded.forwarded_for(),
    ///     vec!["192.0.2.43", "[2001:db8:cafe::17]", "unknown"]
    /// );
    /// # Ok(()) }
    /// ```
    ///
    /// ```rust

    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {

    /// let mut headers = Headers::new();
    /// headers.insert("X-Forwarded-For", "192.0.2.43, 2001:db8:cafe::17, unknown");
    /// headers.insert("X-Forwarded-Proto", "https");
    /// let forwarded = Forwarded::from_headers(&headers)?.unwrap();
    /// assert_eq!(forwarded.forwarded_for(), vec!["192.0.2.43", "[2001:db8:cafe::17]", "unknown"]);
    /// assert_eq!(forwarded.proto(), Some("https"));
    /// assert_eq!(
    ///     forwarded.to_string(),
    ///     r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown;proto=https"#
    /// );
    /// # Ok(()) }
    /// ```

    pub fn from_headers(headers: &'a Headers) -> Result<Option<Self>, ParseError> {
        if let Some(forwarded) = Self::from_forwarded_header(headers)? {
            Ok(Some(forwarded))
        } else {
            Self::from_x_headers(headers)
        }
    }

    /// Parse a borrowed Forwarded from the Forwarded header, without x-forwarded-{for,by,proto}
    /// fallback
    ///
    /// # Examples
    /// ```rust
    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut headers = Headers::new();
    /// headers.insert(
    ///     "Forwarded",
    ///     r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown;proto=https"#,
    /// );
    /// let forwarded = Forwarded::from_forwarded_header(&headers)?.unwrap();
    /// assert_eq!(forwarded.proto(), Some("https"));
    /// assert_eq!(
    ///     forwarded.forwarded_for(),
    ///     vec!["192.0.2.43", "[2001:db8:cafe::17]", "unknown"]
    /// );
    /// # Ok(()) }
    /// ```
    /// ```rust
    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut headers = Headers::new();
    /// headers.insert("X-Forwarded-For", "192.0.2.43, 2001:db8:cafe::17");
    /// assert!(Forwarded::from_forwarded_header(&headers)?.is_none());
    /// # Ok(()) }
    /// ```
    pub fn from_forwarded_header(headers: &'a Headers) -> Result<Option<Self>, ParseError> {
        if let Some(headers) = headers.get_str(ForwardedHeader) {
            Ok(Some(Self::parse(headers)?))
        } else {
            Ok(None)
        }
    }

    /// Parse a borrowed Forwarded from the historical
    /// non-standardized x-forwarded-{for,by,proto} headers, without
    /// support for the Forwarded header.
    ///
    /// # Examples
    /// ```rust
    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut headers = Headers::new();
    /// headers.insert("X-Forwarded-For", "192.0.2.43, 2001:db8:cafe::17");
    /// let forwarded = Forwarded::from_headers(&headers)?.unwrap();
    /// assert_eq!(
    ///     forwarded.forwarded_for(),
    ///     vec!["192.0.2.43", "[2001:db8:cafe::17]"]
    /// );
    /// # Ok(()) }
    /// ```
    /// ```rust
    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut headers = Headers::new();
    /// headers.insert(
    ///     "Forwarded",
    ///     r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown;proto=https"#,
    /// );
    /// assert!(Forwarded::from_x_headers(&headers)?.is_none());
    /// # Ok(()) }
    /// ```
    pub fn from_x_headers(headers: &'a Headers) -> Result<Option<Self>, ParseError> {
        let forwarded_for: Vec<Cow<'a, str>> = headers
            .get_str(XforwardedFor)
            .map(|hv| {
                hv.split(',')
                    .map(|v| {
                        let v = v.trim();
                        match v.parse::<IpAddr>().ok() {
                            Some(IpAddr::V6(v6)) => Cow::Owned(format!(r#"[{v6}]"#)),
                            _ => Cow::Borrowed(v),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let by = headers.get_str(XforwardedBy).map(Cow::Borrowed);

        let proto = headers
            .get_str(XforwardedProto)
            .map(Cow::Borrowed)
            .or_else(|| {
                if headers.eq_ignore_ascii_case(XforwardedSsl, "on") {
                    Some(Cow::Borrowed("https"))
                } else {
                    None
                }
            });

        let host = headers.get_str(XforwardedHost).map(Cow::Borrowed);

        if !forwarded_for.is_empty() || by.is_some() || proto.is_some() || host.is_some() {
            Ok(Some(Self {
                forwarded_for,
                by,
                proto,
                host,
            }))
        } else {
            Ok(None)
        }
    }

    /// parse a &str into a borrowed Forwarded
    ///
    /// # Examples
    /// ```rust

    /// # use trillium::Headers;
    /// # use trillium_forwarding::Forwarded;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///
    /// let forwarded = Forwarded::parse(
    ///     r#"for=192.0.2.43,         for="[2001:db8:cafe::17]", FOR=unknown;proto=https"#
    /// )?;
    /// assert_eq!(forwarded.forwarded_for(), vec!["192.0.2.43", "[2001:db8:cafe::17]", "unknown"]);
    /// assert_eq!(
    ///     forwarded.to_string(),
    ///     r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown;proto=https"#
    /// );
    /// # Ok(()) }
    /// ```
    pub fn parse(input: &'a str) -> Result<Self, ParseError> {
        let mut input = input;
        let mut forwarded = Forwarded::new();

        while !input.is_empty() {
            input = if starts_with_ignore_case("for=", input) {
                forwarded.parse_for(input)?
            } else {
                forwarded.parse_forwarded_pair(input)?
            }
        }

        Ok(forwarded)
    }

    fn parse_forwarded_pair(&mut self, input: &'a str) -> Result<&'a str, ParseError> {
        let (key, value, rest) = match parse_token(input) {
            (Some(key), rest) if rest.starts_with('=') => match parse_value(&rest[1..]) {
                (Some(value), rest) => Some((key, value, rest)),
                (None, _) => None,
            },
            _ => None,
        }
        .ok_or_else(|| ParseError::new("parse error in forwarded-pair"))?;

        match key {
            "by" => {
                if self.by.is_some() {
                    return Err(ParseError::new("parse error, duplicate `by` key"));
                }
                self.by = Some(value);
            }

            "host" => {
                if self.host.is_some() {
                    return Err(ParseError::new("parse error, duplicate `host` key"));
                }
                self.host = Some(value);
            }

            "proto" => {
                if self.proto.is_some() {
                    return Err(ParseError::new("parse error, duplicate `proto` key"));
                }
                self.proto = Some(value);
            }

            _ => { /* extensions are allowed in the spec */ }
        }

        match rest.strip_prefix(';') {
            Some(rest) => Ok(rest),
            None => Ok(rest),
        }
    }

    fn parse_for(&mut self, input: &'a str) -> Result<&'a str, ParseError> {
        let mut rest = input;

        loop {
            rest = match match_ignore_case("for=", rest) {
                (true, rest) => rest,
                (false, _) => return Err(ParseError::new("http list must start with for=")),
            };

            let (value, rest_) = parse_value(rest);
            rest = rest_;

            if let Some(value) = value {
                // add a successful for= value
                self.forwarded_for.push(value);
            } else {
                return Err(ParseError::new("for= without valid value"));
            }

            match rest.chars().next() {
                // we have another for=
                Some(',') => {
                    rest = rest[1..].trim_start();
                }

                // we have reached the end of the for= section
                Some(';') => return Ok(&rest[1..]),

                // reached the end of the input
                None => return Ok(rest),

                // bail
                _ => return Err(ParseError::new("unexpected character after for= section")),
            }
        }
    }

    /// Transform a borrowed Forwarded into an owned
    /// Forwarded. This is a noop if the Forwarded is already owned.
    pub fn into_owned(self) -> Forwarded<'static> {
        Forwarded {
            by: self.by.map(|by| Cow::Owned(by.into_owned())),
            forwarded_for: self
                .forwarded_for
                .into_iter()
                .map(|ff| Cow::Owned(ff.into_owned()))
                .collect(),
            host: self.host.map(|h| Cow::Owned(h.into_owned())),
            proto: self.proto.map(|p| Cow::Owned(p.into_owned())),
        }
    }

    /// Builds a new empty Forwarded
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a `for` section to this header
    pub fn add_for(&mut self, forwarded_for: impl Into<Cow<'a, str>>) {
        self.forwarded_for.push(forwarded_for.into());
    }

    /// Returns the `for` field of this header
    pub fn forwarded_for(&self) -> Vec<&str> {
        self.forwarded_for.iter().map(|x| x.as_ref()).collect()
    }

    /// Sets the `host` field of this header
    pub fn set_host(&mut self, host: impl Into<Cow<'a, str>>) {
        self.host = Some(host.into());
    }

    /// Returns the `host` field of this header
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Sets the `proto` field of this header
    pub fn set_proto(&mut self, proto: impl Into<Cow<'a, str>>) {
        self.proto = Some(proto.into())
    }

    /// Returns the `proto` field of this header
    pub fn proto(&self) -> Option<&str> {
        self.proto.as_deref()
    }

    /// Sets the `by` field of this header
    pub fn set_by(&mut self, by: impl Into<Cow<'a, str>>) {
        self.by = Some(by.into());
    }

    /// Returns the `by` field of this header
    pub fn by(&self) -> Option<&str> {
        self.by.as_deref()
    }
}

impl std::fmt::Display for Forwarded<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut needs_semi = false;
        if let Some(by) = self.by() {
            needs_semi = true;
            write!(f, "by={}", format_value(by))?;
        }

        if !self.forwarded_for.is_empty() {
            if needs_semi {
                f.write_char(';')?;
            }
            needs_semi = true;
            f.write_str(
                &self
                    .forwarded_for
                    .iter()
                    .map(|f| format!("for={}", format_value(f)))
                    .collect::<Vec<_>>()
                    .join(", "),
            )?;
        }

        if let Some(host) = self.host() {
            if needs_semi {
                f.write_char(';')?;
            }
            needs_semi = true;
            write!(f, "host={}", format_value(host))?
        }

        if let Some(proto) = self.proto() {
            if needs_semi {
                f.write_char(';')?;
            }
            write!(f, "proto={}", format_value(proto))?
        }

        Ok(())
    }
}

fn parse_value(input: &str) -> (Option<Cow<'_, str>>, &str) {
    match parse_token(input) {
        (Some(token), rest) => (Some(Cow::Borrowed(token)), rest),
        (None, rest) => parse_quoted_string(rest),
    }
}

fn format_value(input: &str) -> Cow<'_, str> {
    match parse_token(input) {
        (_, "") => input.into(),
        _ => {
            let mut string = String::from("\"");
            for ch in input.chars() {
                if let '\\' | '"' = ch {
                    string.push('\\');
                }
                string.push(ch);
            }
            string.push('"');
            string.into()
        }
    }
}

fn match_ignore_case<'a>(start: &'static str, input: &'a str) -> (bool, &'a str) {
    let len = start.len();
    if input[..len].eq_ignore_ascii_case(start) {
        (true, &input[len..])
    } else {
        (false, input)
    }
}

fn starts_with_ignore_case(start: &'static str, input: &str) -> bool {
    if start.len() <= input.len() {
        let len = start.len();
        input[..len].eq_ignore_ascii_case(start)
    } else {
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParseError(&'static str);
impl ParseError {
    pub fn new(msg: &'static str) -> Self {
        Self(msg)
    }
}

impl std::error::Error for ParseError {}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unable to parse forwarded header: {}", self.0)
    }
}

impl<'a> TryFrom<&'a str> for Forwarded<'a> {
    type Error = ParseError;
    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type Result = std::result::Result<(), ParseError>;

    #[test]
    fn starts_with_ignore_case_can_handle_short_inputs() {
        assert!(!starts_with_ignore_case("helloooooo", "h"));
    }

    #[test]
    fn parsing_for() -> Result {
        assert_eq!(
            Forwarded::parse(r#"for="_gazonk""#)?.forwarded_for(),
            vec!["_gazonk"]
        );
        assert_eq!(
            Forwarded::parse(r#"For="[2001:db8:cafe::17]:4711""#)?.forwarded_for(),
            vec!["[2001:db8:cafe::17]:4711"]
        );

        assert_eq!(
            Forwarded::parse("for=192.0.2.60;proto=http;by=203.0.113.43")?.forwarded_for(),
            vec!["192.0.2.60"]
        );

        assert_eq!(
            Forwarded::parse("for=192.0.2.43,   for=198.51.100.17")?.forwarded_for(),
            vec!["192.0.2.43", "198.51.100.17"]
        );

        assert_eq!(
            Forwarded::parse(r#"for=192.0.2.43,for="[2001:db8:cafe::17]",for=unknown"#)?
                .forwarded_for(),
            Forwarded::parse(r#"for=192.0.2.43, for="[2001:db8:cafe::17]", for=unknown"#)?
                .forwarded_for()
        );

        assert_eq!(
            Forwarded::parse(
                r#"for=192.0.2.43,for="this is a valid quoted-string, \" \\",for=unknown"#
            )?
            .forwarded_for(),
            vec![
                "192.0.2.43",
                r#"this is a valid quoted-string, " \"#,
                "unknown"
            ]
        );

        Ok(())
    }

    #[test]
    fn basic_parse() -> Result {
        let forwarded = Forwarded::parse("for=client.com;by=proxy.com;host=host.com;proto=https")?;

        assert_eq!(forwarded.by(), Some("proxy.com"));
        assert_eq!(forwarded.forwarded_for(), vec!["client.com"]);
        assert_eq!(forwarded.host(), Some("host.com"));
        assert_eq!(forwarded.proto(), Some("https"));
        assert!(matches!(forwarded, Forwarded { .. }));
        Ok(())
    }

    #[test]
    fn bad_parse() {
        let err = Forwarded::parse("by=proxy.com;for=client;host=example.com;host").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to parse forwarded header: parse error in forwarded-pair"
        );

        let err = Forwarded::parse("by;for;host;proto").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to parse forwarded header: parse error in forwarded-pair"
        );

        let err = Forwarded::parse("for=for, key=value").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to parse forwarded header: http list must start with for="
        );

        let err = Forwarded::parse(r#"for="unterminated string"#).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to parse forwarded header: for= without valid value"
        );

        let err = Forwarded::parse(r#"for=, for=;"#).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to parse forwarded header: for= without valid value"
        );
    }

    #[test]
    fn bad_parse_from_headers() -> Result {
        let mut headers = Headers::new();
        headers.append("forwarded", "uh oh");
        assert_eq!(
            Forwarded::from_headers(&headers).unwrap_err().to_string(),
            "unable to parse forwarded header: parse error in forwarded-pair"
        );

        let headers = Headers::new();
        assert!(Forwarded::from_headers(&headers)?.is_none());
        Ok(())
    }

    #[test]
    fn from_x_headers() -> Result {
        let mut headers = Headers::new();
        headers.append(XforwardedFor, "192.0.2.43, 2001:db8:cafe::17");
        headers.append(XforwardedProto, "gopher");
        headers.append(XforwardedHost, "example.com");
        let forwarded = Forwarded::from_headers(&headers)?.unwrap();
        assert_eq!(
            forwarded.to_string(),
            r#"for=192.0.2.43, for="[2001:db8:cafe::17]";host=example.com;proto=gopher"#
        );
        Ok(())
    }

    #[test]
    fn from_x_headers_with_ssl_on() -> Result {
        let mut headers = Headers::new();
        headers.append(XforwardedFor, "192.0.2.43, 2001:db8:cafe::17");
        headers.append(XforwardedHost, "example.com");
        headers.append(XforwardedSsl, "on");
        let forwarded = Forwarded::from_headers(&headers)?.unwrap();
        assert_eq!(
            forwarded.to_string(),
            r#"for=192.0.2.43, for="[2001:db8:cafe::17]";host=example.com;proto=https"#
        );
        Ok(())
    }

    #[test]
    fn formatting_edge_cases() {
        let mut forwarded = Forwarded::new();
        forwarded.add_for(r#"quote: " backslash: \"#);
        forwarded.add_for(";proto=https");
        assert_eq!(
            forwarded.to_string(),
            r#"for="quote: \" backslash: \\", for=";proto=https""#
        );

        let mut forwarded = Forwarded::new();
        forwarded.set_host("localhost:8080");
        forwarded.set_proto("not:normal"); // handled correctly but should not happen
        forwarded.set_by("localhost:8081");
        assert_eq!(
            forwarded.to_string(),
            r#"by="localhost:8081";host="localhost:8080";proto="not:normal""#
        );
    }

    #[test]
    fn parse_edge_cases() -> Result {
        let forwarded =
            Forwarded::parse(r#"for=";", for=",", for="\"", for=unquoted;by=";proto=https""#)?;
        assert_eq!(forwarded.forwarded_for(), vec![";", ",", "\"", "unquoted"]);
        assert_eq!(forwarded.by(), Some(";proto=https"));
        assert!(forwarded.proto().is_none());

        let forwarded = Forwarded::parse("proto=https")?;
        assert_eq!(forwarded.proto(), Some("https"));
        Ok(())
    }

    #[test]
    fn owned_parse() -> Result {
        let forwarded =
            Forwarded::parse("for=client;by=proxy.com;host=example.com;proto=https")?.into_owned();

        assert_eq!(forwarded.by(), Some("proxy.com"));
        assert_eq!(forwarded.forwarded_for(), vec!["client"]);
        assert_eq!(forwarded.host(), Some("example.com"));
        assert_eq!(forwarded.proto(), Some("https"));
        assert!(matches!(forwarded, Forwarded { .. }));
        Ok(())
    }

    #[test]
    fn from_headers() -> Result {
        let mut headers = Headers::new();
        headers.append("Forwarded", "for=for");

        let forwarded = Forwarded::from_headers(&headers)?.unwrap();
        assert_eq!(forwarded.forwarded_for(), vec!["for"]);

        Ok(())
    }

    #[test]
    fn owned_can_outlive_headers() -> Result {
        let forwarded = {
            let mut headers = Headers::new();
            headers.append("Forwarded", "for=for;by=by;host=host;proto=proto");
            Forwarded::from_headers(&headers)?.unwrap().into_owned()
        };
        assert_eq!(forwarded.by(), Some("by"));
        Ok(())
    }

    #[test]
    fn round_trip() -> Result {
        let inputs = [
            "for=client,for=b,for=c;by=proxy.com;host=example.com;proto=https",
            "by=proxy.com;proto=https;host=example.com;for=a,for=b",
            "by=proxy.com",
            "proto=https",
            "host=example.com",
            "for=a,for=b",
            r#"by="localhost:8081";host="localhost:8080";proto="not:normal""#,
        ];
        for input in inputs {
            let forwarded = Forwarded::parse(input)?;
            let header = forwarded.to_string();
            let parsed = Forwarded::parse(header.as_str())?;
            assert_eq!(forwarded, parsed);
        }
        Ok(())
    }
}
