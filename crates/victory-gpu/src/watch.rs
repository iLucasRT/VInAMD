//! Which title memory the title wrote since a host copy took it (decision
//! 0007, "Copies of title memory"). The runtime reserves title GPU memory
//! with write watching (`victory-hle` xmem), so the kernel keeps a dirty bit
//! per 4 KiB page; asking for a range's written pages also clears them. Host
//! copies of the same memory can overlap (nested buffers, textures in
//! buffers), so what one asks is kept here for the others: a journal of
//! each page's last write, numbered by the asking.
//!
//! Physical-page views (xmem's sections) cannot be write-watched. There a
//! copy leaves each 64 KiB block it took read-only, and our vectored handler
//! takes the first write to it: it notes the block as written and gives the
//! write access back. The runtime tells us when it maps or unmaps views
//! ([`title_memory_remapped`]), which resets their protection. Blocks found
//! wholly uncommitted are skipped while they remain so; one query can
//! confirm a whole region, including ordinary reservations between views.
//!
//! shortcut: writes into physical pages through another view (xmem's
//! `write_physical`, DirectStorage's page destinations) are not seen at
//! the guarded addresses; they go to the texture pool, whose textures are
//! hashed, not guarded. A system call writing into a guarded block fails
//! (none is known to).

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The unit tracked and copied: 64 KiB, the console's page (the kernel's
/// 4 KiB pages fold into it). Finer units made the checks of the world's
/// big buffers cost more than the copies they saved.
pub(crate) const PAGE: usize = 64 << 10;
/// The kernel's page, the unit of its answers.
const KERNEL_PAGE: usize = 4096;

/// A title-memory range whose changed pages another subsystem needs to read
/// (after fxceless/victory-gta6 a26859f).
pub(crate) struct Tracked {
    start: usize,
    end: usize,
    seen: Mutex<BTreeMap<usize, u64>>,
}

/// Tracks a runtime GPU-memory range, including physical-page views when
/// `views` is true. Other process memory is not tracked.
pub(crate) fn track(start: usize, len: usize, views: bool) -> Option<Tracked> {
    let end = start.checked_add(len)?;
    if len == 0 || writes(start, end, views).is_none() {
        return None;
    }
    Some(Tracked {
        start,
        end,
        seen: Mutex::default(),
    })
}

impl Tracked {
    /// Takes changed pages of `[from, to)`, cut to the tracked range. A
    /// partial page stays changed so a later take can read its other bytes.
    pub(crate) fn take(&self, from: usize, to: usize) -> Vec<(usize, usize)> {
        let (from, to) = (from.max(self.start), to.min(self.end));
        if from >= to {
            return Vec::new();
        }
        let Some((_, pages)) = writes(from, to, true) else {
            return Vec::new();
        };
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (page, write) in pages {
            if seen.get(&page).is_some_and(|&last| last >= write) {
                continue;
            }
            let (start, end) = ((page * PAGE).max(from), ((page + 1) * PAGE).min(to));
            if start >= end {
                continue;
            }
            // Keep partial pages dirty: the caller has not taken the rest.
            if (page * PAGE).max(self.start) >= from && ((page + 1) * PAGE).min(self.end) <= to {
                seen.insert(page, write);
            }
            match ranges.last_mut() {
                Some((_, last)) if *last == start => *last = end,
                _ => ranges.push((start, end)),
            }
        }
        ranges
    }
}

struct Journal {
    /// Numbers the askings; 0 is "never".
    asked: u64,
    /// Page number → the asking that last found it written.
    written: BTreeMap<usize, u64>,
    /// Room for the kernel's answer.
    found: Vec<usize>,
    /// Page ranges asked ahead for a submission's refreshes ([`ask_ahead`])
    /// and the asking that did: `writes` answers for a range inside one
    /// from the journal, without another kernel call.
    ahead: Vec<(usize, usize)>,
    ahead_asked: u64,
    /// The ahead asking stands for a whole submission ([`ask_once`]): no
    /// other ahead asking is made, and `ahead` stays, until
    /// [`ask_once_end`].
    kept: bool,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    asked: 0,
    written: BTreeMap::new(),
    found: Vec::new(),
    ahead: Vec::new(),
    ahead_asked: 0,
    kept: false,
});

/// Ranges of one submission closer than this are asked about together:
/// walking the pages between them costs less than another kernel call (a
/// call that resets written pages costs 30-70 microseconds whatever the
/// range).
const AHEAD_GAP: usize = (8 << 20) / PAGE;
/// The most one asking covers (its answer needs 8 bytes per 4 KiB page).
const AHEAD_MAX: usize = (1 << 30) / PAGE;

/// Asks the kernel about many ranges `(start, end)` at once, before the
/// [`writes`] of each: ranges close together are one asking, and `writes`
/// then answers from the journal until [`ask_ahead_done`]. The world's
/// submissions refresh thousands of host copies (6,664 askings a frame,
/// 456 ms, in run 14). Ranges that are not write-watched, or lie alone, are
/// left to `writes`.
pub(crate) fn ask_ahead(ranges: &[(usize, usize)]) {
    let mut pieces: Vec<(usize, usize)> = ranges
        .iter()
        .filter(|r| r.0 < r.1)
        .map(|&(start, end)| (start / PAGE, end.div_ceil(PAGE)))
        .collect();
    if pieces.len() < 2 {
        return;
    }
    pieces.sort_unstable();
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    // The submission's one asking stands ([`ask_once`]): what it does not
    // hold is left to `writes`.
    if j.kept {
        return;
    }
    j.asked += 1;
    j.ahead_asked = j.asked;
    j.ahead.clear();
    let mut i = 0;
    while i < pieces.len() {
        let first = pieces[i].0;
        let mut last = pieces[i].1;
        let mut k = i + 1;
        while k < pieces.len()
            && pieces[k].0 <= last + AHEAD_GAP
            && last.max(pieces[k].1) - first <= AHEAD_MAX
        {
            last = last.max(pieces[k].1);
            k += 1;
        }
        ask_run(j, &pieces[i..k]);
        i = k;
    }
}

/// Since the last frame line: the kernel askings that took a millisecond
/// or more, their microseconds, and the longest asking's. An asking walks
/// page tables, some ten microseconds for most ranges; in run 75 the 2,644
/// askings of a frame took 219 ms in the lines where the pass over the
/// heap's views ran long, 25-50 ms in the others. These numbers say
/// whether a few askings wait for something or all of them are slow.
#[cfg(feature = "diagnostics")]
static SLOW: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

#[cfg(feature = "diagnostics")]
fn slow(us: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    if us >= 1000 {
        SLOW[0].fetch_add(1, Relaxed);
        SLOW[1].fetch_add(us, Relaxed);
    }
    SLOW[2].fetch_max(us, Relaxed);
}

/// [`SLOW`], taken.
#[cfg(feature = "diagnostics")]
pub(crate) fn slow_asks() -> [u64; 3] {
    [0, 1, 2].map(|i| SLOW[i].swap(0, std::sync::atomic::Ordering::Relaxed))
}

/// The submission's refreshes are done: `writes` asks the kernel again.
pub(crate) fn ask_ahead_done() {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if !j.kept {
        j.ahead.clear();
    }
}

/// Whether a submission asks the kernel once, before its first refresh,
/// about every range its lists refresh (`VICTORY_ASK_ONCE=1`; off by
/// default).
///
/// A submission's refreshes are done in groups, between the title's own
/// buffer copies (each copy flushes the refreshes recorded before it), and
/// every group asked the kernel again: 2,644 askings a frame, 70 a
/// submission. With the switch the journal answers all of them from one
/// asking made when the submission starts.
///
/// What changes: memory the title's other threads write while the
/// submission is being prepared is seen by the next submission's asking,
/// not by this one's later groups. What this submission's lists read was
/// written before they were submitted, so before the asking. Our own
/// writes into title memory while preparing are told to the journal
/// ([`wrote`]) or end the asking ([`ask_once_end`]).
pub(crate) fn once() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // On by default since run 81 (with card buffers: 375-450 ms a frame
    // against 573-647, the picture unchanged over four minutes);
    // `VICTORY_ASK_ONCE=0` asks per range, as before.
    *ON.get_or_init(|| std::env::var_os("VICTORY_ASK_ONCE").is_none_or(|v| v != "0"))
}

/// With [`once`]: asks about `ranges` (everything a submission's lists
/// refresh) as [`ask_ahead`] does, and lets that asking stand until
/// [`ask_once_end`].
pub(crate) fn ask_once(ranges: &[(usize, usize)]) {
    if !once() {
        return;
    }
    ask_once_end();
    ask_ahead(ranges);
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    if j.ahead.is_empty() {
        // `ask_ahead` asked nothing (fewer than two ranges).
        j.asked += 1;
        j.ahead_asked = j.asked;
    }
    // `ask_ahead` leaves a range that lies alone to `writes`, which asks
    // once per flush: with one asking for the submission that was every
    // such range asked again at each of its 70 flushes (run 76: 2,300
    // askings a frame with the switch on, as without it). They are asked
    // here, once; what is not write-watched stays with `writes`.
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        if !j.ahead.iter().any(|&(a, b)| a <= first && last <= b) && ask_kernel(j, first, last) {
            j.ahead.push((first, last));
        }
    }
    j.kept = !j.ahead.is_empty();
}

/// The submission's one asking stands no longer: its refreshes are done,
/// or title memory was written in a way the journal was not told.
pub(crate) fn ask_once_end() {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if j.kept {
        j.kept = false;
        j.ahead.clear();
    }
}

/// We wrote title memory `[at, at + len)` while a submission's one asking
/// stands (the title's buffer copies, done at submission): its pages count
/// as written after that asking, so the refreshes that follow copy them.
pub(crate) fn wrote(at: usize, len: usize) {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if !j.kept || len == 0 {
        return;
    }
    j.asked += 1;
    let now = j.asked;
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        j.written.insert(page, now);
    }
}

/// One kernel asking for `pieces` (page ranges in order); by halves if it
/// fails, as a run across two reservations or into memory that is not
/// write-watched does. A piece alone is left to [`writes`].
fn ask_run(j: &mut Journal, pieces: &[(usize, usize)]) {
    if pieces.len() < 2 {
        return;
    }
    let first = pieces[0].0;
    let last = pieces.iter().map(|p| p.1).max().unwrap_or(first);
    if ask_kernel(j, first, last) {
        j.ahead.push((first, last));
        return;
    }
    let (a, b) = pieces.split_at(pieces.len() / 2);
    ask_run(j, a);
    ask_run(j, b);
}

/// Asks the kernel for the written pages of pages `[first, last)` and notes
/// them in the journal under the ahead asking; false (and nothing changed)
/// if the range is not write-watched memory of one reservation.
fn ask_kernel(j: &mut Journal, first: usize, last: usize) -> bool {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let mut count = (last - first) * PAGE / KERNEL_PAGE;
    j.found.resize(count.max(j.found.len()), 0);
    let mut granularity = 0;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    // SAFETY: the answer buffer holds `count` entries.
    let failed = unsafe {
        GetWriteWatch(
            WRITE_WATCH_FLAG_RESET,
            (first * PAGE) as *const _,
            (last - first) * PAGE,
            j.found.as_mut_ptr().cast(),
            &mut count,
            &mut granularity,
        )
    } != 0;
    if failed {
        return false;
    }
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::Watches, 1);
        add(C::WatchUs, started.elapsed().as_micros() as u64);
        slow(started.elapsed().as_micros() as u64);
    }
    let now = j.ahead_asked;
    for k in 0..count {
        let page = j.found[k] / PAGE;
        j.written.insert(page, now);
    }
    true
}

/// What [`writes`] would say of `[start, end)` from the journal alone, if
/// the range lies in one asked ahead ([`ask_ahead`], [`ask_once`]): the
/// pages written since tracking began and the asking that last found each
/// written, with the number of the asking that stands (as [`writes`] gives
/// it). `None` if the kernel would have to be asked.
pub(crate) fn asked(start: usize, end: usize) -> Option<(u64, Vec<(usize, u64)>)> {
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    let j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    j.ahead
        .iter()
        .any(|&(a, b)| a <= first && last <= b)
        .then(|| (j.ahead_asked, j.written.range(first..last).map(|(&p, &w)| (p, w)).collect()))
}

/// The pages of `[start, end)` (whole pages) as last written, for pages
/// written since tracking began: `(page number, asking)` in page order, and
/// this asking's number, which is higher than any in the list but those the
/// kernel just reported. `None` if the memory is not write-watched (nor,
/// with `guarded`, view memory, which is then guarded).
pub(crate) fn writes(start: usize, end: usize, guarded: bool) -> Option<(u64, Vec<(usize, u64)>)> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    WHOLLY_CLEAN.set(false);
    ANSWER_STAMP.set(None);
    if guarded
        && batch()
        && let Some(answer) = ANSWERS.read().unwrap_or_else(|e| e.into_inner()).get(&(first, last))
    {
        WHOLLY_CLEAN.set(answer.clean);
        ANSWER_STAMP.set(Some(answer.stamp));
        return Some((answer.now, answer.written.clone()));
    }
    // View memory, known by a guard of ours: not the kernel's to answer.
    // (The guards are taken once for the whole answer. Every refreshing
    // thread wants them at once, and three times for an answer was 22% of
    // the submitting thread's time, waiting for its turns.)
    if guarded && !journal_covers(first, last) {
        let g = guards();
        if known_view(&g, start) {
            return guarded_writes(first, last, Some(g));
        }
    }
    let mut held = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *held;
    // Asked ahead with the submission's other ranges: the journal has it.
    if j.ahead.iter().any(|&(a, b)| a <= first && last <= b) {
        return Some((
            j.ahead_asked,
            j.written
                .range(first..last)
                .map(|(&p, &w)| (p, w))
                .collect(),
        ));
    }
    let mut count = (last - first) * PAGE / KERNEL_PAGE;
    j.found.resize(count.max(j.found.len()), 0);
    let mut granularity = 0;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    // SAFETY: the answer buffer holds `count` entries.
    let failed = unsafe {
        GetWriteWatch(
            WRITE_WATCH_FLAG_RESET,
            (first * PAGE) as *const _,
            (last - first) * PAGE,
            j.found.as_mut_ptr().cast(),
            &mut count,
            &mut granularity,
        )
    } != 0;
    if failed {
        drop(held);
        return if guarded { guarded_writes(first, last, None) } else { None };
    }
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::Watches, 1);
        add(C::WatchUs, started.elapsed().as_micros() as u64);
        slow(started.elapsed().as_micros() as u64);
    }
    j.asked += 1;
    let now = j.asked;
    for k in 0..count {
        let page = j.found[k] / PAGE;
        j.written.insert(page, now);
    }
    Some((
        now,
        j.written
            .range(first..last)
            .map(|(&p, &w)| (p, w))
            .collect(),
    ))
}

/// Whether pages `[first, last)` were asked ahead ([`ask_ahead`]).
fn journal_covers(first: usize, last: usize) -> bool {
    let j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    j.ahead.iter().any(|&(a, b)| a <= first && last <= b)
}

/// Whether the block at `start` is guarded, or noted absent: view memory.
fn known_view(g: &Guards, start: usize) -> bool {
    let first = start / BLOCK;
    g.protected.contains(first) || g.absent.contains(first)
}

/// [`writes`] of view memory, pages `[first, last)`. The guards are asked
/// before the journal is taken: protecting blocks is slow, and every other
/// refresh waited behind it for an answer the journal had. They are held
/// until the journal has what they said, so that nobody finds a block clean
/// whose write is not noted yet. (Guards, then journal: no one takes them
/// the other way round.)
///
/// `known`: the guards, held, if they say it is a view ([`known_view`]).
fn guarded_writes(
    first: usize,
    last: usize,
    known: Option<std::sync::MutexGuard<'static, Guards>>,
) -> Option<(u64, Vec<(usize, u64)>)> {
    #[cfg(feature = "diagnostics")]
    let guarding = std::time::Instant::now();
    let (written, held) = guard(first * PAGE, last * PAGE, known)?;
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::GuardUs, guarding.elapsed().as_micros() as u64);
        add(C::Watches, 1);
        add(C::WatchUs, guarding.elapsed().as_micros() as u64);
    }
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    j.asked += 1;
    let now = j.asked;
    for page in written {
        j.written.insert(page, now);
    }
    drop(held);
    Some((now, j.written.range(first..last).map(|(&p, &w)| (p, w)).collect()))
}

/// What the guards' answers are made of, since the last frame line
/// (diagnostics): answers; blocks in their ranges; answers for wholly clean
/// ranges; then microseconds of all threads, waiting for the guards' lock,
/// asking Windows whether it is view memory, reading off the clean blocks,
/// protecting blocks, looking up and protecting aliases, asking for absent
/// blocks; and runs protected, blocks in them, aliases protected, askings
/// for absent blocks; blocks not clean; and of blocks opened: microseconds,
/// blocks, times.
#[cfg(feature = "diagnostics")]
pub(crate) static GUARD_PARTS: [std::sync::atomic::AtomicU64; 17] = [const { std::sync::atomic::AtomicU64::new(0) }; 17];

#[cfg(feature = "diagnostics")]
fn part(i: usize, n: u64) {
    GUARD_PARTS[i].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// For the frame line: [`GUARD_PARTS`], taken.
#[cfg(feature = "diagnostics")]
pub(crate) fn log_guard_parts(frames: f64) {
    let p = GUARD_PARTS.each_ref().map(|p| p.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / frames);
    tracing::info!(
        "guards per frame, all threads: {:.0} answers over {:.0} blocks, {:.0} of them wholly clean; {:.1} ms waiting for the lock, \
         {:.1} ms asking whether it is view memory, {:.1} ms reading off the clean blocks, {:.1} ms protecting {:.0} runs of {:.0} \
         blocks, {:.1} ms for their aliases ({:.0} protected), {:.1} ms in {:.0} askings for absent blocks; {:.0} blocks were not clean; \
         {:.0} blocks opened in {:.0} times, {:.1} ms with the lock",
        p[0],
        p[1],
        p[2],
        p[3] / 1e3,
        p[4] / 1e3,
        p[5] / 1e3,
        p[6] / 1e3,
        p[9],
        p[10],
        p[7] / 1e3,
        p[11],
        p[8] / 1e3,
        p[12],
        p[13],
        p[15],
        p[16],
        p[14] / 1e3
    );
}

/// Guarded blocks of view memory.
const BLOCK: usize = PAGE;

/// A set of blocks, a bit each, in chunks of 4,096 blocks (256 MiB). A
/// frame asks about some 110,000 blocks, nearly all of them clean: read off
/// a word at a time.
struct BlockSet(BTreeMap<usize, Box<[u64; 64]>>);

impl BlockSet {
    const fn new() -> Self {
        BlockSet(BTreeMap::new())
    }

    fn contains(&self, block: usize) -> bool {
        self.0.get(&(block >> 12)).is_some_and(|c| c[block >> 6 & 63] >> (block & 63) & 1 != 0)
    }

    /// Whether it was not in the set.
    fn insert(&mut self, block: usize) -> bool {
        let word = &mut self.0.entry(block >> 12).or_insert_with(|| Box::new([0; 64]))[block >> 6 & 63];
        let was = *word;
        *word |= 1 << (block & 63);
        was != *word
    }

    /// Whether it was in the set.
    fn remove(&mut self, block: usize) -> bool {
        let Some(chunk) = self.0.get_mut(&(block >> 12)) else {
            return false;
        };
        let word = &mut chunk[block >> 6 & 63];
        let was = *word;
        *word &= !(1 << (block & 63));
        was != *word
    }

    fn extend(&mut self, blocks: std::ops::Range<usize>) {
        for block in blocks {
            self.insert(block);
        }
    }

    /// The blocks of `[first, last)` in the set, in order.
    fn range(&self, first: usize, last: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if first >= last {
            return out;
        }
        for (&chunk, words) in self.0.range(first >> 12..=(last - 1) >> 12) {
            let base = chunk << 12;
            let (from, to) = (first.max(base) - base, last.min(base + 4096) - base);
            for (w, &word) in words.iter().enumerate().take(to.div_ceil(64)).skip(from / 64) {
                push_bits(word & word_mask(w, from, to), base + w * 64, &mut out);
            }
        }
        out
    }

    /// The blocks of `[first, last)` not in the set, in order.
    fn missing(&self, first: usize, last: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = first;
        while at < last {
            let base = at >> 12 << 12;
            let (from, to) = (at - base, last.min(base + 4096) - base);
            match self.0.get(&(at >> 12)) {
                None => out.extend(base + from..base + to),
                Some(words) => {
                    for (w, &word) in words.iter().enumerate().take(to.div_ceil(64)).skip(from / 64) {
                        push_bits(!word & word_mask(w, from, to), base + w * 64, &mut out);
                    }
                }
            }
            at = base + to;
        }
        out
    }
}

/// The bits of word `w` of a chunk that lie in its blocks `[from, to)`.
fn word_mask(w: usize, from: usize, to: usize) -> u64 {
    let below = |n: usize| if n >= 64 { u64::MAX } else { (1 << n) - 1 };
    below(to.saturating_sub(w * 64)) & !below(from.saturating_sub(w * 64))
}

fn push_bits(mut word: u64, base: usize, out: &mut Vec<usize>) {
    while word != 0 {
        out.push(base + word.trailing_zeros() as usize);
        word &= word - 1;
    }
}

/// Blocks in order, as runs of blocks that follow each other.
fn block_runs(blocks: &[usize]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &block in blocks {
        match runs.last_mut() {
            Some(run) if run.1 == block => run.1 += 1,
            _ => runs.push((block, block + 1)),
        }
    }
    runs
}

struct Guards {
    /// Blocks we made read-only.
    protected: BlockSet,
    /// Of those, the ones not written since a copy took them.
    clean: BlockSet,
    /// Blocks already copied as zeros. While they remain wholly uncommitted,
    /// the copy can skip hundreds of MB of failing kernel reads per world frame.
    absent: BlockSet,
    /// Blocks whose aliases (other views of the same pages) we made
    /// read-only, so a write here opens them.
    aliased: BlockSet,
}

static GUARDS: Mutex<Guards> = Mutex::new(Guards {
    protected: BlockSet::new(),
    clean: BlockSet::new(),
    absent: BlockSet::new(),
    aliased: BlockSet::new(),
});

fn guards() -> std::sync::MutexGuard<'static, Guards> {
    GUARDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runtime's answer to "which other addresses show the physical page
/// mapped at this page address" (the same page in several views).
static ALIAS_LOOKUP: std::sync::OnceLock<fn(usize, &mut Vec<usize>)> = std::sync::OnceLock::new();

/// The runtime tells where else view memory's physical pages show: a write
/// through any of those views is a write to all of them, so guarding a
/// block guards its aliases too, and a write to one opens them all (else a
/// copy of one view keeps what it first took while the title writes the
/// page through another).
pub fn set_alias_lookup(lookup: fn(usize, &mut Vec<usize>)) {
    let _ = ALIAS_LOOKUP.set(lookup);
}

/// The blocks showing the same physical memory as `block` (not itself).
fn aliases_of(block: usize, out: &mut Vec<usize>) {
    out.clear();
    if let Some(lookup) = ALIAS_LOOKUP.get() {
        lookup(block * BLOCK, out);
        for at in out.iter_mut() {
            *at /= BLOCK;
        }
        out.retain(|&b| b != block);
    }
}

/// For view memory: the pages of `[start, end)`'s blocks written since
/// they were last guarded (all of a block's pages), guarding them again
/// before the caller copies them. `None` if the memory is not a view.
/// The guards come back held with the answer. `known`: the guards, held, if
/// they say it is a view ([`known_view`]).
fn guard(
    start: usize,
    end: usize,
    known: Option<std::sync::MutexGuard<'static, Guards>>,
) -> Option<(Vec<usize>, std::sync::MutexGuard<'static, Guards>)> {
    // A range whose first block the guards hold already (read-only by our
    // hand, or noted absent) is a view: no question to Windows. The
    // question walks the page tables to the end of the pages that share the
    // first one's state, and a guarded view is one long run of read-only
    // pages: 0.6 ms for the title's 174 MB heap, 16 times a frame, and most
    // of the 68 ms a frame of "guards" (run 58).
    // `VICTORY_GUARD_ASK=1` asks every time, as before.
    static ASK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ask = *ASK.get_or_init(|| std::env::var_os("VICTORY_GUARD_ASK").is_some_and(|v| v == "1"));
    #[cfg(feature = "diagnostics")]
    let asking = std::time::Instant::now();
    let known = known.filter(|_| !ask);
    if known.is_none() && !is_view(start, end) {
        return None;
    }
    install();
    #[cfg(feature = "diagnostics")]
    let waiting = std::time::Instant::now();
    let mut g = known.unwrap_or_else(guards);
    #[cfg(feature = "diagnostics")]
    {
        part(4, (waiting - asking).as_micros() as u64);
        part(3, waiting.elapsed().as_micros() as u64);
    }
    let (written, clean) = guard_held(&mut g, start / BLOCK, end.div_ceil(BLOCK));
    WHOLLY_CLEAN.set(clean);
    Some((written, g))
}

/// [`guard`] of blocks `[first, last)` with the guards held: the pages
/// written, and whether the range was wholly clean.
fn guard_held(g: &mut Guards, first: usize, last: usize) -> (Vec<usize>, bool) {
    #[cfg(feature = "diagnostics")]
    let reading = std::time::Instant::now();
    let mut written = Vec::new();
    // The range's blocks that are not clean, in order: read off the set in
    // one pass, not looked up one block at a time. A frame asks about some
    // 10 GB of view memory, 150,000 blocks, nearly all clean: the lookups
    // were most of the 60-90 ms a frame of "guards" (run 54).
    let unclean = g.clean.missing(first, last);
    #[cfg(feature = "diagnostics")]
    {
        part(0, 1);
        part(1, (last - first) as u64);
        part(5, reading.elapsed().as_micros() as u64);
        part(2, u64::from(unclean.is_empty()));
        part(13, unclean.len() as u64);
    }
    if unclean.is_empty() {
        // Nothing written, mapped or new since the range was last taken.
        return (written, true);
    }
    let mut uncommitted_until = first;
    let mut to_protect = Vec::new();
    for block in unclean {
        if g.absent.contains(block) {
            // Ordinary commits need no runtime mapping notification. Reuse
            // this asking's query for the following blocks in the same region.
            if block >= uncommitted_until {
                #[cfg(feature = "diagnostics")]
                let asking = std::time::Instant::now();
                uncommitted_until = uncommitted_end(block);
                #[cfg(feature = "diagnostics")]
                {
                    part(8, asking.elapsed().as_micros() as u64);
                    part(12, 1);
                }
            }
            if block < uncommitted_until {
                continue;
            }
            g.absent.remove(block);
        }
        written.extend(block * BLOCK / PAGE..(block + 1) * BLOCK / PAGE);
        if g.protected.contains(block) {
            // Written through the runtime, still read-only.
            g.clean.insert(block);
        } else {
            to_protect.push(block);
        }
    }
    for (from, to) in block_runs(&to_protect) {
        protect(g, from, to);
    }
    (written, false)
}

/// What [`writes`] says of a range, answered ahead ([`answer_ahead`]).
struct Answer {
    now: u64,
    written: Vec<(usize, u64)>,
    clean: bool,
    /// The stamp of the range when it was answered: a write since then is
    /// not in the answer, and must not be in the stamp it is kept under.
    stamp: Option<u64>,
}

/// By first and last page, until [`answered`].
static ANSWERS: std::sync::RwLock<BTreeMap<(usize, usize), Answer>> = std::sync::RwLock::new(BTreeMap::new());

/// Whether a flush's refreshes are answered for at once, before they are
/// spread over the threads that copy (`VICTORY_GUARD_BATCH=1`).
///
/// Every copying thread asked the guards for its own range, one at a time
/// behind one lock, and the thread that submits waited its turns with them:
/// 12% of its time (run AP1, eight threads). Asked together the lock is
/// taken once, and the threads copy.
pub(crate) fn batch() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_GUARD_BATCH").is_some_and(|v| v == "1"))
}

/// Answers [`writes`] for `ranges` (of view memory; others are left to it)
/// until [`answered`]. One asking for all: what one range found written is
/// in the journal for the others.
pub(crate) fn answer_ahead(ranges: &[(usize, usize)]) {
    let mut pages: Vec<(usize, usize)> = ranges
        .iter()
        .filter(|r| r.0 < r.1)
        .map(|&(a, b)| (a / PAGE, b.div_ceil(PAGE)))
        .collect();
    pages.sort_unstable();
    pages.dedup();
    {
        let j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
        pages.retain(|&(first, last)| !j.ahead.iter().any(|&(a, b)| a <= first && last <= b));
    }
    #[cfg(feature = "diagnostics")]
    let guarding = std::time::Instant::now();
    let mut held = guards();
    pages.retain(|&(first, _)| known_view(&held, first * PAGE));
    if pages.len() < 2 {
        return;
    }
    install();
    let found: Vec<(Vec<usize>, bool, Option<u64>)> = pages
        .iter()
        .map(|&(first, last)| {
            let stamp = stamp(first * PAGE, last * PAGE);
            let (written, clean) = guard_held(&mut held, first, last);
            (written, clean, stamp)
        })
        .collect();
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::GuardUs, guarding.elapsed().as_micros() as u64);
        add(C::Watches, pages.len() as u64);
        add(C::WatchUs, guarding.elapsed().as_micros() as u64);
    }
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    j.asked += 1;
    let now = j.asked;
    for page in found.iter().flat_map(|f| &f.0) {
        j.written.insert(*page, now);
    }
    // (Held until the journal has what they said: see `guarded_writes`.)
    drop(held);
    let mut answers = ANSWERS.write().unwrap_or_else(|e| e.into_inner());
    for (&(first, last), (_, clean, stamp)) in pages.iter().zip(&found) {
        let written = j.written.range(first..last).map(|(&p, &w)| (p, w)).collect();
        answers.insert((first, last), Answer { now, written, clean: *clean, stamp: *stamp });
    }
}

/// The flush is done: its answers are gone.
pub(crate) fn answered() {
    ANSWERS.write().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Whether `[start, end)` is view memory. The title's views lie in
/// placeholders, mapped and unmapped block by block as it streams, so a
/// range can begin in a block that is not mapped right now: the first
/// pages of the range that are there decide. A range with nothing there
/// counts as a view (its blocks are noted absent, and looked at again when
/// the runtime maps them).
///
/// Looking at the first address only took such a range for "not a view":
/// its copy was marked not watched for good and copied whole at every
/// refresh, around its unmapped pages (run 54: 300 such ranges, 570 MB
/// copied a frame for them, 255 copies a frame reading around holes).
/// `VICTORY_GUARD_HOLES=0` looks at the first address only, as before.
fn is_view(start: usize, end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    static HOLES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let holes = *HOLES.get_or_init(|| std::env::var_os("VICTORY_GUARD_HOLES").is_none_or(|v| v != "0"));
    let mut at = start;
    loop {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
        if n == 0 {
            return false;
        }
        if info.Type == MEM_MAPPED {
            return true;
        }
        // Committed memory that is no view (the title's heaps, ours), or
        // the answer as it was before.
        if info.State == MEM_COMMIT || !holes {
            return false;
        }
        // A hole (a placeholder, or free): on to what follows it.
        let next = (info.BaseAddress as usize).saturating_add(info.RegionSize);
        if next <= at || next >= end {
            return true;
        }
        at = next;
    }
}

/// Makes blocks `[from, to)` read-only and notes them guarded and clean;
/// block by block if they span views, leaving out any that fail, and
/// noting those not committed (unmapped) absent.
fn protect(g: &mut Guards, from: usize, to: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_READONLY, VirtualProtect};
    let set = |from: usize, to: usize| {
        let mut old = 0;
        // SAFETY: title view memory; only its protection changes.
        unsafe {
            VirtualProtect(
                (from * BLOCK) as *const _,
                (to - from) * BLOCK,
                PAGE_READONLY,
                &mut old,
            ) != 0
        }
    };
    let note = |g: &mut Guards, from: usize, to: usize| {
        g.protected.extend(from..to);
        g.clean.extend(from..to);
    };
    #[cfg(feature = "diagnostics")]
    let protecting = std::time::Instant::now();
    if set(from, to) {
        note(g, from, to);
    } else {
        for block in from..to {
            if set(block, block + 1) {
                note(g, block, block + 1);
            } else if block < uncommitted_end(block) {
                g.absent.insert(block);
            }
        }
    }
    #[cfg(feature = "diagnostics")]
    {
        part(6, protecting.elapsed().as_micros() as u64);
        part(9, 1);
        part(10, (to - from) as u64);
    }
    #[cfg(feature = "diagnostics")]
    let aliasing = std::time::Instant::now();
    // Their aliases read-only too, so a write through them faults; not
    // clean, though: a copy of an alias has not taken them yet.
    let mut others = Vec::new();
    let mut unguarded = Vec::new();
    for block in from..to {
        aliases_of(block, &mut others);
        for &other in &others {
            if !g.protected.contains(other) {
                unguarded.push(other);
            }
            // Both ways: a write through either opens the other.
            g.aliased.insert(block);
            g.aliased.insert(other);
        }
    }
    unguarded.sort_unstable();
    unguarded.dedup();
    // Pages that follow each other in one view often do in the others.
    for (a, b) in block_runs(&unguarded) {
        #[cfg(feature = "diagnostics")]
        part(11, (b - a) as u64);
        if set(a, b) {
            g.protected.extend(a..b);
        } else {
            for other in a..b {
                if set(other, other + 1) {
                    g.protected.insert(other);
                }
            }
        }
    }
    #[cfg(feature = "diagnostics")]
    part(7, aliasing.elapsed().as_micros() as u64);
}

/// The end (exclusive) of the whole uncommitted blocks starting at `block`.
fn uncommitted_end(block: usize) -> usize {
    use windows_sys::Win32::System::Memory::{
        MEM_FREE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery((block * BLOCK) as *const _, &mut info, size_of_val(&info)) };
    if n != 0 && matches!(info.State, MEM_RESERVE | MEM_FREE) {
        (info.BaseAddress as usize).saturating_add(info.RegionSize) / BLOCK
    } else {
        block
    }
}

/// The runtime's way to write title view memory through a view of its own:
/// calls its last argument with where `[at, at + len)` shows there.
type Behind = fn(usize, usize, &mut dyn FnMut(*mut u8) -> bool) -> Option<bool>;

static BEHIND: std::sync::OnceLock<Behind> = std::sync::OnceLock::new();

pub fn set_behind(behind: Behind) {
    let _ = BEHIND.set(behind);
}

/// Whether what we write into title view memory goes through views of our
/// own, beside the guards (`VICTORY_WRITE_BEHIND=1`).
///
/// A guarded block was opened for such a write, with every view of its
/// pages, and protected again at the next refresh: 6,600 blocks opened and
/// 6,400 protected a frame, 30 ms of all threads with the guards' lock
/// held, nearly all of it for what shaders wrote coming back and the
/// title's copies made at submission. Behind the guards the blocks stay
/// read-only and only count as written.
pub fn writes_behind() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WRITE_BEHIND").is_some_and(|v| v == "1"))
}

/// Since the last frame line (diagnostics): writes behind the guards, and
/// those that could not be made there.
#[cfg(feature = "diagnostics")]
pub(crate) static PUTS: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Writes title memory `[at, at + len)` behind the guards: `write` is
/// called with where to. False if it has to be written at its own address
/// ([`writing`] first; some of it may have been written).
///
/// The blocks count as written after the write, not before: a copy that
/// took a block in between would have found it clean with the old bytes,
/// and nothing faults to say so afterwards.
pub(crate) fn put(at: usize, len: usize, mut write: impl FnMut(*mut u8) -> bool) -> bool {
    if !writes_behind() || len == 0 {
        return false;
    }
    let done = BEHIND.get().and_then(|&behind| put_with(behind, at, len, &mut write));
    #[cfg(feature = "diagnostics")]
    PUTS[usize::from(done != Some(true))].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    done == Some(true)
}

/// [`put`] by `behind`: as that, and the blocks count as written.
fn put_with(behind: Behind, at: usize, len: usize, write: &mut dyn FnMut(*mut u8) -> bool) -> Option<bool> {
    let done = behind(at, len, write);
    if done.is_some() {
        let (first, last) = (at / BLOCK, at.saturating_add(len).div_ceil(BLOCK));
        let mut g = guards();
        for block in g.clean.range(first, last) {
            g.unclean(block);
        }
        let mut others = Vec::new();
        for block in g.aliased.range(first, last) {
            aliases_of(block, &mut others);
            for &other in &others {
                g.unclean(other);
            }
        }
        drop(g);
        crate::descriptor::host::remapped(at, len);
    }
    done
}

/// A number per 16 MiB of addresses below 256 GiB, which rises whenever a
/// clean block there stops being clean ([`Guards::unclean`]).
static STAMPS: [std::sync::atomic::AtomicU64; 1 << 14] = [const { std::sync::atomic::AtomicU64::new(0) }; 1 << 14];
const STAMPED: usize = 16 << 20;

/// The sum of the stamps of `[start, end)`: while it stays the same, blocks
/// that were clean are. `None` past what is stamped, or switched off
/// (`VICTORY_CLEAN_STAMPS=1` is on, `check` asks all the same and counts).
pub(crate) fn stamp(start: usize, end: usize) -> Option<u64> {
    if clean_stamps() == 0 || start >= end || (end - 1) / STAMPED >= STAMPS.len() {
        return None;
    }
    Some(
        (start / STAMPED..=(end - 1) / STAMPED)
            .fold(0u64, |sum, i| sum.wrapping_add(STAMPS[i].load(std::sync::atomic::Ordering::Acquire))),
    )
}

/// 0: no stamps. 1: a range of view memory that was wholly clean when last
/// asked about, and whose stamps are the same, is not asked about again.
/// 2: asked about all the same; what the stamps got wrong is counted.
///
/// Some 740 of a frame's 1,650 answers of the guards are for wholly clean
/// ranges, each behind the guards' lock.
pub(crate) fn clean_stamps() -> u8 {
    static ON: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("VICTORY_CLEAN_STAMPS").as_deref() {
        Ok("1") => 1,
        Ok("check") => 2,
        _ => 0,
    })
}

thread_local! {
    /// Whether this thread's last [`writes`] found the range wholly clean,
    /// by the guards.
    static WHOLLY_CLEAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// If it was answered ahead: the stamp from then.
    static ANSWER_STAMP: std::cell::Cell<Option<Option<u64>>> = const { std::cell::Cell::new(None) };
}

/// [`ANSWER_STAMP`].
pub(crate) fn answer_stamp() -> Option<Option<u64>> {
    ANSWER_STAMP.get()
}

/// [`WHOLLY_CLEAN`].
pub(crate) fn wholly_clean() -> bool {
    WHOLLY_CLEAN.get()
}

impl Guards {
    /// `block` is not clean any more: a copy must take it.
    fn unclean(&mut self, block: usize) {
        if self.clean.remove(block)
            && let Some(stamp) = STAMPS.get(block / (STAMPED / BLOCK))
        {
            stamp.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
}

/// Gives write access back to the guarded blocks of `[start, end)` and
/// notes them written; true if any was guarded.
fn open(g: &mut Guards, start: usize, end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    #[cfg(feature = "diagnostics")]
    let opening = std::time::Instant::now();
    let (first, last) = (start / BLOCK, end.div_ceil(BLOCK));
    let blocks = g.protected.range(first, last);
    let mut guarded = Vec::new();
    let mut unguard = |g: &mut Guards, block: usize| {
        g.unclean(block);
        if g.protected.remove(block) {
            guarded.push(block);
        }
    };
    // A write here is a write to every view of the same pages. Only blocks
    // with guarded aliases are looked up: a write range can be huge.
    let aliased = g.aliased.range(first, last);
    let mut others = Vec::new();
    for block in aliased {
        g.aliased.remove(block);
        aliases_of(block, &mut others);
        for &other in &others {
            unguard(g, other);
        }
    }
    for &block in &blocks {
        unguard(g, block);
    }
    guarded.sort_unstable();
    let set = |from: usize, to: usize| {
        let mut old = 0;
        // SAFETY: blocks we made read-only.
        unsafe { VirtualProtect((from * BLOCK) as *const _, (to - from) * BLOCK, PAGE_READWRITE, &mut old) != 0 }
    };
    for (from, to) in block_runs(&guarded) {
        // A run over two views is not one call's.
        if !set(from, to) {
            for block in from..to {
                set(block, block + 1);
            }
        }
    }
    #[cfg(feature = "diagnostics")]
    {
        part(14, opening.elapsed().as_micros() as u64);
        part(15, guarded.len() as u64);
        part(16, 1);
    }
    !blocks.is_empty()
}

/// Before our own write into title memory `[at, at + len)` through the
/// kernel (which does not fault into our handler): opens its guarded blocks.
/// The runtime calls it (as `kernel_will_write`) before the title's kernel
/// reads into its own memory.
pub fn writing(at: usize, len: usize) {
    // The length is the title's (`fread`'s size times count, saturated).
    open(&mut guards(), at, at.saturating_add(len));
    // A placed texture there changes with no descriptor written.
    crate::descriptor::host::remapped(at, len);
}

/// Held shared by a read of title memory in place, and alone by the runtime
/// while it changes what title memory shows.
///
/// The runtime does not change a mapping in one step. To unmap one page of
/// a view it unmaps the whole view and maps what stays again
/// (`xmem.rs` `unmap`), and a view it maps is a reservation until the next
/// call commits it (`map`, then `commit`). For that moment pages the title
/// never gave up are "reserved, never committed", and a thread reading
/// them in place takes an access violation: run 71 in `Layout::detile`,
/// run 74 in the hash before it, both under `Staging::refresh_now`, which
/// had asked whether the texture was all mapped and been told yes. Asking
/// cannot help; not changing the mapping while such a read is under way
/// does.
///
/// Only with `VICTORY_MAPPING_RANGES=0` ([`by_range`]); otherwise reads and
/// changes hold the bytes they touch ([`SPANS`]).
static MAPPING: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Whether a read in place and a change of mapping keep apart only when
/// their bytes meet ([`SPANS`]), not whatever memory each touches
/// ([`MAPPING`]); `VICTORY_MAPPING_RANGES=0` holds all of title memory for
/// each, as before. On by default.
///
/// With one lock over all of title memory each texture refresh waited for
/// every change of mapping anywhere, and each change for the refresh under
/// way. While the world streams in the title changes mappings 650-1,100
/// times a frame, many a page at a time (`XMemMapPhysicalPagesScatter`
/// maps each page on its own). With the one lock the refreshes waited 203
/// ms of an 801 ms frame for title memory to hold still; holding only the
/// bytes touched, 0 ms of a 486 ms frame (medians, a pair of runs).
pub(crate) fn by_range() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MAPPING_RANGES").is_none_or(|v| v != "0"))
}

/// Reads of title memory in place under way, and changes of its mapping,
/// by the bytes they touch ([`by_range`]).
struct Spans {
    /// To tell them apart.
    next: u64,
    /// Reads under way: number, start and end.
    reads: Vec<(u64, usize, usize)>,
    /// Changes: number, start, end, and whether under way (else waiting for
    /// the reads over its bytes to end; no read begins over them meanwhile,
    /// so that one read after another cannot keep a change waiting).
    changes: Vec<(u64, usize, usize, bool)>,
    /// Threads waiting on [`SPANS_LEFT`].
    waiting: usize,
}

static SPANS: Mutex<Spans> = Mutex::new(Spans {
    next: 0,
    reads: Vec::new(),
    changes: Vec::new(),
    waiting: 0,
});

/// Told when a read or a change ends while a thread waits.
static SPANS_LEFT: std::sync::Condvar = std::sync::Condvar::new();

fn spans() -> std::sync::MutexGuard<'static, Spans> {
    SPANS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Waits until a read or a change ends.
fn spans_wait(mut s: std::sync::MutexGuard<'static, Spans>) -> std::sync::MutexGuard<'static, Spans> {
    s.waiting += 1;
    let mut s = SPANS_LEFT.wait(s).unwrap_or_else(|e| e.into_inner());
    s.waiting -= 1;
    s
}

/// Whether `[a, b)` and `[c, d)` share a byte.
fn meet(a: usize, b: usize, c: usize, d: usize) -> bool {
    a < d && c < b
}

impl Spans {
    fn read(&mut self, at: usize, end: usize) -> Span {
        self.next += 1;
        self.reads.push((self.next, at, end));
        Span(self.next, false)
    }
}

/// A read's or a change's place in [`SPANS`] (its number, and whether a
/// change), given up when dropped.
struct Span(u64, bool);

impl Drop for Span {
    fn drop(&mut self) {
        let mut s = spans();
        if self.1 {
            s.changes.retain(|c| c.0 != self.0);
        } else if let Some(i) = s.reads.iter().position(|r| r.0 == self.0) {
            s.reads.swap_remove(i);
        }
        if s.waiting > 0 {
            SPANS_LEFT.notify_all();
        }
    }
}

/// What a read or a change of mapping holds: its bytes ([`by_range`]) or
/// the lock over all of title memory.
enum Held<G> {
    All(#[allow(dead_code)] G),
    Bytes(#[allow(dead_code)] Span),
}

/// Since the last frame-rate line: changes of title memory's mapping, the
/// microseconds they waited for reads in place, and those they were under
/// way.
pub(crate) static REMAPS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// What the runtime holds while it changes what title memory shows
/// ([`title_memory_remapping`]).
pub struct Remapping {
    _held: Held<std::sync::RwLockWriteGuard<'static, ()>>,
    since: std::time::Instant,
}

impl Drop for Remapping {
    fn drop(&mut self) {
        REMAPS[2].fetch_add(self.since.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The runtime is about to map, unmap or move physical pages of title
/// memory `[at, at + len)`: until what it returns is dropped, no read of
/// those bytes in place is under way and none begins
/// ([`title_memory_reading`]). Taken around the whole change, the unmapping
/// and the mapping again, over all the bytes whose mapping it changes.
pub fn title_memory_remapping(at: usize, len: usize) -> Remapping {
    use std::sync::atomic::Ordering::Relaxed;
    let started = std::time::Instant::now();
    let held = if by_range() {
        let end = at.saturating_add(len);
        let mut s = spans();
        s.next += 1;
        let id = s.next;
        s.changes.push((id, at, end, false));
        while s.reads.iter().any(|r| meet(r.1, r.2, at, end))
            || s.changes.iter().any(|c| c.3 && meet(c.1, c.2, at, end))
        {
            s = spans_wait(s);
        }
        if let Some(c) = s.changes.iter_mut().find(|c| c.0 == id) {
            c.3 = true;
        }
        Held::Bytes(Span(id, true))
    } else {
        Held::All(MAPPING.write().unwrap_or_else(|e| e.into_inner()))
    };
    REMAPS[0].fetch_add(1, Relaxed);
    REMAPS[1].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
    Remapping {
        _held: held,
        since: std::time::Instant::now(),
    }
}

/// What a read of title memory in place holds ([`title_memory_reading`]).
pub struct Reading(#[allow(dead_code)] Held<std::sync::RwLockReadGuard<'static, ()>>);

/// For reading title memory `[at, at + len)` in place: until what it
/// returns is dropped, the runtime changes no mapping of those bytes, so
/// memory found mapped stays mapped. Not to be taken twice by one thread,
/// nor held across anything that maps title memory.
pub fn title_memory_reading(at: usize, len: usize) -> Reading {
    if !by_range() {
        return Reading(Held::All(MAPPING.read().unwrap_or_else(|e| e.into_inner())));
    }
    let end = at.saturating_add(len);
    let mut s = spans();
    while s.changes.iter().any(|c| meet(c.1, c.2, at, end)) {
        s = spans_wait(s);
    }
    Reading(Held::Bytes(s.read(at, end)))
}

/// [`title_memory_reading`] if it can be had at once: `None` while the
/// runtime changes a mapping of those bytes (or waits to), for a caller
/// that has another way to read and would rather not wait.
pub fn title_memory_reading_now(at: usize, len: usize) -> Option<Reading> {
    if !by_range() {
        return match MAPPING.try_read() {
            Ok(guard) => Some(Reading(Held::All(guard))),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(Reading(Held::All(e.into_inner()))),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
    }
    let end = at.saturating_add(len);
    let mut s = spans();
    if s.changes.iter().any(|c| meet(c.1, c.2, at, end)) {
        return None;
    }
    Some(Reading(Held::Bytes(s.read(at, end))))
}

/// The runtime mapped or unmapped title memory `[at, at + len)`: what it
/// shows is new and read-write, so it is no longer guarded (nor known
/// unmapped).
pub fn title_memory_remapped(at: usize, len: usize) {
    // A placed texture there arrives, goes or changes with no descriptor
    // written.
    crate::descriptor::host::remapped(at, len);
    // The views the runtime told us of there are no longer what it shows.
    crate::inplace::views_gone(at, len);
    // And nothing stays in place over it.
    crate::inplace::title_memory_freeing(at, len);
    let mut g = guards();
    let range = at / BLOCK..at.saturating_add(len).div_ceil(BLOCK);
    for block in g.absent.range(range.start, range.end) {
        g.absent.remove(block);
    }
    for block in g.protected.range(range.start, range.end) {
        g.protected.remove(block);
        g.unclean(block);
    }
    for block in g.aliased.range(range.start, range.end) {
        g.aliased.remove(block);
    }
}

/// Registers the handler that takes writes to guarded blocks, once, ahead
/// of the crash reporter's.
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_write(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        const WRITE: usize = 1;
        if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS
            || r.NumberParameters < 2
            || r.ExceptionInformation[0] != WRITE
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        if write_faulted(at) {
            EXCEPTION_CONTINUE_EXECUTION
        } else {
            EXCEPTION_CONTINUE_SEARCH
        }
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the handler only touches our guard state and protections.
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_write));
    });
}

/// Since the last frame line (diagnostics): how often each block was opened
/// by a fault.
#[cfg(feature = "diagnostics")]
static FAULTED: Mutex<BTreeMap<usize, u32>> = Mutex::new(BTreeMap::new());

/// For the frame line: the faults, the blocks they were in, those in blocks
/// that fault in most frames, and the blocks that do.
#[cfg(feature = "diagnostics")]
pub(crate) fn log_faulted(frames: f64) {
    let faulted = std::mem::take(&mut *FAULTED.lock().unwrap_or_else(|e| e.into_inner()));
    let hot: Vec<u32> = faulted.values().copied().filter(|&n| f64::from(n) >= frames * 0.75).collect();
    tracing::info!(
        "write faults into guarded blocks per frame: {:.0}, in {} blocks; {} of the blocks fault in most frames and take {:.0} of them",
        faulted.values().map(|&n| f64::from(n)).sum::<f64>() / frames,
        faulted.len(),
        hot.len(),
        hot.iter().map(|&n| f64::from(n)).sum::<f64>() / frames
    );
}

/// A write to `at` faulted: whether it may be tried again (the block was
/// guarded and is open now, or is writable after all).
pub(crate) fn write_faulted(at: usize) -> bool {
    // Held across both checks: another thread may have opened the block
    // since this one faulted, and a copy may have guarded it again since
    // (then it is ours again, not read-only for some other reason).
    let mut g = guards();
    if open(&mut g, at, at + 1) {
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::GuardFaults, 1);
        #[cfg(feature = "diagnostics")]
        {
            *FAULTED.lock().unwrap_or_else(|e| e.into_inner()).entry(at / BLOCK).or_default() += 1;
        }
        return true;
    }
    writable(at)
}

/// Whether `at` is committed read-write memory now.
fn writable(at: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    n != 0 && info.State == MEM_COMMIT && info.Protect == PAGE_READWRITE
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;

    #[test]
    fn written_pages_are_reported_to_every_asker() {
        // SAFETY: a fresh write-watched allocation, freed at the end.
        let p = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4 * PAGE,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        } as usize;
        assert_ne!(p, 0);
        let (start, end) = (p, p + 4 * PAGE);
        let (_, before) = writes(start, end, false).expect("watched");
        assert!(before.is_empty());
        // SAFETY: committed pages of ours.
        unsafe { ((p + PAGE + 8) as *mut u8).write(1) };
        let (now, after) = writes(start, end, false).expect("watched");
        assert_eq!(after, [(p / PAGE + 1, now)]);
        // Another asker after the kernel's bit was cleared still sees it.
        let (later, again) = writes(p + PAGE, p + 2 * PAGE, false).expect("watched");
        assert!(later > now);
        assert_eq!(again, [(p / PAGE + 1, now)]);
        let mut plain = vec![0u8; PAGE];
        assert!(
            writes(
                plain.as_mut_ptr() as usize,
                plain.as_ptr() as usize + PAGE,
                true
            )
            .is_none()
        );
        // SAFETY: our allocation.
        unsafe { VirtualFree(p as *mut _, 0, MEM_RELEASE) };
    }

    /// A kernel read into a guarded view block fails; opened first, it
    /// lands and the block counts as written.
    #[test]
    fn kernel_reads_into_guarded_blocks_after_opening() {
        use std::io::{Read, Seek, Write};
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, UnmapViewOfFile,
        };
        // SAFETY: a pagefile section of two blocks, mapped read-write.
        let view = unsafe {
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (2 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 2 * BLOCK).Value as usize
        };
        assert_ne!(view, 0);
        assert!(guard(view, view + 2 * BLOCK, None).map(|(w, _)| w).is_some()); // now read-only
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"from the disk").unwrap();
        // `ReadFile` inside: the kernel writes the view.
        let read = |file: &mut std::fs::File| {
            file.rewind().unwrap();
            // SAFETY: 13 bytes of our view, nothing else refers to them.
            let into = unsafe { std::slice::from_raw_parts_mut((view + 100) as *mut u8, 13) };
            file.read(into).is_ok_and(|n| n == 13)
        };
        assert!(
            !read(&mut file),
            "the kernel does not fault into our handler"
        );
        writing(view + 100, 13);
        assert!(read(&mut file));
        // SAFETY: the bytes just read.
        let landed = unsafe { std::slice::from_raw_parts((view + 100) as *const u8, 13) };
        assert_eq!(landed, b"from the disk");
        assert_eq!(
            guard(view, view + 2 * BLOCK, None).map(|(w, _)| w),
            Some((view / PAGE..view / PAGE + 1).collect())
        );
        // SAFETY: our view.
        unsafe {
            UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view as *mut _,
                },
            )
        };
        title_memory_remapped(view, 2 * BLOCK);
    }

    #[test]
    fn writes_in_partly_committed_view_blocks_are_reported() {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            SEC_RESERVE, UnmapViewOfFile,
        };

        const SIZE: usize = 3 * BLOCK;
        struct View(MEMORY_MAPPED_VIEW_ADDRESS);
        impl Drop for View {
            fn drop(&mut self) {
                title_memory_remapped(self.0.Value as usize, SIZE);
                // SAFETY: our view is still mapped.
                unsafe { UnmapViewOfFile(self.0) };
            }
        }
        // SAFETY: a new reserved pagefile section. The view holds its own
        // reference after we close the handle, including on assertion failure.
        let view = unsafe {
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE | SEC_RESERVE,
                0,
                SIZE as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            let view = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, SIZE);
            CloseHandle(section);
            assert!(!view.Value.is_null());
            View(view)
        };
        let start = view.0.Value as usize;
        let commit = |at: usize, len| {
            // SAFETY: these pages belong to our reserved view.
            let committed = unsafe { VirtualAlloc(at as _, len, MEM_COMMIT, PAGE_READWRITE) };
            assert_eq!(committed as usize, at);
        };
        let early = start + BLOCK + 3 * KERNEL_PAGE;
        let late = start + 2 * BLOCK + 3 * KERNEL_PAGE;
        commit(start, BLOCK);
        commit(early, KERNEL_PAGE);
        let refresh = || guard(start, start + SIZE, None).map(|(w, _)| w).unwrap();
        assert_eq!(refresh(), [start / PAGE, early / PAGE, late / PAGE]);
        // The last block is still entirely reserved.
        assert!(!refresh().contains(&(late / PAGE)));

        commit(late, KERNEL_PAGE); // Ordinary commit, without a runtime mapping notification.
        for at in [early, late] {
            assert!(writable(at));
            // SAFETY: each address is inside its committed read-write page.
            unsafe { (at as *mut u8).write_volatile(1) };
        }
        assert_eq!(refresh(), [early / PAGE, late / PAGE]);
    }

    /// A range of view memory whose first block is not mapped is a view
    /// all the same: its blocks are reported once, then the ones written
    /// or mapped since.
    #[test]
    fn a_range_that_begins_in_an_unmapped_block_is_guarded() {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MEM_PRESERVE_PLACEHOLDER, MEM_REPLACE_PLACEHOLDER,
            MEM_RESERVE_PLACEHOLDER, MapViewOfFile3, PAGE_NOACCESS, UnmapViewOfFile2,
            VirtualAlloc2,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: a fresh placeholder of three blocks and a pagefile
        // section of three; the middle block is mapped first.
        let (p, section) = unsafe {
            let p = VirtualAlloc2(
                GetCurrentProcess(),
                std::ptr::null(),
                3 * BLOCK,
                MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                PAGE_NOACCESS,
                std::ptr::null_mut(),
                0,
            ) as usize;
            assert_ne!(p, 0);
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (3 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            (p, section)
        };
        let map = |block: usize| {
            // SAFETY: splits the placeholder at the block and maps the
            // section's block there.
            let view = unsafe {
                VirtualFree(
                    (p + block * BLOCK) as *mut _,
                    BLOCK,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                );
                MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    (p + block * BLOCK) as *const _,
                    (block * BLOCK) as u64,
                    BLOCK,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(view.Value as usize, p + block * BLOCK);
            view
        };
        let pages = |block: usize| (p + block * BLOCK) / PAGE..(p + (block + 1) * BLOCK) / PAGE;
        let all: Vec<usize> = (0..3).flat_map(pages).collect();
        // Nothing mapped at all: a view with every block absent.
        assert!(is_view(p, p + 3 * BLOCK));
        let middle = map(1);
        title_memory_remapped(p + BLOCK, BLOCK);
        // The range begins in the placeholder, its middle block is a view.
        assert!(is_view(p, p + 3 * BLOCK));
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(all));
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(Vec::new()), "nothing written since");
        // SAFETY: inside the mapped middle block.
        unsafe { ((p + BLOCK + 8) as *mut u8).write_volatile(1) };
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(pages(1).collect()));
        let first = map(0);
        title_memory_remapped(p, BLOCK);
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(pages(0).collect()));
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(Vec::new()));
        // Memory of ours that is no view stays none.
        let mut local = [0u8; 16];
        let at = local.as_mut_ptr() as usize;
        assert!(!is_view(at, at + local.len()));
        // SAFETY: our views and placeholder.
        unsafe {
            UnmapViewOfFile2(GetCurrentProcess(), first, MEM_PRESERVE_PLACEHOLDER);
            UnmapViewOfFile2(GetCurrentProcess(), middle, MEM_PRESERVE_PLACEHOLDER);
            for block in 0..3 {
                VirtualFree((p + block * BLOCK) as *mut _, 0, MEM_RELEASE);
            }
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
        title_memory_remapped(p, 3 * BLOCK);
    }

    /// Blocks of a placeholder between views are reported once, then not
    /// again (they cannot be guarded) until the runtime maps them.
    #[test]
    fn unmapped_blocks_are_reported_once_until_mapped() {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MEM_PRESERVE_PLACEHOLDER, MEM_REPLACE_PLACEHOLDER,
            MEM_RESERVE_PLACEHOLDER, MapViewOfFile3, PAGE_NOACCESS, UnmapViewOfFile2,
            VirtualAlloc2,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: a fresh placeholder of three blocks and a pagefile
        // section of two; block 0 shows the section's first block, as the
        // runtime maps physical pages.
        let (p, section) = unsafe {
            let p = VirtualAlloc2(
                GetCurrentProcess(),
                std::ptr::null(),
                3 * BLOCK,
                MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                PAGE_NOACCESS,
                std::ptr::null_mut(),
                0,
            ) as usize;
            assert_ne!(p, 0);
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (2 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            (p, section)
        };
        let map = |block: usize| {
            // SAFETY: splits the placeholder at the block and maps the
            // section's block there.
            let view = unsafe {
                VirtualFree(
                    (p + block * BLOCK) as *mut _,
                    BLOCK,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                );
                MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    (p + block * BLOCK) as *const _,
                    (block * BLOCK) as u64,
                    BLOCK,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(view.Value as usize, p + block * BLOCK);
            view
        };
        let first = map(0);
        let pages = |block: usize| (p + block * BLOCK) / PAGE..(p + (block + 1) * BLOCK) / PAGE;
        let all: Vec<usize> = (0..3).flat_map(pages).collect();
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(all));
        assert_eq!(
            guard(p, p + 3 * BLOCK, None).map(|(w, _)| w),
            Some(Vec::new()),
            "nothing written since"
        );
        let second = map(1);
        title_memory_remapped(p + BLOCK, BLOCK);
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(pages(1).collect()));
        assert_eq!(guard(p, p + 3 * BLOCK, None).map(|(w, _)| w), Some(Vec::new()));
        // SAFETY: our views and placeholder.
        unsafe {
            UnmapViewOfFile2(GetCurrentProcess(), first, MEM_PRESERVE_PLACEHOLDER);
            UnmapViewOfFile2(GetCurrentProcess(), second, MEM_PRESERVE_PLACEHOLDER);
            for block in 0..3 {
                VirtualFree((p + block * BLOCK) as *mut _, 0, MEM_RELEASE);
            }
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
        title_memory_remapped(p, 3 * BLOCK);
    }

    /// The sets of blocks answer as ordered sets do, over chunk borders too.
    #[test]
    fn block_sets_answer_as_ordered_sets() {
        use std::collections::BTreeSet;
        let (mut set, mut model) = (super::BlockSet::new(), BTreeSet::new());
        let mut seed = 12345u64;
        let mut next = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for _ in 0..20_000 {
            let block = 4096 * 5 - 300 + next(5000) as usize;
            match next(4) {
                0 => assert_eq!(set.remove(block), model.remove(&block)),
                1 => {
                    let to = block + next(200) as usize;
                    set.extend(block..to);
                    model.extend(block..to);
                }
                _ => assert_eq!(set.insert(block), model.insert(block)),
            }
            assert_eq!(set.contains(block), model.contains(&block));
            let (a, b) = (4096 * 4 + next(12_000) as usize, 4096 * 4 + next(12_000) as usize);
            if next(50) == 0 {
                let have: Vec<usize> = if a < b { model.range(a..b).copied().collect() } else { Vec::new() };
                assert_eq!(set.range(a, b), have);
                let lack: Vec<usize> = (a..b).filter(|x| !model.contains(x)).collect();
                assert_eq!(set.missing(a, b), lack);
                let runs = super::block_runs(&have);
                assert_eq!(runs.iter().flat_map(|r| r.0..r.1).collect::<Vec<_>>(), have);
                assert!(runs.windows(2).all(|w| w[0].1 < w[1].0));
            }
        }
        assert!(set.range(7, 7).is_empty() && set.missing(9, 7).is_empty());
        assert!(set.range(0, usize::MAX / 65536).len() == model.len());
    }

    /// A write or mapping that runs past the end of the address space (a
    /// title's `fread` with a huge count) covers up to the end; the sum used
    /// to overflow. Up there no test has guarded blocks.
    #[test]
    fn ranges_past_the_address_space_end_do_not_overflow() {
        let top = usize::MAX - 16;
        writing(top, PAGE);
        title_memory_remapped(top, usize::MAX);
    }

    #[test]
    fn a_write_through_an_alias_view_is_seen_by_a_guard_on_the_other() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            UnmapViewOfFile,
        };
        static A: AtomicUsize = AtomicUsize::new(0);
        static B: AtomicUsize = AtomicUsize::new(0);
        fn lookup(at: usize, out: &mut Vec<usize>) {
            let (a, b) = (A.load(Ordering::Relaxed), B.load(Ordering::Relaxed));
            let page = at - at % BLOCK;
            if page == a {
                out.push(b);
            } else if page == b {
                out.push(a);
            }
        }
        // SAFETY: one pagefile section of a block, mapped twice read-write.
        let (a, b, c) = unsafe {
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                BLOCK as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            (
                MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, BLOCK).Value as usize,
                MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, BLOCK).Value as usize,
                MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, BLOCK).Value as usize,
            )
        };
        assert!(a != 0 && b != 0 && a % BLOCK == 0 && b % BLOCK == 0);
        A.store(a, Ordering::Relaxed);
        B.store(b, Ordering::Relaxed);
        set_alias_lookup(lookup);
        // A copy of view `a` takes the block and guards it (and `b`).
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![a / PAGE]));
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![]), "clean");
        // The title writes the page through `b`: `a` shows it written.
        // SAFETY: our view; the guard's handler opens it.
        unsafe { ((b + 8) as *mut u8).write_volatile(7) };
        // SAFETY: the same byte through the other view.
        assert_eq!(unsafe { ((a + 8) as *const u8).read_volatile() }, 7);
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![a / PAGE]), "written through the alias");
        // And the other way round: a kernel write into `a` opens `b`.
        assert_eq!(guard(b, b + BLOCK, None).map(|(w, _)| w), Some(vec![b / PAGE]));
        assert_eq!(guard(b, b + BLOCK, None).map(|(w, _)| w), Some(vec![]));
        writing(a + 16, 4);
        assert_eq!(guard(b, b + BLOCK, None).map(|(w, _)| w), Some(vec![b / PAGE]));
        // A write behind the guards, through a third view: both count as
        // written, and both stay read-only (the next write faults).
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![a / PAGE]));
        assert_eq!(guard(b, b + BLOCK, None).map(|(w, _)| w), Some(vec![]));
        static C: AtomicUsize = AtomicUsize::new(0);
        fn behind(at: usize, len: usize, write: &mut dyn FnMut(*mut u8) -> bool) -> Option<bool> {
            let a = A.load(Ordering::Relaxed);
            (at >= a && at + len <= a + BLOCK).then(|| write((C.load(Ordering::Relaxed) + at - a) as *mut u8))
        }
        C.store(c, Ordering::Relaxed);
        let mut store = |to: *mut u8| {
            // SAFETY: the third view, read-write.
            unsafe { to.write_volatile(42) };
            true
        };
        assert_eq!(put_with(behind, a + 100, 1, &mut store), Some(true));
        // SAFETY: the same byte through the first view.
        assert_eq!(unsafe { ((a + 100) as *const u8).read_volatile() }, 42);
        {
            let g = guards();
            assert!(g.protected.contains(a / BLOCK) && g.protected.contains(b / BLOCK));
        }
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![a / PAGE]));
        assert_eq!(guard(b, b + BLOCK, None).map(|(w, _)| w), Some(vec![b / PAGE]));
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![]));
        assert_eq!(put_with(behind, b, 1, &mut store), None, "not shown there");
        // SAFETY: our view; the guard's handler opens it.
        unsafe { ((b + 8) as *mut u8).write_volatile(9) };
        assert_eq!(guard(a, a + BLOCK, None).map(|(w, _)| w), Some(vec![a / PAGE]));
        A.store(0, Ordering::Relaxed);
        B.store(0, Ordering::Relaxed);
        for v in [a, b] {
            // SAFETY: our views.
            unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: v as *mut _ }) };
            title_memory_remapped(v, BLOCK);
        }
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::{PAGE, by_range, title_memory_reading, title_memory_remapping};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// Addresses only these tests hold (nothing is read there).
    const HERE: usize = 0x7e00_0000_0000;

    /// A change of mapping waits for a read in place of its bytes that is
    /// under way, and such a read waits for the change.
    #[test]
    fn reads_in_place_and_remappings_wait_for_each_other() {
        // A reader holds; the change does not begin until it lets go.
        let changed = Arc::new(AtomicBool::new(false));
        let reading = title_memory_reading(HERE + PAGE / 2, PAGE);
        let (began, wait) = mpsc::channel();
        let change = {
            let changed = changed.clone();
            std::thread::spawn(move || {
                began.send(()).unwrap();
                let _changing = title_memory_remapping(HERE + PAGE, PAGE);
                changed.store(true, Ordering::SeqCst);
            })
        };
        wait.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!changed.load(Ordering::SeqCst), "the mapping changed under a read in place");
        drop(reading);
        change.join().unwrap();
        assert!(changed.load(Ordering::SeqCst));
        // A change holds; the read does not begin until it is done.
        let read = Arc::new(AtomicBool::new(false));
        let changing = title_memory_remapping(HERE, 2 * PAGE);
        let (began, wait) = mpsc::channel();
        let reader = {
            let read = read.clone();
            std::thread::spawn(move || {
                began.send(()).unwrap();
                let _reading = title_memory_reading(HERE + PAGE + 16, 16);
                read.store(true, Ordering::SeqCst);
            })
        };
        wait.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!read.load(Ordering::SeqCst), "a read in place began during a change of mapping");
        drop(changing);
        reader.join().unwrap();
        assert!(read.load(Ordering::SeqCst));
    }

    /// Reads and changes whose bytes do not meet do not wait for each other,
    /// and a change waiting for a read keeps new reads of its bytes out (but
    /// not of others).
    #[test]
    fn reads_and_remappings_of_other_bytes_go_on() {
        if !by_range() {
            return; // VICTORY_MAPPING_RANGES=0: one lock over all of it
        }
        let at = HERE + 16 * PAGE;
        let reading = title_memory_reading(at, PAGE);
        // Another page changes while the read is under way, and another
        // read goes on while that change is.
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let changing = title_memory_remapping(at + PAGE, PAGE);
            let (read, was_read) = mpsc::channel();
            std::thread::spawn(move || {
                let _reading = title_memory_reading(at + 2 * PAGE, PAGE);
                read.send(()).unwrap();
            });
            let other = was_read.recv_timeout(Duration::from_secs(10)).is_ok();
            drop(changing);
            done.send(other).unwrap();
        });
        assert_eq!(finished.recv_timeout(Duration::from_secs(10)), Ok(true), "kept apart though their bytes do not meet");
        // A change of the read's page waits, and keeps a new read of its
        // bytes out meanwhile; a read elsewhere is not kept out.
        let changed = Arc::new(AtomicBool::new(false));
        let change = {
            let changed = changed.clone();
            std::thread::spawn(move || {
                let _changing = title_memory_remapping(at, PAGE);
                changed.store(true, Ordering::SeqCst);
            })
        };
        while !super::spans().changes.iter().any(|c| c.1 == at) {
            std::thread::yield_now();
        }
        assert!(super::title_memory_reading_now(at + 8, 8).is_none(), "a read began over a change that waits");
        assert!(super::title_memory_reading_now(at + 3 * PAGE, 8).is_some());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!changed.load(Ordering::SeqCst), "the mapping changed under a read in place");
        drop(reading);
        change.join().unwrap();
        assert!(changed.load(Ordering::SeqCst));
    }
}
