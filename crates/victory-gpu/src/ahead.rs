//! Textures picked by index, made ready on threads of their own.
//!
//! The pass over the view heap (`draw.rs` `heap_textures`) runs on a thread
//! that records the frame. For each texture it made the host texture and
//! its staging buffer and recorded the copy, and the submitting thread then
//! hashed and detiled the title's memory: some 0.9 ms a texture, 20,000 of
//! them when the world opens (1.5 s of every frame until they are through)
//! and 1,000 at a turn of the camera (a frame of 300-700 ms).
//!
//! With `VICTORY_TEXTURE_THREADS=n` the pass hands a slot's texture to `n`
//! threads and leaves the slot due; they make the host texture and the
//! staging buffer and detile into it. The next pass that finds it ready
//! records the copy, which is all that is left. The refresh at submission
//! looks at the memory again, as ever, so what the title wrote meanwhile is
//! not lost.

use crate::object::This;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

/// A placed texture: where, and the resource description's words.
pub(crate) type Key = (u64, [u32; 14]);

/// What a texture made ready holds until its copy is recorded.
type Held = (Arc<crate::resource::HostTexture>, Arc<crate::texture::Staging>);

enum State {
    /// A thread has it, or will.
    Waiting,
    /// Ready since a frame; what it holds (nothing, if there was nothing to
    /// make), and whether the staging buffer is there because of this.
    Ready { frame: u64, held: Option<Held>, ours: bool, bytes: u64 },
    /// Its copy was recorded in this frame.
    Used(u64),
}

#[derive(Default)]
struct Work {
    jobs: VecDeque<Key>,
    states: HashMap<Key, State>,
}

static WORK: OnceLock<Mutex<Work>> = OnceLock::new();
static MORE: Condvar = Condvar::new();

/// Staging bytes of textures made ready whose copy is not recorded yet, and
/// since the last frame line: textures made ready and the threads'
/// microseconds.
static BYTES: AtomicU64 = AtomicU64::new(0);
static MADE: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// The most textures with the threads at once, and the most staging bytes
/// waiting for their copy.
const MOST_JOBS: usize = 2048;
const MOST_BYTES: u64 = 512 << 20;
/// Frames a texture stays ready unasked (its slot was written over, say).
const KEPT: u64 = 120;

fn lock() -> MutexGuard<'static, Work> {
    WORK.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// How many threads make textures ready (`VICTORY_TEXTURE_THREADS`; unset
/// or 0: none, the pass does it).
fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        let n = std::env::var("VICTORY_TEXTURE_THREADS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
            .min(16);
        for i in 0..n {
            let _ = std::thread::Builder::new().name(format!("victory-texture-{i}")).spawn(work);
        }
        n
    })
}

pub(crate) fn on() -> bool {
    threads() != 0
}

fn work() {
    // Behind the title's threads and ours that a frame waits for.
    // SAFETY: our own thread.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    loop {
        let key = {
            let mut work = lock();
            loop {
                if let Some(key) = work.jobs.pop_front() {
                    break key;
                }
                work = MORE.wait(work).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = std::time::Instant::now();
        let made = prepare(&key);
        MADE[0].fetch_add(1, Ordering::Relaxed);
        MADE[1].fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        let (held, ours) = match made {
            Some((held, ours)) => (Some(held), ours),
            None => (None, false),
        };
        let bytes = held.as_ref().map_or(0, |h| h.1.bytes());
        BYTES.fetch_add(bytes, Ordering::Relaxed);
        lock().states.insert(key, State::Ready { frame: crate::frame::current(), held, ours, bytes });
    }
}

/// Makes a texture ready; `None` if there is nothing to make (no memory
/// there, the host refuses it, or its contents are the host's own).
fn prepare(key: &Key) -> Option<(Held, bool)> {
    let len = crate::resource::placed_bytes(&key.1).unwrap_or(1);
    if crate::texture::mapped_bytes(key.0 as *const u8, len as usize) == 0 {
        return None;
    }
    // (A placed texture's resource is ours and is never released.)
    let resource: This = crate::resource::texture_at(key.0, &key.1)?;
    let texture = crate::resource::host_copy(resource)?;
    let (staging, made) = crate::texture::make_staging(resource, &texture.texture)?;
    staging.refresh_ahead();
    Some(((texture, staging), made))
}

/// What the pass is to do with a slot of the texture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Answer {
    /// Pass it by: it is being made ready.
    Wait,
    /// Go on: it is ready (or there is nothing to make ready), and the
    /// staging buffer is there because of it.
    Go { ours: bool },
}

/// Whether a thread has the texture now.
pub(crate) fn waiting(key: &Key) -> bool {
    matches!(lock().states.get(key), Some(State::Waiting))
}

/// Asks for the texture to be made ready.
pub(crate) fn ask(key: &Key) -> Answer {
    let mut work = lock();
    match work.states.get(key) {
        Some(State::Waiting) => Answer::Wait,
        Some(State::Ready { ours, .. }) => Answer::Go { ours: *ours },
        Some(State::Used(_)) => Answer::Go { ours: true },
        None => {
            if work.jobs.len() < MOST_JOBS && BYTES.load(Ordering::Relaxed) < MOST_BYTES {
                work.states.insert(*key, State::Waiting);
                work.jobs.push_back(*key);
                MORE.notify_one();
            }
            Answer::Wait
        }
    }
}

/// The pass is done with the texture in this frame: other slots of it in
/// the frame find it so.
pub(crate) fn used(key: &Key) {
    let was = lock().states.insert(*key, State::Used(crate::frame::current()));
    let_go(was);
}

fn let_go(state: Option<State>) {
    if let Some(State::Ready { bytes, held, .. }) = state {
        BYTES.fetch_sub(bytes, Ordering::Relaxed);
        drop(held);
    }
}

/// At a frame's first pass: what earlier frames used, and what nothing
/// asked for any more, is forgotten.
pub(crate) fn sweep(frame: u64) {
    // Let go of outside the lock: freeing a buffer takes the host a while.
    let mut gone = Vec::new();
    {
        let mut work = lock();
        let old: Vec<Key> = work
            .states
            .iter()
            .filter(|(_, state)| match state {
                State::Waiting => false,
                State::Used(at) => *at != frame,
                State::Ready { frame: at, .. } => frame > at + KEPT,
            })
            .map(|(key, _)| *key)
            .collect();
        for key in old {
            gone.extend(work.states.remove(&key).map(|state| (key, state)));
        }
    }
    for (key, state) in gone {
        // Staged for this alone, and never uploaded.
        if let State::Ready { ours: true, held: Some(_), .. } = &state
            && let Some(resource) = crate::resource::texture_at(key.0, &key.1)
        {
            crate::texture::unstage(resource);
        }
        let_go(Some(state));
    }
}

/// For the frame line.
#[cfg(feature = "diagnostics")]
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let [made, us] = MADE.each_ref().map(|n| n.swap(0, Ordering::Relaxed) as f64 / frames);
    let (jobs, states) = {
        let work = lock();
        (work.jobs.len(), work.states.len())
    };
    tracing::info!(
        "textures made ready ahead per frame (VICTORY_TEXTURE_THREADS={}): {made:.0} in {:.1} ms of all threads; {jobs} waiting for a thread, \
         {states} with them or ready, {} MB of staging ready",
        threads(),
        us / 1e3,
        BYTES.load(Ordering::Relaxed) >> 20
    );
}
