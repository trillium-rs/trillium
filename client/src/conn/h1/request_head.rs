use std::io::Write;
use trillium_http::{Error, HeaderName, HeaderValues, Result};

/// Writes each header as a `name: value` line, one line per value. Fails on a name or value that
/// can't be written to the wire, leaving `buf` partially written.
pub(crate) fn write_headers<'a>(
    buf: &mut Vec<u8>,
    headers: impl IntoIterator<Item = (HeaderName<'a>, &'a HeaderValues)>,
) -> Result<()> {
    for (name, values) in headers {
        if !name.is_valid() {
            return Err(Error::InvalidHeaderName);
        }

        for value in values {
            if !value.is_valid() {
                return Err(Error::InvalidHeaderValue(name.to_owned()));
            }
            write!(buf, "{name}: ")?;
            buf.extend_from_slice(value.as_ref());
            write!(buf, "\r\n")?;
        }
    }
    Ok(())
}
