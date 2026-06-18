use super::{DEFAULT_URGENCY, MAX_URGENCY, Priority};

/// Parse a `priority` header value into a [`Priority`], degrading gracefully.
///
/// The value is an RFC 8941 structured-fields dictionary; only the `u` (urgency,
/// integer) and `i` (incremental, boolean) members are meaningful. Unknown members,
/// parameters, and malformed values are skipped, leaving the corresponding default in
/// place — a peer's bad signal never costs the request.
pub(super) fn parse(input: &str) -> Priority {
    let mut priority = Priority::default();

    for member in input.split(',') {
        // A member may carry `;`-delimited parameters; none are defined for this
        // scheme, so keep only the key=value head and let future ones pass through.
        let member = member.split(';').next().unwrap_or("").trim();
        if member.is_empty() {
            continue;
        }

        let (key, value) = match member.split_once('=') {
            Some((key, value)) => (key.trim(), Some(value.trim())),
            None => (member, None),
        };

        match key {
            "u" => {
                priority.urgency = value
                    .and_then(|v| v.parse::<i64>().ok())
                    .and_then(|n| u8::try_from(n).ok())
                    .filter(|u| *u <= MAX_URGENCY)
                    .unwrap_or(DEFAULT_URGENCY);
            }
            "i" => {
                priority.incremental = matches!(value, None | Some("?1"));
            }
            _ => {}
        }
    }

    priority
}

#[cfg(test)]
mod test {
    use super::parse;

    #[test]
    fn well_formed() {
        assert_eq!(parse("u=5").urgency(), 5);
        assert_eq!(parse("u=0, i").urgency(), 0);
        assert!(parse("u=0, i").is_incremental());
        assert!(parse("u=2, i=?1").is_incremental());
        assert!(!parse("u=2, i=?0").is_incremental());
        assert!(!parse("u=2").is_incremental());
    }

    #[test]
    fn whitespace_and_order() {
        let p = parse("  i ,  u=1  ");
        assert_eq!(p.urgency(), 1);
        assert!(p.is_incremental());
    }

    #[test]
    fn unknown_members_and_parameters_ignored() {
        let p = parse("u=4;q=0.5, x=9, foo");
        assert_eq!(p.urgency(), 4);
        assert!(!p.is_incremental());
    }

    #[test]
    fn malformed_falls_to_default() {
        // out of range, negative, and non-integer urgencies all default to 3
        assert_eq!(parse("u=9").urgency(), 3);
        assert_eq!(parse("u=-1").urgency(), 3);
        assert_eq!(parse("u=abc").urgency(), 3);
        assert_eq!(parse("u").urgency(), 3);
        assert_eq!(parse("").urgency(), 3);
        assert_eq!(parse("garbage").urgency(), 3);
    }
}
