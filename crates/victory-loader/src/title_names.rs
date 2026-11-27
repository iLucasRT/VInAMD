//! What the title asks for by name (`VICTORY_TITLE_NAMES=<file>`; diagnostics
//! builds, off without it): every visual tunable it looks up, with the value
//! it got, and every shader effect it loads.
//!
//! What the title does (game_scarlett_master.exe 1.0.2.1, image offsets):
//!
//! - +0x501aca0 looks a visual tunable up. Its name's hash comes in `ecx`
//!   and a default in `xmm1` (four bytes: a float, or an integer's bits);
//!   the value goes out in `xmm0`. The values come from
//!   `commoncrc:/data/visualsettings`, which +0x577c7b0 loads from
//!   `common.rpf`. There are 1,754 calls in the code; +0x15eaf50, for
//!   example, takes the screen shadows' sample counts and ray lengths per
//!   quality level from it.
//! - +0x1adcc20 loads a shader effect by name: a C string in `rcx`, flags in
//!   `edx`, the effect out. There are 195 calls, for example
//!   `rtdirectionalshadows`, `sr_tracing`, `sr_denoising`,
//!   `indirect_light_probes` and `bvh_skinning`. The effect's programs are
//!   at +0xc80 of it, and their count at +0xcf8.
//!
//! How it is hooked:
//! - Each function's first instructions are position independent (pushes, a
//!   `sub rsp`, a `mov` between registers). They are compared byte for byte
//!   first, then replaced by a jump to a stub near the title's code.
//! - The stub does `mov r8, [rsp]` and jumps to our hook. That puts the
//!   caller's return address in a third argument, which neither function
//!   has, and `r8` is volatile.
//! - The hook calls the original through a trampoline (the moved
//!   instructions, then a jump back), notes what it saw and returns the
//!   original's result untouched.
//!
//! The file is rewritten every 5 s when something new came in. It has one
//! line per tunable (hash, default, value, calls, how many values it had,
//! first caller) and one per effect (name, address, programs, loads, first
//! caller), callers as image offsets.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The tunable lookup: offset, original first instructions.
pub const TUNABLE: (usize, &[u8]) = (
    0x501aca0,
    &[
        0x56, // push rsi
        0x48, 0x83, 0xec, 0x30, // sub rsp, 0x30
        0x89, 0xce, // mov esi, ecx
    ],
);

/// The effect loader: offset, original first instructions.
pub const EFFECT: (usize, &[u8]) = (
    0x1adcc20,
    &[
        0x56, 0x57, 0x55, 0x53, // push rsi ; push rdi ; push rbp ; push rbx
        0x48, 0x81, 0xec, 0x38, 0x01, 0x00, 0x00, // sub rsp, 0x138
    ],
);

/// Where an effect keeps its programs' count.
const EFFECT_PROGRAMS: usize = 0xcf8;

type TunableFn = unsafe extern "system" fn(u32, f32) -> f32;
type EffectFn = unsafe extern "system" fn(*const c_char, u32) -> usize;

static TUNABLE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static EFFECT_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The image's base, for callers as offsets.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// Something came in since the file was written.
static DIRTY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tunable {
    default: u32,
    value: u32,
    calls: u64,
    /// How many times the value differed from the one before it.
    changed: u32,
    caller: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Effect {
    address: usize,
    programs: u16,
    loads: u64,
    caller: usize,
}

#[derive(Default)]
struct Seen {
    tunables: HashMap<u32, Tunable>,
    effects: HashMap<String, Effect>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

fn note_tunable(hash: u32, default: u32, value: u32, caller: usize) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Seen::default);
    let base = BASE.load(Ordering::Relaxed);
    let e = seen.tunables.entry(hash).or_insert_with(|| {
        DIRTY.store(true, Ordering::Relaxed);
        Tunable { default, value, calls: 0, changed: 0, caller: caller.wrapping_sub(base) }
    });
    e.calls += 1;
    if e.value != value {
        e.value = value;
        e.changed += 1;
        DIRTY.store(true, Ordering::Relaxed);
    }
}

fn note_effect(name: String, address: usize, programs: u16, caller: usize) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Seen::default);
    let base = BASE.load(Ordering::Relaxed);
    let e = seen.effects.entry(name).or_insert_with(|| Effect { address, programs, loads: 0, caller: caller.wrapping_sub(base) });
    e.loads += 1;
    if e.address != address || e.programs != programs {
        e.address = address;
        e.programs = programs;
    }
    DIRTY.store(true, Ordering::Relaxed);
}

/// The hook of the tunable lookup (reached from its stub, `ret` in `r8`).
unsafe extern "system" fn tunable(hash: u32, default: f32, ret: usize) -> f32 {
    // SAFETY: set to the trampoline before the jump to this hook is written.
    let original: TunableFn = unsafe { std::mem::transmute::<usize, TunableFn>(TUNABLE_ORIGINAL.load(Ordering::Acquire)) };
    // SAFETY: the title's own call, passed on as it came.
    let value = unsafe { original(hash, default) };
    note_tunable(hash, default.to_bits(), value.to_bits(), ret);
    value
}

/// The hook of the effect loader (reached from its stub, `ret` in `r8`).
unsafe extern "system" fn effect(name: *const c_char, flags: u32, ret: usize) -> usize {
    // SAFETY: set to the trampoline before the jump to this hook is written.
    let original: EffectFn = unsafe { std::mem::transmute::<usize, EffectFn>(EFFECT_ORIGINAL.load(Ordering::Acquire)) };
    // SAFETY: the title's own call, passed on as it came.
    let address = unsafe { original(name, flags) };
    let text = if name.is_null() {
        String::from("(null)")
    } else {
        // SAFETY: the title passes a C string (it hashes it the same way).
        unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned()
    };
    let programs = if address == 0 {
        0
    } else {
        // SAFETY: the effect the loader returned; the title reads this field too.
        unsafe { ((address + EFFECT_PROGRAMS) as *const u16).read_unaligned() }
    };
    note_effect(text, address, programs, ret);
    address
}

/// The stub near the title's code at `stub`: the caller's return address
/// into `r8`, then an absolute jump to `hook`.
fn stub_code(hook: usize) -> Vec<u8> {
    let mut code = vec![0x4c, 0x8b, 0x04, 0x24]; // mov r8, [rsp]
    code.extend([0xff, 0x25, 0, 0, 0, 0]); // jmp [rip+0]
    code.extend((hook as u64).to_le_bytes());
    code
}

/// The trampoline at `at`: the moved instructions, then a jump back to the
/// instruction after them at `resume`.
fn trampoline_code(moved: &[u8], at: usize, resume: usize) -> Option<Vec<u8>> {
    let mut code = moved.to_vec();
    code.extend(crate::console_insn::jump(at + moved.len(), resume)?);
    Some(code)
}

/// Hooks the function at `at`, whose first instructions are `expected`, to
/// `hook`; the trampoline's address goes to `original`. Whether it did.
///
/// # Safety
/// `at` is the start of a function in code that stays mapped, no thread runs
/// it yet, and `expected` are whole position-independent instructions of at
/// least 5 bytes.
unsafe fn hook_function(at: usize, expected: &[u8], hook: usize, original: &AtomicUsize) -> Result<(), &'static str> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    if expected.len() < 5 {
        return Err("fewer than 5 bytes to move");
    }
    // SAFETY: the caller's word that `at` is mapped code of that length.
    let there = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if there != expected {
        return Err("its bytes are not the ones expected (another build?)");
    }
    let place = crate::console_insn::near(at).ok_or("no room for code near the title's")?;
    let stub = stub_code(hook);
    let tramp_at = place + stub.len().next_multiple_of(16);
    let tramp = trampoline_code(expected, tramp_at, at + expected.len()).ok_or("out of reach")?;
    let jump = crate::console_insn::jump(at, place).ok_or("out of reach")?;
    let mut old = 0;
    // SAFETY: our own fresh code memory; then the title's code, which nothing
    // runs yet, made writable for the jump and put back.
    unsafe {
        std::ptr::copy_nonoverlapping(stub.as_ptr(), place as *mut u8, stub.len());
        std::ptr::copy_nonoverlapping(tramp.as_ptr(), tramp_at as *mut u8, tramp.len());
        original.store(tramp_at, Ordering::Release);
        if VirtualProtect(at as *const _, expected.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return Err("Windows would not make it writable");
        }
        std::ptr::write_bytes(at as *mut u8, 0x90, expected.len());
        std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
        VirtualProtect(at as *const _, expected.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const _, expected.len());
    }
    Ok(())
}

/// The file's text: tunables by hash, then effects by name.
fn report(seen: &Seen) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# VICTORY_TITLE_NAMES: {} tunables, {} effects. tunable <hash> <default bits> <default as float> <value bits> <value as float> <calls> <times changed> <first caller>; effect <name> <address> <programs> <loads> <first caller>",
        seen.tunables.len(),
        seen.effects.len()
    );
    let mut t: Vec<_> = seen.tunables.iter().collect();
    t.sort_by_key(|(h, _)| **h);
    for (h, e) in t {
        let _ = writeln!(
            out,
            "tunable {h:#010x} {:#010x} {} {:#010x} {} {} {} +{:#x}",
            e.default,
            f32::from_bits(e.default),
            e.value,
            f32::from_bits(e.value),
            e.calls,
            e.changed,
            e.caller
        );
    }
    let mut f: Vec<_> = seen.effects.iter().collect();
    f.sort_by(|a, b| a.0.cmp(b.0));
    for (n, e) in f {
        let _ = writeln!(out, "effect {n} {:#x} {} {} +{:#x}", e.address, e.programs, e.loads, e.caller);
    }
    out
}

/// Writes the file now (`file.tmp`, then renamed over it).
fn write(file: &std::path::Path) {
    let text = {
        let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        match seen.as_ref() {
            Some(s) => report(s),
            None => return,
        }
    };
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

/// Hooks both functions in the image of `size` bytes at `base` when
/// `VICTORY_TITLE_NAMES` names a file, and starts the thread that writes it.
///
/// # Safety
/// The image stays mapped and nothing of it has run yet.
pub unsafe fn apply_asked(base: usize, size: usize) {
    let Some(file) = std::env::var_os("VICTORY_TITLE_NAMES").filter(|v| !v.is_empty()).map(std::path::PathBuf::from) else {
        return;
    };
    BASE.store(base, Ordering::Relaxed);
    let mut hooked = 0;
    for (what, (offset, bytes), hook, original) in [
        ("tunable lookup", TUNABLE, tunable as *const () as usize, &TUNABLE_ORIGINAL),
        ("effect loader", EFFECT, effect as *const () as usize, &EFFECT_ORIGINAL),
    ] {
        if offset + bytes.len() > size {
            tracing::warn!(what, "title names: not hooked, outside the image");
            continue;
        }
        // SAFETY: inside the mapped image, which nothing runs yet; the
        // bytes are checked to be the instructions named above.
        match unsafe { hook_function(base + offset, bytes, hook, original) } {
            Ok(()) => {
                hooked += 1;
                tracing::info!(what, at = format_args!("+{offset:#x}"), "title names: hooked (VICTORY_TITLE_NAMES)");
            }
            Err(why) => tracing::warn!(what, at = format_args!("+{offset:#x}"), why, "title names: not hooked"),
        }
    }
    println!("title names: {hooked} of 2 functions hooked, written to {}", file.display());
    if hooked == 0 {
        return;
    }
    let _ = std::thread::Builder::new().name("title-names".into()).spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if DIRTY.swap(false, Ordering::Relaxed) {
                write(&file);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function in fresh code memory near `near_to`: `prologue`, then
    /// `body`; its address.
    fn make(prologue: &[u8], body: &[u8]) -> usize {
        let place = crate::console_insn::near(make as *const () as usize).expect("code memory");
        // SAFETY: fresh code memory of ours, large enough.
        unsafe {
            std::ptr::copy_nonoverlapping(prologue.as_ptr(), place as *mut u8, prologue.len());
            std::ptr::copy_nonoverlapping(body.as_ptr(), (place + prologue.len()) as *mut u8, body.len());
        }
        place
    }

    #[test]
    fn a_hooked_lookup_returns_the_same_and_is_noted() {
        // The lookup's prologue, then: vaddss xmm0, xmm1, xmm1 ; add rsp, 0x30 ; pop rsi ; ret.
        let f = make(TUNABLE.1, &[0xc5, 0xf2, 0x58, 0xc1, 0x48, 0x83, 0xc4, 0x30, 0x5e, 0xc3]);
        // SAFETY: our own function of that signature.
        let plain: TunableFn = unsafe { std::mem::transmute::<usize, TunableFn>(f) };
        // SAFETY: as above.
        assert_eq!(unsafe { plain(0x1234, 1.5) }, 3.0);
        // SAFETY: our own function, which no other thread runs.
        unsafe { hook_function(f, TUNABLE.1, tunable as *const () as usize, &TUNABLE_ORIGINAL) }.unwrap();
        // SAFETY: as above, now through the hook.
        assert_eq!(unsafe { plain(0x1234, 1.5) }, 3.0);
        // SAFETY: as above.
        assert_eq!(unsafe { plain(0x1234, 2.0) }, 4.0);
        let seen = SEEN.lock().unwrap();
        let e = &seen.as_ref().unwrap().tunables[&0x1234];
        assert_eq!((e.default, e.value, e.calls, e.changed), (1.5f32.to_bits(), 4.0f32.to_bits(), 2, 1));
        assert!(report(seen.as_ref().unwrap()).contains("tunable 0x00001234 0x3fc00000 1.5 0x40800000 4 2 1"));
    }

    #[test]
    fn a_hooked_effect_load_is_noted_by_name() {
        // The loader's prologue, then: mov rax, rcx ; add rsp, 0x138 ; pop rbx ; pop rbp ; pop rdi ; pop rsi ; ret.
        let f = make(EFFECT.1, &[0x48, 0x89, 0xc8, 0x48, 0x81, 0xc4, 0x38, 0x01, 0x00, 0x00, 0x5b, 0x5d, 0x5f, 0x5e, 0xc3]);
        // SAFETY: our own function, which no other thread runs.
        unsafe { hook_function(f, EFFECT.1, effect as *const () as usize, &EFFECT_ORIGINAL) }.unwrap();
        // The "effect" is the name's own buffer: its program count at +0xcf8.
        let mut buffer = vec![0u8; 0x1000];
        buffer[..11].copy_from_slice(b"sr_tracing\0");
        buffer[EFFECT_PROGRAMS..EFFECT_PROGRAMS + 2].copy_from_slice(&7u16.to_le_bytes());
        // SAFETY: our own function of that signature.
        let plain: EffectFn = unsafe { std::mem::transmute::<usize, EffectFn>(f) };
        // SAFETY: as above.
        let got = unsafe { plain(buffer.as_ptr().cast(), 0) };
        assert_eq!(got, buffer.as_ptr() as usize);
        let seen = SEEN.lock().unwrap();
        let e = &seen.as_ref().unwrap().effects["sr_tracing"];
        assert_eq!((e.address, e.programs, e.loads), (buffer.as_ptr() as usize, 7, 1));
    }

    #[test]
    fn other_bytes_are_left_alone() {
        let f = make(&[0x55, 0x48, 0x89, 0xe5, 0x90, 0x90, 0x90], &[0xc3]);
        // SAFETY: our own function, which no other thread runs.
        assert!(unsafe { hook_function(f, TUNABLE.1, tunable as *const () as usize, &TUNABLE_ORIGINAL) }.is_err());
        // SAFETY: as above.
        assert_eq!(unsafe { *(f as *const u8) }, 0x55);
    }
}
