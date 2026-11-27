//! Bytes left only on the card (`VICTORY_LAND_SKIP_WC=1`): what shaders
//! wrote into title memory the title asked to be write-combined is not
//! landed every frame (run YP44: 120-143 MB a frame, the whole landing, and
//! 95 ms a frame against 158). The card's copy is then the only one with
//! those bytes; title memory and the copy's CPU side keep older ones.
//!
//! That holds while the copy lives and nothing else reads the memory. Two
//! things do (black patches in YP44, with the commit charge low):
//! - **Letting go:** an idle copy let go ([`crate::shadow::let_go_idle`];
//!   2 s idle under memory pressure) takes the bytes with it. The next read
//!   makes a new copy from title memory.
//! - **Overlap:** a new copy of the same memory (another buffer over it, a
//!   view in place) starts from title memory too.
//!
//! So each piece left on the card is kept here by title address, with the
//! copy holding it.
//! - A copy holding some is not let go: it is asked for ([`want`]) and the
//!   next landing fetches those pieces with its own ([`take_wanted`]). It
//!   can go at a later sweep, once they are in title memory.
//! - A new copy over some has their holder asked for the same way, and is
//!   counted. Title memory, and the copies of it, catch up when the pieces
//!   land.
//! - A copy that goes while holding some is counted.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::shadow::Shadow;

/// A piece left on the card: its end (title address), the copy holding it
/// and that copy's number, and its offset in the copy.
struct Held {
    end: u64,
    holder: Weak<Shadow>,
    id: u64,
    lo: usize,
}

/// Pieces left on the card, by title address.
static HELD: Mutex<BTreeMap<u64, Held>> = Mutex::new(BTreeMap::new());

/// Copies whose pieces the next landing fetches, each once.
static WANTED: Mutex<Vec<Arc<Shadow>>> = Mutex::new(Vec::new());

/// Since the last line: idle copies kept to land first, bytes landed for
/// them, new copies over pieces held by another, copies gone holding some
/// and their pieces' bytes.
static KEPT: AtomicU64 = AtomicU64::new(0);
static LANDED: AtomicU64 = AtomicU64::new(0);
static OVERLAPS: AtomicU64 = AtomicU64::new(0);
static GONE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// `[lo, hi)` of `shadow` was left on the card.
pub(crate) fn note(shadow: &Arc<Shadow>, lo: usize, hi: usize) {
    if hi <= lo {
        return;
    }
    let start = shadow.span().0;
    let at = start + lo as u64;
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    held.insert(at, Held { end: start + hi as u64, holder: Arc::downgrade(shadow), id: shadow.id(), lo });
}

/// The pieces `shadow` holds, as offsets in it.
pub(crate) fn pieces_of(shadow: &Shadow) -> Vec<(usize, usize)> {
    let (start, len) = shadow.span();
    let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    held.range(start..start + len)
        .filter(|(_, h)| h.id == shadow.id())
        .map(|(&at, h)| (h.lo, h.lo + (h.end - at) as usize))
        .collect()
}

/// Whether `shadow` holds any piece.
pub(crate) fn holds(shadow: &Shadow) -> bool {
    let (start, len) = shadow.span();
    let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    held.range(start..start + len).any(|(_, h)| h.id == shadow.id())
}

/// `pieces` of `shadow` were landed: they are no longer only on the card.
pub(crate) fn landed(shadow: &Shadow, pieces: &[(usize, usize)]) {
    let start = shadow.span().0;
    let mut bytes = 0;
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    for &(lo, hi) in pieces {
        let at = start + lo as u64;
        if held.get(&at).is_some_and(|h| h.id == shadow.id()) {
            held.remove(&at);
            bytes += (hi - lo) as u64;
        }
    }
    LANDED.fetch_add(bytes, Ordering::Relaxed);
}

/// Asks the next landing to fetch `shadow`'s pieces.
pub(crate) fn want(shadow: Arc<Shadow>) {
    let mut wanted = WANTED.lock().unwrap_or_else(|e| e.into_inner());
    if !wanted.iter().any(|s| s.id() == shadow.id()) {
        wanted.push(shadow);
    }
}

/// The copies asked for since the last call.
pub(crate) fn take_wanted() -> Vec<Arc<Shadow>> {
    std::mem::take(&mut *WANTED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// An idle copy was not let go because it holds pieces (asked for).
pub(crate) fn kept(shadow: &Arc<Shadow>) {
    KEPT.fetch_add(1, Ordering::Relaxed);
    want(shadow.clone());
}

/// A new copy of `[start, start + len)` (number `id`) is made: the holders
/// of pieces over it are asked for, and it is counted.
pub(crate) fn made(start: u64, len: u64, id: u64) {
    let holders: Vec<Arc<Shadow>> = {
        let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        if held.is_empty() {
            return;
        }
        // A piece is at most a few blocks long: one that begins before
        // `start` and reaches into it is found from a little before.
        let from = start.saturating_sub(1 << 20);
        held.range(from..start + len)
            .filter(|(_, h)| h.end > start && h.id != id)
            .filter_map(|(_, h)| h.holder.upgrade())
            .collect()
    };
    if holders.is_empty() {
        return;
    }
    OVERLAPS.fetch_add(1, Ordering::Relaxed);
    for h in holders {
        want(h);
    }
}

/// `shadow` goes: pieces it held are counted and forgotten.
pub(crate) fn gone(id: u64, start: u64, len: u64) {
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let ours: Vec<u64> = held.range(start..start + len).filter(|(_, h)| h.id == id).map(|(&at, _)| at).collect();
    if ours.is_empty() {
        return;
    }
    let mut bytes = 0;
    for at in ours {
        if let Some(h) = held.remove(&at) {
            bytes += h.end - at;
        }
    }
    GONE[0].fetch_add(1, Ordering::Relaxed);
    GONE[1].fetch_add(bytes, Ordering::Relaxed);
}

/// For the landing line: bytes held and copies holding them, then since the
/// last call the idle copies kept, bytes landed for them, new copies over
/// held pieces, copies gone holding some and those pieces' bytes.
#[cfg(feature = "diagnostics")]
pub(crate) fn report() -> [u64; 7] {
    let (bytes, copies) = {
        let held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids = std::collections::HashSet::new();
        let bytes: u64 = held
            .iter()
            .map(|(&at, h)| {
                ids.insert(h.id);
                h.end - at
            })
            .sum();
        (bytes, ids.len() as u64)
    };
    [
        bytes,
        copies,
        KEPT.swap(0, Ordering::Relaxed),
        LANDED.swap(0, Ordering::Relaxed),
        OVERLAPS.swap(0, Ordering::Relaxed),
        GONE[0].swap(0, Ordering::Relaxed),
        GONE[1].swap(0, Ordering::Relaxed),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of title memory no other test uses (no buffer behind it).
    fn copy(start: u64, len: u64) -> Arc<Shadow> {
        let data = Box::leak(vec![0u8; len as usize].into_boxed_slice()).as_mut_ptr();
        Shadow::unbacked(start, data, len, true)
    }

    #[test]
    fn pieces_are_kept_by_copy_and_forgotten_once_landed() {
        let a = copy(0x7100_0000, 0x40000);
        note(&a, 0x10000, 0x20000);
        note(&a, 0x30000, 0x40000);
        assert!(holds(&a));
        let mut got = pieces_of(&a);
        got.sort();
        assert_eq!(got, vec![(0x10000, 0x20000), (0x30000, 0x40000)]);
        landed(&a, &[(0x10000, 0x20000)]);
        assert_eq!(pieces_of(&a), vec![(0x30000, 0x40000)]);
        landed(&a, &[(0x30000, 0x40000)]);
        assert!(!holds(&a));
    }

    #[test]
    fn a_new_copy_over_held_pieces_asks_for_their_holder() {
        let a = copy(0x7200_0000, 0x40000);
        note(&a, 0x20000, 0x30000);
        // Another copy of part of the same memory, not over the piece: nothing.
        made(0x7200_0000, 0x10000, 999_999_001);
        assert!(!take_wanted().iter().any(|s| s.id() == a.id()));
        // Over it: its holder is asked for (once, however often).
        made(0x7202_8000, 0x1000, 999_999_002);
        made(0x7202_0000, 0x40000, 999_999_003);
        let wanted = take_wanted();
        assert_eq!(wanted.iter().filter(|s| s.id() == a.id()).count(), 1);
        // The holder's own number does not count as another copy.
        made(0x7202_0000, 0x10000, a.id());
        assert!(!take_wanted().iter().any(|s| s.id() == a.id()));
    }

    #[test]
    fn a_copy_gone_forgets_its_pieces() {
        let a = copy(0x7300_0000, 0x20000);
        note(&a, 0, 0x10000);
        gone(a.id(), 0x7300_0000, 0x20000);
        assert!(!holds(&a));
    }
}
