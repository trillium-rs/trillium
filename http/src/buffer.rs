use std::{
    fmt::{Debug, Formatter},
    ops::Deref,
    str,
};

/// A growable byte buffer that models its regions distinctly.
///
/// Three regions exist, two of them reachable and one managed by [`Vec`]:
///
/// ```text
/// [ ignored | live | window ]
///           [written         ]     <- high-water mark, vec coordinates
/// ```
///
/// * `ignored` — bytes already consumed by `ignore_front`; lazily reclaimed.
/// * `live` — received data, from `ignored` to `written`.
/// * `window` — initialized, not-yet-live space beyond `written`, lent out by [`Buffer::window`]
///   for readers to fill. Every byte handed out is zero until a reader writes it, except bytes
///   partially written into an outstanding lend; [`Buffer::truncate`] and [`Buffer::clear`] drop
///   any such residue.
///
/// The governing invariant: bytes visible through [`Deref`] are bytes that arrived. Space that has
/// not been advanced past the high-water mark is reachable only through [`Buffer::window`], and
/// the buffer only grows when real data demands room — lending a window exposes capacity but
/// cannot make unreceived bytes visible.
#[derive(Default)]
#[doc(hidden)]
pub struct Buffer {
    ignored: usize,
    written: usize,
    /// Length of the window most recently lent by [`Buffer::window`], zeroed by any other
    /// mutation. `advance` may only consume out of this.
    lent: usize,
    vec: Vec<u8>,
}

impl Debug for Buffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Ok(s) = str::from_utf8(self.live()) {
            s.fmt(f)
        } else {
            self.live().fmt(f)
        }
    }
}

impl From<Buffer> for Vec<u8> {
    fn from(buffer: Buffer) -> Self {
        let Buffer {
            ignored,
            written,
            lent: _,
            mut vec,
        } = buffer;
        if ignored > 0 {
            vec.copy_within(ignored..written, 0);
        }
        vec.truncate(written - ignored);
        vec
    }
}

impl From<Vec<u8>> for Buffer {
    fn from(value: Vec<u8>) -> Self {
        let written = value.len();
        Self {
            ignored: 0,
            written,
            lent: 0,
            vec: value,
        }
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.live()
    }
}

#[doc(hidden)]
impl Buffer {
    /// Reclaim the ignored prefix when it holds at least as many bytes as the
    /// live region — the break-even at which moving live bytes cannot lose.
    fn maybe_compact(&mut self) {
        let live_len = self.live_len();
        if self.ignored > 0 && self.ignored >= live_len {
            self.vec.copy_within(self.ignored..self.written, 0);
            self.written -= self.ignored;
            self.ignored = 0;
            self.lent = 0;
            self.vec.truncate(self.written);
        }
    }

    /// Received bytes.
    pub fn live(&self) -> &[u8] {
        &self.vec[self.ignored..self.written]
    }

    /// Received bytes, mutably. Not-yet-written space is not reachable through
    /// deref; use [`Buffer::window`] to lend writable space.
    pub fn live_mut(&mut self) -> &mut [u8] {
        &mut self.vec[self.ignored..self.written]
    }

    pub fn live_len(&self) -> usize {
        self.written - self.ignored
    }

    pub fn is_empty(&self) -> bool {
        self.ignored == self.written
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }

    /// Discard `n` leading bytes of the live region.
    ///
    /// Resets to empty once everything is consumed, releasing the allocation's
    /// length while retaining its capacity.
    pub fn ignore_front(&mut self, n: usize) {
        assert!(n <= self.live_len(), "ignore_front past the live region");
        self.lent = 0;
        self.ignored += n;
        if self.ignored >= self.written {
            self.ignored = 0;
            self.written = 0;
            self.vec.clear();
        }
    }

    /// Empty the buffer, releasing the allocation's length while retaining its
    /// capacity.
    ///
    /// This is [`Vec::clear`]'s contract: the allocation is reused by
    /// subsequent writes rather than returned to the allocator.
    pub fn clear(&mut self) {
        self.lent = 0;
        self.ignored = 0;
        self.written = 0;
        self.vec.clear();
    }

    /// Drop every byte beyond the high-water mark, keeping the live region and
    /// the allocation.
    ///
    /// This is [`Vec::truncate`] with the destination length elided: the type
    /// knows how many bytes are real. Dropped bytes leave the buffer's
    /// management entirely, so nothing received during an earlier span of the
    /// connection can be re-lent afterwards — and because the only route back
    /// to that span is growth, which arrives zeroed, everything
    /// [`Buffer::window`] lends after a truncate is fresh zeros.
    pub fn truncate(&mut self) {
        self.lent = 0;
        self.vec.truncate(self.written);
    }

    /// Append received bytes to the live region.
    ///
    /// Any outstanding window is reclaimed: the append pins the high-water
    /// mark and the vec length shrinks back to it before extending.
    pub fn extend_live(&mut self, data: &[u8]) {
        self.lent = 0;
        self.vec.resize(self.written, 0);
        self.vec.extend_from_slice(data);
        self.written += data.len();
    }

    /// Insert `data` at the front of the live region, chronologically before the
    /// existing contents.
    ///
    /// Cheap when `data.len() <= self.ignored` — writes into the already-
    /// consumed prefix left by prior [`ignore_front`](Self::ignore_front)
    /// calls. Otherwise shifts or allocates.
    pub fn prepend(&mut self, data: &[u8]) {
        self.lent = 0;
        if data.len() <= self.ignored {
            self.ignored -= data.len();
            self.vec[self.ignored..self.ignored + data.len()].copy_from_slice(data);
        } else {
            let live_len = self.live_len();
            let new_written = data.len() + live_len;
            self.maybe_compact();
            self.vec.resize(new_written.max(self.written), 0);
            if live_len > 0 && self.ignored > 0 {
                self.vec.copy_within(self.ignored..self.written, data.len());
            } else if live_len > 0 {
                self.vec.copy_within(..live_len, data.len());
            }
            self.vec[..data.len()].copy_from_slice(data);
            self.ignored = 0;
            self.written = new_written;
        }
    }

    /// Lend `want` bytes of initialized, unwritten space starting at the
    /// high-water mark, for a reader to fill.
    ///
    /// Returns exactly `want` bytes — never more — so a reader bounded by the
    /// slice length cannot consume past its budget. Growing the allocation
    /// happens only when real demand (`written + want`) exceeds it, and zeroes
    /// the newly grown span; space reclaimed by an earlier truncation may hold
    /// stale bytes instead (see the type-level docs). When the ignored prefix
    /// can fund the demand, it is reclaimed instead.
    ///
    /// Lending replaces any prior outstanding lend.
    ///
    /// In debug builds, lent bytes that did not arrive freshly zeroed are
    /// poisoned with `0xa5`, so a reader that consumes past what it wrote
    /// fails loudly instead of quietly reusing residue.
    pub fn window(&mut self, want: usize) -> &mut [u8] {
        let established = self.vec.len();
        if self.written + want > established {
            self.maybe_compact();
            let needed = self.written + want;
            if needed > self.vec.len() {
                self.vec.reserve(needed - self.vec.len());
                self.vec.resize(needed, 0);
            }
        }
        let end = (self.written + want).min(self.vec.len());
        self.lent = end - self.written;
        let window = &mut self.vec[self.written..end];

        #[cfg(debug_assertions)]
        {
            // Bytes that did not arrive freshly zeroed were handed out by an
            // earlier lend or survived from before a truncate. A reader that
            // consumes more than it actually wrote must meet conspicuous
            // garbage, not plausible-looking residue, so poison them here.
            let residue = established.min(end).saturating_sub(self.written);
            window[..residue].fill(0xA5);
        }

        window
    }

    /// Record that `n` bytes were written into the most recent
    /// [`window`](Self::window).
    ///
    /// # Panics
    ///
    /// If `n` exceeds the currently lent window length.
    pub fn advance(&mut self, n: usize) {
        assert!(n <= self.lent, "advance past the lent window");
        self.written += n;
        self.lent -= n;
    }

    #[cfg(test)]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ignored: 0,
            written: 0,
            lent: 0,
            vec: Vec::with_capacity(capacity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn from_vec_and_back_compacts() {
        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.ignore_front(2);
        assert_eq!(buffer.live(), b"cdef");
        assert_eq!(Vec::<u8>::from(buffer), b"cdef".to_vec());
    }

    #[test]
    fn extend_live_appends_at_the_high_water_mark() {
        let mut buffer = Buffer::from(b"ab".to_vec());
        buffer.ignore_front(1);
        buffer.extend_live(b"cd");
        assert_eq!(buffer.live(), b"bcd");
    }

    #[test]
    fn window_lends_exactly_want_bytes() {
        let mut buffer = Buffer::with_capacity(1024);
        assert_eq!(buffer.window(16).len(), 16);

        // A second request without advancing lends the same span.
        assert_eq!(buffer.window(16).len(), 16);
        assert_eq!(buffer.live_len(), 0);
        assert_eq!(buffer.capacity(), 1024);
    }

    #[test]
    fn window_growth_tracks_demand_not_iteration_count() {
        let mut buffer = Buffer::with_capacity(32);
        for _ in 0..4 {
            buffer.window(1)[0] = b'A';
            buffer.advance(1);
        }
        assert_eq!(buffer.live(), b"AAAA");
        // Four dripped bytes stay comfortably inside the initial allocation:
        // growth is a function of demand, not of how many times a reader asked.
        assert_eq!(buffer.capacity(), 32);
    }

    #[test]
    fn advancing_then_ignoring_keeps_coordinates_consistent() {
        let mut buffer = Buffer::with_capacity(8);
        buffer.window(4).copy_from_slice(b"wxyz");
        buffer.advance(4);
        buffer.ignore_front(2);
        assert_eq!(buffer.live(), b"yz");

        buffer.window(2).copy_from_slice(b"ab");
        buffer.advance(2);
        assert_eq!(buffer.live(), b"yzab");
    }

    #[test]
    fn clear_empties_and_releases_length_but_retains_capacity() {
        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.clear();
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), 6);

        buffer.extend_live(b"x");
        assert_eq!(buffer.live(), b"x");
    }

    #[test]
    fn truncate_keeps_live_and_relends_zeros() {
        let mut buffer = Buffer::from(b"abcd".to_vec());
        buffer.ignore_front(1); // live "bcd"
        buffer.window(4); // outstanding lend holds space beyond the high-water mark

        buffer.truncate();
        assert_eq!(buffer.live(), b"bcd");

        // Everything lent afterwards arrives fresh: dropped bytes left the
        // buffer's management, and regrowth zeroes as it exposes.
        assert_eq!(buffer.window(8), &[0u8; 8]);

        buffer.window(2).copy_from_slice(b"ef");
        buffer.advance(2);
        assert_eq!(buffer.live(), b"bcdef");
    }

    #[test]
    fn prepend_fast_path_writes_into_the_ignored_prefix() {
        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.ignore_front(4); // live = "ef"
        buffer.prepend(b"XY"); // 2 <= 4
        assert_eq!(buffer.live(), b"XYef");
    }

    #[test]
    fn prepend_slow_path_shifts_or_allocates() {
        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.prepend(b"XYZ");
        assert_eq!(buffer.live(), b"XYZabcdef");

        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.ignore_front(5); // live = "f"
        buffer.prepend(b"WXYZ"); // 4 <= 5, fast path
        assert_eq!(buffer.live(), b"WXYZf");

        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.ignore_front(2); // live = "cdef"
        buffer.prepend(b"WXYZ"); // 4 > 2, shift
        assert_eq!(buffer.live(), b"WXYZcdef");
    }

    #[test]
    fn prepend_after_extend_maintains_chronology() {
        let mut buffer = Buffer::from(b"def".to_vec());
        buffer.extend_live(b"later");
        buffer.prepend(b"earlier ");
        assert_eq!(buffer.live(), b"earlier deflater");
    }

    #[test]
    fn ignore_front_past_the_end_resets() {
        let mut buffer = Buffer::from(b"abc".to_vec());
        buffer.ignore_front(3);
        assert!(buffer.is_empty());
        buffer.extend_live(b"x");
        assert_eq!(buffer.live(), b"x");
    }

    #[test]
    fn window_compacts_instead_of_growing_when_the_prefix_can_fund_it() {
        let mut buffer = Buffer::from(b"abcdefgh".to_vec()); // len 8 == cap 8
        buffer.ignore_front(6); // live = "gh"
        let window = buffer.window(6);
        window[..2].copy_from_slice(b"ij");
        buffer.advance(2);
        assert_eq!(buffer.live(), b"ghij");
        assert_eq!(buffer.capacity(), 8);
    }

    #[test]
    #[should_panic(expected = "advance past the lent window")]
    fn advancing_past_the_lent_window_panics() {
        let mut buffer = Buffer::default();
        let window = buffer.window(16);
        window[..4].copy_from_slice(b"abcd");
        buffer.advance(4);

        // Capacity from the first lend outlives it. Only two bytes were lent on
        // the second window; advancing eight must not promote six unwritten or
        // stale bytes into the live region.
        buffer.window(2);
        buffer.advance(8);
    }

    #[test]
    fn partially_filling_a_window_then_relending_the_remainder() {
        let mut buffer = Buffer::default();
        buffer.window(4)[..2].copy_from_slice(b"ab");
        buffer.advance(2);

        // The second lend starts at the high-water mark, not the original one.
        buffer.window(2).copy_from_slice(b"cd");
        buffer.advance(2);
        assert_eq!(buffer.live(), b"abcd");
    }

    #[test]
    fn zero_length_window_lends_nothing() {
        let mut buffer = Buffer::with_capacity(64);
        assert!(buffer.window(0).is_empty());
        buffer.advance(0);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), 64);
    }

    #[test]
    #[should_panic(expected = "ignore_front past the live region")]
    fn ignore_front_past_the_live_region_panics() {
        let mut buffer = Buffer::from(b"abc".to_vec());
        buffer.ignore_front(4);
    }

    #[test]
    fn prepend_onto_an_empty_default_buffer() {
        let mut buffer = Buffer::default();
        buffer.prepend(b"hello");
        assert_eq!(buffer.live(), b"hello");
    }

    #[test]
    fn prepending_an_empty_slice_is_a_noop() {
        let mut buffer = Buffer::default();
        buffer.prepend(b"");
        assert!(buffer.is_empty());

        let mut buffer = Buffer::from(b"abc".to_vec());
        buffer.ignore_front(1);
        buffer.prepend(b"");
        assert_eq!(buffer.live(), b"bc");
    }

    #[test]
    fn into_vec_drops_outstanding_window_bytes() {
        let mut buffer = Buffer::default();
        buffer.window(32);
        assert_eq!(Vec::<u8>::from(buffer), Vec::<u8>::new());

        let mut buffer = Buffer::default();
        buffer.extend_live(b"abc");
        buffer.ignore_front(1);
        buffer.window(8);
        assert_eq!(Vec::<u8>::from(buffer), b"bc".to_vec());
    }

    #[test]
    fn extend_live_after_a_window_reclaims_the_lend() {
        let mut buffer = Buffer::default();
        buffer.window(16);
        buffer.extend_live(b"xy");
        assert_eq!(buffer.live(), b"xy");

        buffer.window(2).copy_from_slice(b"zw");
        buffer.advance(2);
        assert_eq!(buffer.live(), b"xyzw");
    }

    #[test]
    fn window_below_the_compaction_break_even_grows_instead() {
        let mut buffer = Buffer::from(b"abcdefgh".to_vec());
        buffer.ignore_front(3); // 3 < 5 live bytes: compaction would move more than it reclaims
        assert_eq!(buffer.live(), b"defgh");
        buffer.window(6);
        assert!(
            buffer.capacity() >= 14,
            "demand exceeded capacity without compaction"
        );
        assert_eq!(buffer.live(), b"defgh");
    }

    #[test]
    fn debug_shows_only_the_live_region() {
        let mut buffer = Buffer::from(b"abcdef".to_vec());
        buffer.ignore_front(2);
        buffer.window(8);
        assert_eq!(format!("{buffer:?}"), format!("{:?}", "cdef"));

        let buffer = Buffer::from(vec![0xFF, b'a']);
        assert_eq!(format!("{buffer:?}"), format!("{:?}", [0xFFu8, b'a']));
    }

    #[test]
    fn live_mut_exposes_exactly_the_live_region() {
        let mut buffer = Buffer::from(b"abcd".to_vec());
        buffer.ignore_front(1);
        buffer.window(8); // an outstanding lend is not reachable through live_mut
        assert_eq!(buffer.live_mut().len(), 3);
        buffer.live_mut()[0] = b'X';
        assert_eq!(buffer.live(), b"Xcd");
    }

    #[test]
    fn random_operation_sequences_match_a_reference_model() {
        fn xorshift(state: &mut u64) -> u64 {
            let x = *state;
            let x = x ^ (x >> 12);
            let x = x ^ (x << 25);
            let x = x ^ (x >> 27);
            *state = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut buffer = Buffer::default();
        let mut model: Vec<u8> = Vec::new();

        for round in 0..5000u64 {
            match xorshift(&mut state) % 100 {
                // consume from the front
                0..=29 => {
                    let n = (xorshift(&mut state) as usize) % (model.len() + 1);
                    buffer.ignore_front(n);
                    model.drain(..n);
                }
                // append at the high-water mark
                30..=49 => {
                    let len = (xorshift(&mut state) as usize) % 64;
                    let data: Vec<u8> = (0..len).map(|_| xorshift(&mut state) as u8).collect();
                    buffer.extend_live(&data);
                    model.extend_from_slice(&data);
                }
                // lend a window, fill a prefix, advance by exactly what was filled
                50..=64 => {
                    let want = (xorshift(&mut state) as usize) % 48 + 1;
                    let filled = (xorshift(&mut state) as usize) % (want + 1);
                    let fill = xorshift(&mut state) as u8;
                    buffer.window(want)[..filled].fill(fill);
                    buffer.advance(filled);
                    model.resize(model.len() + filled, fill);
                }
                // release everything beyond the high-water mark: invisible to
                // the model's live region, but every later lend must still behave
                65..=72 => {
                    buffer.truncate();
                    assert_eq!(buffer.vec.len(), buffer.written);
                }
                // empty the buffer wholesale
                73..=79 => {
                    buffer.clear();
                    model.clear();
                }
                // insert at the front, chronologically earlier
                75..=89 => {
                    let len = (xorshift(&mut state) as usize) % 32;
                    let data: Vec<u8> = (0..len).map(|_| xorshift(&mut state) as u8).collect();
                    buffer.prepend(&data);
                    model.splice(..0, data);
                }
                // occasionally rebuild through the Into<Vec> conversion
                _ => {
                    let vec = Vec::from(buffer);
                    assert_eq!(vec, model, "round {round}");
                    buffer = Buffer::from(vec);
                }
            }

            assert_eq!(buffer.live(), &model[..], "round {round}");
            assert!(buffer.ignored <= buffer.written, "round {round}");
            assert!(buffer.written <= buffer.vec.len(), "round {round}");
            assert!(
                buffer.lent <= buffer.vec.len() - buffer.written,
                "round {round}"
            );
        }
    }
}
