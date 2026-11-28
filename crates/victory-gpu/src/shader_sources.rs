//! (Diagnostics) Where the title's shader code lives
//! (`VICTORY_PIPELINE_SHADERS=<file>`; off without it): each shader
//! container a pipeline is made from, once per title address, with its stage,
//! length and content hash (the one logged at a pipeline's first bind).
//!
//! The other side is in `victory_loader::title_names`, which dumps each
//! effect the title loads by name (`<VICTORY_TITLE_NAMES>.effects.txt`).
//! Matching the two from a run tells how an effect's programs lead to the code
//! its pipelines are made from, so that pipelines, and the passes RGP shows,
//! can carry the effect's name.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Containers by title address: stage, length, hash, and the effect the
/// title was loading on that thread when the pipeline was made, if any.
static SEEN: Mutex<Option<HashMap<usize, (&'static str, usize, String, Option<std::sync::Arc<str>>)>>> = Mutex::new(None);

thread_local! {
    /// The effect the title is loading by name on this thread
    /// (`victory_loader::title_names` tells, around the loader's call).
    static LOADING: std::cell::RefCell<Option<std::sync::Arc<str>>> = const { std::cell::RefCell::new(None) };
}

/// The title starts (`Some`) or ends (`None`) loading an effect by name on
/// this thread.
pub fn effect_loading(name: Option<&str>) {
    LOADING.with(|l| *l.borrow_mut() = name.map(std::sync::Arc::from));
}
static DIRTY: AtomicBool = AtomicBool::new(false);

/// The file, once asked for; the writer starts with the first note.
fn file() -> Option<&'static std::path::PathBuf> {
    static FILE: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let file = std::env::var_os("VICTORY_PIPELINE_SHADERS").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)?;
        let at = file.clone();
        let _ = std::thread::Builder::new().name("pipeline-shaders".into()).spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                if DIRTY.swap(false, Ordering::Relaxed) {
                    write(&at);
                }
            }
        });
        Some(file)
    })
    .as_ref()
}

/// A pipeline is made from `len` bytes of `stage` code at title address `at`.
pub(crate) fn note(stage: &'static str, at: usize, len: usize, hash: &str) {
    if file().is_none() {
        return;
    }
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(at) {
        let effect = LOADING.with(|l| l.borrow().clone());
        e.insert((stage, len, hash.to_owned(), effect));
        DIRTY.store(true, Ordering::Relaxed);
    }
}

fn write(file: &std::path::Path) {
    use std::fmt::Write as _;
    let mut text = String::new();
    {
        let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(seen) = seen.as_ref() else { return };
        let mut all: Vec<_> = seen.iter().collect();
        all.sort_by_key(|(at, _)| **at);
        let _ = writeln!(
            text,
            "# VICTORY_PIPELINE_SHADERS: {} containers. shader <stage> <title address> <bytes> <hash> <effect being loaded on the thread, or ->",
            all.len()
        );
        for (at, (stage, len, hash, effect)) in all {
            let _ = writeln!(text, "shader {stage} {at:#x} {len} {hash} {}", effect.as_deref().unwrap_or("-"));
        }
    }
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}
