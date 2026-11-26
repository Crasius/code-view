//! The process's heap, counted: every allocation goes through the system allocator as before, and
//! [`live_bytes`] says how many bytes are allocated right now. The frame stats overlay shows it,
//! and the memory walkthrough checks that closed pull requests give their memory back.
//!
//! The count covers what Rust code allocates (the app, egui, syntect, git output, provider
//! answers). Memory the OpenGL driver or the system's own libraries allocate does not go through
//! here; the OS's private bytes include it ([`private_bytes`]). Counting costs one relaxed atomic
//! add and one thread-local add per allocation and per free.
//!
//! The process's own counters ([`private_bytes`], [`handle_count`], [`thread_count`]) come from the
//! system and are `None` where it does not give them; the memory journal writes them down.
#![expect(unsafe_code, reason = "a global allocator implements the unsafe GlobalAlloc trait")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[global_allocator]
static HEAP: CountingAllocator = CountingAllocator;

/// Bytes allocated and not yet freed, as the callers asked for them (without the allocator's own
/// bookkeeping).
static LIVE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Bytes this thread allocated minus the bytes it freed, wrapping. Constant-initialized and
    /// without a destructor, so the allocator can reach it at any point of a thread's life.
    static THREAD_NET: Cell<isize> = const { Cell::new(0) };
}

/// Bytes the process holds on the heap right now.
pub fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Bytes the calling thread allocated minus the bytes it freed so far. The difference across a
/// piece of work is what that work left on the heap, however busy the other threads are.
pub fn thread_net_bytes() -> isize {
    THREAD_NET.get()
}

/// The process's private bytes as Windows counts them (`PrivateUsage`): the live bytes above, what
/// the heap keeps of freed blocks for reuse, and what system libraries and drivers allocate. None
/// when the system does not answer.
#[cfg(windows)]
pub fn private_bytes() -> Option<usize> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: 0,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
        PrivateUsage: 0,
    };
    let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(u32::MAX);
    counters.cb = size;
    // SAFETY: takes no arguments and returns a pseudo handle that needs no closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `counters` is a writable `PROCESS_MEMORY_COUNTERS_EX` of `size` bytes, which the
    // call accepts in place of the `PROCESS_MEMORY_COUNTERS` it starts with.
    let is_read = unsafe {
        GetProcessMemoryInfo(process, (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), size)
    };
    (is_read != 0).then_some(counters.PrivateUsage)
}

/// The kernel handles the process holds open (files, threads, events, registry keys).
#[cfg(windows)]
pub fn handle_count() -> Option<u32> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    let mut count = 0_u32;
    // SAFETY: takes no arguments and returns a pseudo handle that needs no closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `count` is a writable `u32` that outlives the call.
    let is_read = unsafe { GetProcessHandleCount(process, &raw mut count) };
    (is_read != 0).then_some(count)
}

/// Counting the threads takes a snapshot of every thread on the system; the handle count covers
/// threads the app keeps.
#[cfg(windows)]
pub fn thread_count() -> Option<u32> {
    None
}

/// The process's anonymous resident bytes (`RssAnon` in `/proc/self/status`): the heap and what
/// libraries and drivers allocate, without mapped files.
#[cfg(target_os = "linux")]
pub fn private_bytes() -> Option<usize> {
    proc_status("RssAnon:").map(|kilobytes| kilobytes * 1024)
}

/// Linux keeps no handle count of its own; open files are a folder listing away, too costly here.
#[cfg(target_os = "linux")]
pub fn handle_count() -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
pub fn thread_count() -> Option<u32> {
    proc_status("Threads:").and_then(|threads| u32::try_from(threads).ok())
}

/// The number on the line of `/proc/self/status` that starts with `field`.
#[cfg(target_os = "linux")]
fn proc_status(field: &str) -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(field))?;
    line[field.len()..].split_whitespace().next()?.parse().ok()
}

/// Other systems: not counted.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn private_bytes() -> Option<usize> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn handle_count() -> Option<u32> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn thread_count() -> Option<u32> {
    None
}

fn added(bytes: usize) {
    LIVE.fetch_add(bytes, Ordering::Relaxed);
    THREAD_NET.set(THREAD_NET.get().wrapping_add_unsigned(bytes));
}

fn removed(bytes: usize) {
    LIVE.fetch_sub(bytes, Ordering::Relaxed);
    THREAD_NET.set(THREAD_NET.get().wrapping_sub_unsigned(bytes));
}

/// The system allocator with a running count of the bytes it handed out.
struct CountingAllocator;

// SAFETY: every method forwards to `System` with the caller's arguments unchanged and returns its
// result unchanged; the count is a side effect that never touches the memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract for `layout`.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            added(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract for `layout`.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            added(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` came from this allocator (that is, from `System`) with `layout`.
        unsafe { System.dealloc(pointer, layout) };
        removed(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `pointer` came from `System` with `layout`; the caller upholds `realloc`'s
        // contract for `new_size`.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                added(new_size - layout.size());
            } else {
                removed(layout.size() - new_size);
            }
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::{live_bytes, thread_net_bytes};

    /// Another test thread may allocate meanwhile, so the check allows for that noise: a block
    /// far larger than any test's background churn shows up and goes away.
    #[test]
    fn counts_a_block_while_it_lives() {
        const BLOCK: usize = 256 * 1024 * 1024;
        let before = live_bytes();
        // Zeroed: the system hands out untouched pages, so the block costs no real memory.
        let block = std::hint::black_box(vec![0_u8; BLOCK]);
        let during = live_bytes();
        assert!(during >= before + BLOCK / 2, "{before} -> {during}");
        drop(block);
        let after = live_bytes();
        assert!(after + BLOCK / 2 <= during, "{during} -> {after}");
    }

    /// The thread's own count sees exactly its block, whatever the other test threads do.
    #[test]
    fn counts_this_threads_block_exactly() {
        const BLOCK: usize = 64 * 1024 * 1024;
        let before = thread_net_bytes();
        let block = std::hint::black_box(vec![0_u8; BLOCK]);
        assert_eq!(thread_net_bytes() - before, BLOCK.cast_signed());
        drop(block);
        assert_eq!(thread_net_bytes(), before);
    }
}
