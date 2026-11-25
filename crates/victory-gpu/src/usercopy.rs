//! Copies of title memory that may not be there, without the kernel.
//!
//! A page of title memory can go away at any time, so it was read through
//! the kernel (`ReadProcessMemory`), which reports a page that is not there
//! instead of faulting. That is some 2.5 microseconds a call whatever its
//! length, and 0.6 ms a megabyte against 0.3 in place: 13,500 title copies
//! of 6.6 MB in all cost a world frame 35 ms, and its 180 MB of refreshes
//! twice what they need.
//!
//! Here the bytes are copied in place, by a routine of our own whose first
//! and last instructions are known. An access violation inside it is taken
//! by a handler that sends the routine to its failing return: the copy
//! says no, as the kernel's does, and the bytes before the page may have
//! been copied, as there.

use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};

// The routine keeps to registers a function may change (no unwind data is
// needed): rcx, rdx, r8 (destination, source, bytes), rax, r9, r10, ymm0-1.
// Inside `begin..end` nothing but rax, r8 to r10 and ymm0-1 changes, so the
// failing return may be entered from any instruction of it.
std::arch::global_asm!(
    ".globl victory_user_copy",
    ".globl victory_user_copy_begin",
    ".globl victory_user_copy_end",
    ".globl victory_user_copy_fail",
    "victory_user_copy:",
    "mov r9, rcx",
    "mov r10, rdx",
    "victory_user_copy_begin:",
    "cmp r8, 64",
    "jb 3f",
    "2:",
    "vmovdqu ymm0, ymmword ptr [r10]",
    "vmovdqu ymm1, ymmword ptr [r10 + 32]",
    "vmovdqu ymmword ptr [r9], ymm0",
    "vmovdqu ymmword ptr [r9 + 32], ymm1",
    "add r10, 64",
    "add r9, 64",
    "sub r8, 64",
    "cmp r8, 64",
    "jae 2b",
    "3:",
    "cmp r8, 8",
    "jb 5f",
    "4:",
    "mov rax, qword ptr [r10]",
    "mov qword ptr [r9], rax",
    "add r10, 8",
    "add r9, 8",
    "sub r8, 8",
    "cmp r8, 8",
    "jae 4b",
    "5:",
    "test r8, r8",
    "jz victory_user_copy_end",
    "6:",
    "mov al, byte ptr [r10]",
    "mov byte ptr [r9], al",
    "inc r10",
    "inc r9",
    "dec r8",
    "jnz 6b",
    "victory_user_copy_end:",
    "vzeroupper",
    "xor eax, eax",
    "ret",
    "victory_user_copy_fail:",
    "vzeroupper",
    "mov eax, 1",
    "ret",
);

unsafe extern "sysv64" {
    // Only named, never called by this type: see `raw`.
    static victory_user_copy: u8;
    static victory_user_copy_begin: u8;
    static victory_user_copy_end: u8;
    static victory_user_copy_fail: u8;
}

/// Copies that met a page that was not there, since the start.
pub(crate) static FAILED: AtomicU64 = AtomicU64::new(0);

/// Whether title memory is copied in place (`VICTORY_USER_COPY=0`: through
/// the kernel, as before). Off on a processor without AVX2.
fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_USER_COPY").is_none_or(|v| v != "0") && is_x86_feature_detected!("avx2");
        if on {
            // SAFETY: the handler only looks at the exception, and changes
            // the context of a thread that faulted in our routine.
            unsafe { AddVectoredExceptionHandler(1, Some(on_fault)) };
        }
        on
    })
}

/// Makes the page of `at`, whose guard our read just spent, a guard page again:
/// the kernel's copy leaves guards alone, and they are the title's. (Until now
/// the fault went to nobody: run RG3 ended in one.)
///
/// Between the fault and this, a title thread that touches the page is not
/// told (the next touch is). One that takes the guard off itself in those
/// microseconds finds it on again, unless it left another protection, which is
/// put back. From VIctory 79c9b93 (TG0000).
fn guard_again(at: usize) {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualProtect, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    if n == 0 || info.State != MEM_COMMIT {
        return;
    }
    let protect = |protection: u32| {
        let mut old = 0;
        // SAFETY: only the page's protection changes; fails if it is gone.
        (unsafe { VirtualProtect(at as *const _, 1, protection, &mut old) } != 0).then_some(old)
    };
    if let Some(old) = protect(info.Protect | PAGE_GUARD)
        && old & !PAGE_GUARD != info.Protect & !PAGE_GUARD
    {
        protect(old);
    }
}

unsafe extern "system" fn on_fault(info: *mut EXCEPTION_POINTERS) -> i32 {
    const ACCESS_VIOLATION: u32 = 0xC000_0005;
    const IN_PAGE_ERROR: u32 = 0xC000_0006;
    const GUARD_PAGE: u32 = 0x8000_0001;
    const WRITE: usize = 1;
    // SAFETY: the system passes the exception being dispatched.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    let (begin, end, fail) = (
        &raw const victory_user_copy_begin as u64,
        &raw const victory_user_copy_end as u64,
        &raw const victory_user_copy_fail as u64,
    );
    if !(begin..end).contains(&context.Rip) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = record.ExceptionCode as u32;
    if (code != ACCESS_VIOLATION && code != IN_PAGE_ERROR && code != GUARD_PAGE) || record.NumberParameters < 2 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // A guard of the title's: left as it was, and the copy fails (the
    // kernel's, which the caller makes then, reads zeros there).
    if code == GUARD_PAGE {
        guard_again(record.ExceptionInformation[1]);
    }
    // A write to a block the copies guard opens it, and goes on.
    if code == ACCESS_VIOLATION
        && record.ExceptionInformation[0] == WRITE
        && crate::watch::write_faulted(record.ExceptionInformation[1])
    {
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    context.Rip = fail;
    FAILED.fetch_add(1, Ordering::Relaxed);
    EXCEPTION_CONTINUE_EXECUTION
}

/// Copies `len` bytes from `src` to `dst` in place; `Some(false)` if a page
/// is not there (the bytes before it may have been copied), `None` if the
/// copy is the kernel's to make (switched off, or the ranges overlap).
///
/// The caller holds no lock a write fault takes (`crate::watch`'s guards).
pub(crate) fn copy(src: *const u8, dst: *mut u8, len: usize) -> Option<bool> {
    if !on() {
        return None;
    }
    let (from, to) = (src as usize, dst as usize);
    if from < to.saturating_add(len) && to < from.saturating_add(len) {
        return None;
    }
    let failed: u64;
    // SAFETY: the routine reads `len` bytes at `src` and writes them at
    // `dst`, where the caller may write; a page of either that is not there
    // ends it with 1 (`on_fault`). It changes what is declared here and
    // leaves the stack as it was.
    unsafe {
        std::arch::asm!(
            "call {routine}",
            routine = in(reg) &raw const victory_user_copy,
            inout("rcx") dst => _,
            inout("rdx") src => _,
            inout("r8") len => _,
            out("rax") failed,
            out("r9") _,
            out("r10") _,
            out("ymm0") _,
            out("ymm1") _,
            clobber_abi("win64"),
        );
    }
    Some(failed as u32 == 0)
}

/// The host's page: what [`copy_changed`] compares at a time.
const CHUNK: usize = 4096;

/// [`copy`] of what differs: each 4 KiB page of `[src, src + len)` (cut at
/// its ends) is read once and stored only if `dst` holds other bytes;
/// `each` is told the offset and bytes of those. `Some(false)` if a page is
/// not there (what came before it is done and told), `None` as for `copy`.
pub(crate) fn copy_changed(
    src: *const u8,
    dst: *mut u8,
    len: usize,
    mut each: impl FnMut(usize, usize),
) -> Option<bool> {
    if !on() {
        return None;
    }
    let mut page = [0u8; CHUNK];
    let mut at = 0;
    while at < len {
        let from = src.wrapping_add(at);
        let n = (CHUNK - from as usize % CHUNK).min(len - at);
        if !copy(from, page.as_mut_ptr(), n)? {
            return Some(false);
        }
        // SAFETY: the caller's destination, `len` bytes of host memory
        // nothing else writes meanwhile.
        let held = unsafe { std::slice::from_raw_parts_mut(dst.add(at), n) };
        if *held != page[..n] {
            held.copy_from_slice(&page[..n]);
            each(at, n);
        }
        at += n;
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::{copy, copy_changed};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        VirtualProtect,
    };

    const PAGE: usize = 4096;

    /// Three reserved pages, the first and last committed.
    fn holed() -> *mut u8 {
        // SAFETY: a fresh reservation of ours.
        unsafe {
            let base = VirtualAlloc(std::ptr::null(), 3 * PAGE, MEM_RESERVE, PAGE_NOACCESS).cast::<u8>();
            assert!(!base.is_null());
            for page in [0, 2] {
                assert!(!VirtualAlloc(base.add(page * PAGE).cast(), PAGE, MEM_COMMIT, PAGE_READWRITE).is_null());
            }
            base
        }
    }

    fn release(base: *mut u8) {
        // SAFETY: a reservation of `holed`.
        assert_ne!(unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) }, 0);
    }

    /// Every length and misalignment arrives whole, and nothing beside it.
    #[test]
    fn bytes_arrive_whole() {
        let from: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        for len in (0..200).chain([255, 256, 257, 511, 640, 777]) {
            for skew in [0, 1, 7, 33] {
                let mut to = vec![0xEE_u8; 1000];
                if copy(from[skew..].as_ptr(), to[skew + 1..].as_mut_ptr(), len) != Some(true) {
                    return; // no AVX2, or switched off
                }
                assert_eq!(&to[skew + 1..skew + 1 + len], &from[skew..skew + len], "{len} bytes at {skew}");
                assert!(to[..=skew].iter().chain(&to[skew + 1 + len..]).all(|&b| b == 0xEE));
            }
        }
    }

    /// A guard page is read by no copy, and stays one: the kernel's copy leaves
    /// it, and the process goes on.
    #[test]
    fn a_guard_page_stays_a_guard_page() {
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_GUARD, VirtualQuery};
        let pages = holed();
        let mut to = vec![0u8; PAGE];
        for other in [PAGE_READWRITE, PAGE_READONLY] {
            let mut old = 0;
            // SAFETY: the first page, committed by `holed`.
            assert_ne!(unsafe { VirtualProtect(pages.cast(), 1, other | PAGE_GUARD, &mut old) }, 0);
            for _ in 0..3 {
                let Some(read) = copy(pages, to.as_mut_ptr(), PAGE) else {
                    release(pages);
                    return;
                };
                assert!(!read);
                // SAFETY: plain data filled in by the query of our own page.
                let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
                assert_ne!(unsafe { VirtualQuery(pages.cast(), &mut info, size_of_val(&info)) }, 0);
                assert_eq!(info.Protect, other | PAGE_GUARD);
            }
        }
        release(pages);
    }

    /// A page that is not there, read or written, ends the copy with a no;
    /// the process and the next copy go on.
    #[test]
    fn a_missing_page_is_told() {
        let hole = holed();
        let mut to = vec![1u8; 3 * PAGE];
        for _ in 0..3 {
            let Some(read) = copy(hole, to.as_mut_ptr(), 3 * PAGE) else {
                release(hole);
                return;
            };
            assert!(!read, "read through a hole");
            assert_eq!(copy(to.as_ptr(), hole, 3 * PAGE), Some(false), "written through a hole");
            assert_eq!(copy(hole, to.as_mut_ptr(), PAGE), Some(true));
            // Across the end of the first page, a byte at a time.
            // SAFETY: inside the reservation.
            assert_eq!(copy(unsafe { hole.add(PAGE - 3) }, to.as_mut_ptr(), 5), Some(false));
        }
        // Not writable: a no as well (it is none of the guards').
        let mut old = 0;
        // SAFETY: a committed page of ours.
        assert_ne!(unsafe { VirtualProtect(hole.cast(), PAGE, PAGE_READONLY, &mut old) }, 0);
        assert_eq!(copy(to.as_ptr(), hole, 100), Some(false));
        release(hole);
    }

    /// Only the pages that differ are stored and told, the last one cut.
    #[test]
    fn only_pages_that_differ_are_stored() {
        let mut from = vec![7u8; 5 * PAGE + 100];
        let mut to = from.clone();
        // Two pages of the source, by its own addresses, and its last bytes.
        let start = from.as_ptr() as usize;
        let page = |k: usize| k * PAGE - start % PAGE;
        from[page(2) + 5] = 1;
        from[page(3)] = 2;
        let last = from.len() - 1;
        from[last] = 3;
        let mut told = Vec::new();
        if copy_changed(from.as_ptr(), to.as_mut_ptr(), from.len(), |at, n| told.push((at, n))) != Some(true) {
            return;
        }
        assert_eq!(to, from);
        assert_eq!(told[..2], [(page(2), PAGE), (page(3), PAGE)]);
        assert_eq!(told.len(), 3);
        assert_eq!(told[2].0 + told[2].1, from.len());
        told.clear();
        assert_eq!(copy_changed(from.as_ptr(), to.as_mut_ptr(), from.len(), |at, n| told.push((at, n))), Some(true));
        assert!(told.is_empty(), "nothing differs any more");
        // A hole is told, what came before it is done.
        let hole = holed();
        // SAFETY: the first page is committed.
        unsafe { hole.write(9) };
        let mut to = vec![0u8; 3 * PAGE];
        assert_eq!(copy_changed(hole, to.as_mut_ptr(), 3 * PAGE, |at, n| told.push((at, n))), Some(false));
        assert_eq!((told.as_slice(), to[0]), ([(0, PAGE)].as_slice(), 9));
        release(hole);
    }

    /// Ranges that overlap are left to the kernel.
    #[test]
    fn overlapping_ranges_are_not_taken() {
        let mut bytes = [0u8; 64];
        let at = bytes.as_mut_ptr();
        // SAFETY: inside the array.
        assert_eq!(copy(at, unsafe { at.add(8) }, 32), None);
    }

    /// Pages that come and go under copies on several threads: every copy
    /// either is whole or says no.
    #[test]
    fn copies_stand_pages_going_away() {
        let mut probe = [0u8; 8];
        if copy([1u8; 8].as_ptr(), probe.as_mut_ptr(), 8) != Some(true) {
            return;
        }
        // SAFETY: a fresh reservation, committed and decommitted below.
        let base = unsafe { VirtualAlloc(std::ptr::null(), 16 * PAGE, MEM_RESERVE, PAGE_NOACCESS) } as usize;
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..3 {
                s.spawn(|| {
                    let mut to = vec![0u8; 16 * PAGE];
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        copy(base as *const u8, to.as_mut_ptr(), 16 * PAGE);
                    }
                });
            }
            for _ in 0..300 {
                // SAFETY: our reservation.
                unsafe {
                    use windows_sys::Win32::System::Memory::MEM_DECOMMIT;
                    assert!(!VirtualAlloc(base as _, 16 * PAGE, MEM_COMMIT, PAGE_READWRITE).is_null());
                    std::thread::yield_now();
                    assert_ne!(VirtualFree(base as _, 16 * PAGE, MEM_DECOMMIT), 0);
                }
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // SAFETY: our reservation.
        assert_ne!(unsafe { VirtualFree(base as _, 0, MEM_RELEASE) }, 0);
    }
}
