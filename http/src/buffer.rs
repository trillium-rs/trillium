use std::{
    fmt::{Debug, Formatter},
    ops::{Deref, DerefMut},
    str,
};

#[derive(Default)]
#[doc(hidden)]
pub struct Buffer(usize, Vec<u8>);

impl Debug for Buffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let slice = self.as_slice();
        if let Ok(str) = str::from_utf8(slice) {
            str.fmt(f)
        } else {
            slice.fmt(f)
        }
    }
}
impl From<Buffer> for Vec<u8> {
    fn from(Buffer(offset, mut vec): Buffer) -> Self {
        if offset > 0 {
            vec.copy_within(offset.., 0);
            vec.truncate(vec.len() - offset);
        }
        vec
    }
}
impl From<Vec<u8>> for Buffer {
    fn from(value: Vec<u8>) -> Self {
        Self(0, value)
    }
}
impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}
#[doc(hidden)]
impl Buffer {
    pub fn truncate(&mut self, n: usize) {
        if n == 0 {
            self.0 = 0;
            self.1.clear();
        } else {
            self.1.truncate(self.0 + n);
        }
    }

    pub fn extend_from_slice(&mut self, slice: &[u8]) {
        self.1.extend_from_slice(slice);
    }

    /// Insert `slice` at the front of the active region, chronologically before the
    /// existing contents.
    ///
    /// Cheap when `slice.len() <= self.0` — writes into the already-consumed prefix left
    /// by prior [`ignore_front`][Self::ignore_front] calls. Otherwise allocates / shifts.
    pub fn prepend(&mut self, slice: &[u8]) {
        if slice.len() <= self.0 {
            self.0 -= slice.len();
            self.1[self.0..self.0 + slice.len()].copy_from_slice(slice);
        } else {
            let active_len = self.len();
            let new_total = slice.len() + active_len;
            self.1.resize(new_total, 0);
            if active_len > 0 {
                self.1.copy_within(self.0..self.0 + active_len, slice.len());
            }
            self.1[..slice.len()].copy_from_slice(slice);
            self.0 = 0;
        }
    }

    pub fn ignore_front(&mut self, n: usize) {
        self.0 += n;
        if self.0 >= self.1.len() {
            self.1.clear();
            self.0 = 0;
        }
    }

    pub fn len(&self) -> usize {
        self.1.len() - self.0
    }

    pub fn is_empty(&self) -> bool {
        self.1.len() == self.0
    }

    pub fn fill_capacity(&mut self) {
        self.1.resize(self.1.capacity(), 0);
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(0, Vec::with_capacity(capacity))
    }

    pub fn expand(&mut self) {
        if self.1.len() == self.1.capacity() {
            let live = self.len();
            // Out of room. Compacting moves `live` bytes to reclaim `self.0` of tail space;
            // reallocating moves up to `capacity` bytes (or none, if the allocator extends in
            // place) but doubles the buffer. Shift down only when it reclaims at least what it
            // moves — the 1:1 break-even, which holds regardless of allocator. Below that, grow.
            if self.0 > 0 && self.0 >= live {
                self.1.copy_within(self.0.., 0);
                self.1.truncate(live);
                self.0 = 0;
            } else {
                self.1.reserve(32);
            }
        }
        self.fill_capacity();
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.1[self.0..]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.1[self.0..]
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn prepend_on_empty_default() {
        let mut buf = Buffer::default();
        buf.prepend(b"hello");
        assert_eq!(&*buf, b"hello");
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.0, 0);
    }

    #[test]
    fn prepend_empty_slice_is_noop() {
        // Empty slice on empty buffer.
        let mut buf = Buffer::default();
        buf.prepend(b"");
        assert!(buf.is_empty());

        // Empty slice on populated buffer doesn't perturb contents or offset.
        let mut buf = Buffer::from(b"abc".to_vec());
        buf.ignore_front(1);
        let offset_before = buf.0;
        buf.prepend(b"");
        assert_eq!(&*buf, b"bc");
        assert_eq!(buf.0, offset_before);
    }

    #[test]
    fn prepend_fast_path_uses_existing_room() {
        // After ignore_front, there's room at the front (offset > 0). Prepending a
        // slice no larger than the offset must take the fast path: no reallocation,
        // no shift, just rewind the offset and write into the existing prefix.
        let mut buf = Buffer::from(b"abcdef".to_vec());
        buf.ignore_front(4); // offset=4, active = "ef"
        assert_eq!(buf.0, 4);
        let ptr_before = buf.1.as_ptr();
        let len_before = buf.1.len();

        buf.prepend(b"XYZ"); // 3 <= 4, fast path
        assert_eq!(&*buf, b"XYZef");
        assert_eq!(buf.0, 1);
        assert_eq!(buf.1.len(), len_before, "underlying vec length unchanged");
        assert_eq!(buf.1.as_ptr(), ptr_before, "no reallocation on fast path");
    }

    #[test]
    fn prepend_fast_path_exact_offset() {
        // Boundary: slice.len() == self.0 — still the fast path, offset goes to 0.
        let mut buf = Buffer::from(b"abcdef".to_vec());
        buf.ignore_front(3); // offset=3, active = "def"
        buf.prepend(b"XYZ"); // 3 == 3, fast path
        assert_eq!(&*buf, b"XYZdef");
        assert_eq!(buf.0, 0);
    }

    #[test]
    fn prepend_slow_path_zero_offset_with_content() {
        // No headroom (offset = 0) and active content present — slow path must shift
        // content and write the prepended bytes at the front.
        let mut buf = Buffer::from(b"world".to_vec());
        assert_eq!(buf.0, 0);
        buf.prepend(b"hello ");
        assert_eq!(&*buf, b"hello world");
        assert_eq!(buf.0, 0);
    }

    #[test]
    fn prepend_slow_path_insufficient_offset() {
        // Some headroom but not enough for the prepend — slow path resizes to grow.
        let mut buf = Buffer::from(b"abcdef".to_vec());
        buf.ignore_front(2); // offset=2, active = "cdef"
        buf.prepend(b"WXYZ"); // 4 > 2, slow path
        assert_eq!(&*buf, b"WXYZcdef");
        assert_eq!(buf.0, 0);
    }

    #[test]
    fn prepend_preserves_order_after_extend() {
        // Residual content sits in the buffer (chronologically *later* bytes), then a
        // prepend pushes chronologically *earlier* bytes onto the front. The final order
        // must match stream order.
        let mut buf = Buffer::default();
        buf.extend_from_slice(b"later"); // chronologically later bytes
        buf.prepend(b"earlier "); // chronologically earlier bytes
        assert_eq!(&*buf, b"earlier later");
    }

    #[test]
    fn expand_compacts_when_offset_reclaims_enough() {
        // Full buffer (len == cap) with a consumed prefix at least as large as the live
        // region: compaction reclaims ≥ what it moves, so expand shifts down in place
        // rather than reallocating.
        let mut buf = Buffer::from(b"ABCDEFGH".to_vec()); // len 8 == cap 8
        let cap_before = buf.1.capacity();
        buf.ignore_front(5); // offset 5, live "FGH" (3); 5 >= 3
        let ptr_before = buf.1.as_ptr();

        buf.expand();

        assert_eq!(buf.1.as_ptr(), ptr_before, "compaction must not reallocate");
        assert_eq!(buf.1.capacity(), cap_before, "capacity unchanged by compaction");
        assert_eq!(buf.0, 0, "offset reset to front");
        assert_eq!(&buf[..3], b"FGH", "live content preserved in order");
    }

    #[test]
    fn expand_grows_when_offset_too_small() {
        // Consumed prefix smaller than the live region: compaction would move more than it
        // reclaims, so expand reallocates instead and leaves the offset in place.
        let mut buf = Buffer::from(b"ABCDEFGH".to_vec());
        let cap_before = buf.1.capacity();
        buf.ignore_front(2); // offset 2, live "CDEFGH" (6); 2 < 6

        buf.expand();

        assert!(buf.1.capacity() > cap_before, "must grow when offset is too small");
        assert_eq!(buf.0, 2, "offset unchanged when growing");
        assert_eq!(&buf[..6], b"CDEFGH", "live content preserved");
    }

    #[test]
    fn expand_grows_default_buffer() {
        // Degenerate case the offset > 0 guard protects: an empty zero-capacity buffer must
        // grow, not loop forever compacting nothing.
        let mut buf = Buffer::default();
        buf.expand();
        assert!(buf.1.capacity() >= 32);
        assert_eq!(buf.len(), buf.1.capacity());
    }

    #[test]
    fn expand_grows_full_buffer_with_no_offset() {
        let mut buf = Buffer::from(b"ABCD".to_vec()); // full, offset 0
        let cap_before = buf.1.capacity();
        buf.expand();
        assert!(buf.1.capacity() > cap_before);
        assert_eq!(&buf[..4], b"ABCD");
    }

    #[test]
    fn into_vec_round_trips_active_region() {
        let mut buf = Buffer::from(b"abcdef".to_vec());
        buf.ignore_front(2);
        assert_eq!(Vec::from(buf), b"cdef");

        // offset 0 path (the guarded no-op shift) is identity on contents.
        let buf = Buffer::from(b"xyz".to_vec());
        assert_eq!(Vec::from(buf), b"xyz");
    }

    #[test]
    fn prepend_then_ignore_front_then_prepend() {
        // After a prepend + drain, the buffer is still in a consistent state for
        // another prepend.
        let mut buf = Buffer::from(b"cccc".to_vec());
        buf.ignore_front(4); // fully drained: vec cleared, offset reset
        assert!(buf.is_empty());
        buf.prepend(b"bb"); // empty + prepend = extend
        assert_eq!(&*buf, b"bb");
        buf.ignore_front(1);
        buf.prepend(b"a"); // 1 == 1, fast path
        assert_eq!(&*buf, b"ab");
    }
}
