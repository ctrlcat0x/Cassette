//! Opt-in allocation origins. Tags follow allocations across threads and frees.
//! Native driver/font/D3D allocations and child processes are outside this allocator.
#[derive(Clone, Copy)]
pub enum Domain {
    General,
    Ui,
    Catalog,
    Artwork,
    Playback,
}

#[cfg(feature = "allocation-profile")]
mod enabled {
    use super::Domain;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering::Relaxed},
    };
    thread_local! { static DOMAIN: Cell<usize> = const { Cell::new(0) }; }
    static BYTES: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
    static PEAK: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
    static BLOCKS: AtomicUsize = AtomicUsize::new(0);
    static OVERHEAD: AtomicUsize = AtomicUsize::new(0);
    pub static IMAGE_BYTES: AtomicUsize = AtomicUsize::new(0);
    pub static IMAGE_COUNT: AtomicUsize = AtomicUsize::new(0);
    #[derive(Clone, Copy, Debug)]
    pub struct AllocationSnapshot {
        pub live_bytes: [usize; 5],
        pub peak_bytes: [usize; 5],
        pub live_blocks: usize,
        pub profiler_prefix_bytes: usize,
        pub decoded_image_bytes: usize,
        pub image_resources: usize,
    }
    /// Read counters without allocating, so taking a sample does not count itself.
    pub fn snapshot() -> AllocationSnapshot {
        AllocationSnapshot {
            live_bytes: std::array::from_fn(|i| BYTES[i].load(Relaxed)),
            peak_bytes: std::array::from_fn(|i| PEAK[i].load(Relaxed)),
            live_blocks: BLOCKS.load(Relaxed),
            profiler_prefix_bytes: OVERHEAD.load(Relaxed),
            decoded_image_bytes: IMAGE_BYTES.load(Relaxed),
            image_resources: IMAGE_COUNT.load(Relaxed),
        }
    }
    #[cfg(target_os = "windows")]
    fn process_memory() -> Option<serde_json::Value> {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            size: u32,
            page_faults: u32,
            peak_working_set: usize,
            working_set: usize,
            peak_paged_pool: usize,
            paged_pool: usize,
            peak_nonpaged_pool: usize,
            nonpaged_pool: usize,
            pagefile: usize,
            peak_pagefile: usize,
            private_bytes: usize,
            private_working_set: usize,
            shared_commit: u64,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut Counters,
                size: u32,
            ) -> i32;
        }
        let mut counters = Counters::default();
        let mut size = std::mem::size_of::<Counters>() as u32;
        // SAFETY: -1 is Windows' current-process pseudo handle. Counters has the
        // documented PROCESS_MEMORY_COUNTERS_EX2 layout and a live writable buffer.
        let read = |counters: &mut Counters, size| unsafe {
            counters.size = size;
            K32GetProcessMemoryInfo(-1isize as *mut _, counters, size) != 0
        };
        let extended = read(&mut counters, size);
        if !extended {
            // Older Windows supports the EX prefix but not the EX2 extension.
            size = std::mem::offset_of!(Counters, private_working_set) as u32;
            if !read(&mut counters, size) {
                return None;
            }
        }
        Some(serde_json::json!({
            "working_set_bytes": counters.working_set,
            "peak_working_set_bytes": counters.peak_working_set,
            "private_committed_bytes": counters.private_bytes,
            "peak_private_committed_bytes": counters.peak_pagefile,
            "private_working_set_bytes": extended.then_some(counters.private_working_set)
        }))
    }
    pub struct Guard(usize);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = DOMAIN.try_with(|domain| domain.set(self.0));
        }
    }
    pub fn enter(domain: Domain) -> Guard {
        Guard(
            DOMAIN
                .try_with(|current| current.replace(domain as usize))
                .unwrap_or(0),
        )
    }
    pub struct TrackedAllocator;
    fn expanded(layout: Layout) -> Option<(Layout, usize)> {
        Layout::new::<usize>().extend(layout).ok()
    }
    // SAFETY: preserve System's alignment with Layout::extend; dealloc/realloc
    // recover the original base with the SAME caller layout. The prefix stores
    // allocation origin, not current thread origin. No hook allocates or locks.
    unsafe impl GlobalAlloc for TrackedAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let Some((whole, offset)) = expanded(layout) else {
                return std::ptr::null_mut();
            };
            let base = unsafe { System.alloc(whole) };
            if base.is_null() {
                return base;
            }
            let domain = DOMAIN.try_with(Cell::get).unwrap_or(0);
            unsafe {
                base.cast::<usize>().write(domain);
            }
            let bytes = BYTES[domain].fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK[domain].fetch_max(bytes, Relaxed);
            BLOCKS.fetch_add(1, Relaxed);
            OVERHEAD.fetch_add(whole.size() - layout.size(), Relaxed);
            unsafe { base.add(offset) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            let Some((whole, offset)) = expanded(layout) else {
                return;
            };
            let base = unsafe { pointer.sub(offset) };
            let domain = unsafe { base.cast::<usize>().read() };
            BYTES[domain].fetch_sub(layout.size(), Relaxed);
            BLOCKS.fetch_sub(1, Relaxed);
            OVERHEAD.fetch_sub(whole.size() - layout.size(), Relaxed);
            unsafe {
                System.dealloc(base, whole);
            }
        }
        unsafe fn realloc(&self, pointer: *mut u8, old: Layout, size: usize) -> *mut u8 {
            let Some((whole, offset)) = expanded(old) else {
                return std::ptr::null_mut();
            };
            let Ok(new) = Layout::from_size_align(size, old.align()) else {
                return std::ptr::null_mut();
            };
            let Some((new_whole, _)) = expanded(new) else {
                return std::ptr::null_mut();
            };
            let base = unsafe { pointer.sub(offset) };
            let domain = unsafe { base.cast::<usize>().read() };
            let moved = unsafe { System.realloc(base, whole, new_whole.size()) };
            if moved.is_null() {
                return moved;
            }
            let bytes = if size >= old.size() {
                BYTES[domain].fetch_add(size - old.size(), Relaxed) + size - old.size()
            } else {
                BYTES[domain].fetch_sub(old.size() - size, Relaxed) - (old.size() - size)
            };
            PEAK[domain].fetch_max(bytes, Relaxed);
            unsafe { moved.add(offset) }
        }
    }
    pub fn start_reporter() {
        let Some(path) = std::env::var_os("RAAG_MEMORY_LOG") else {
            return;
        };
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let names = [
                    "general_framework",
                    "ui",
                    "catalog",
                    "artwork_workers",
                    "playback",
                ];
                let sample = snapshot();
                let domains: Vec<_> = names.into_iter().enumerate().map(|(i, name)| serde_json::json!({
                "origin": name, "live_bytes": sample.live_bytes[i], "peak_bytes": sample.peak_bytes[i]
            })).collect();
                let report = serde_json::json!({ "domains": domains,
                    "live_blocks": sample.live_blocks, "profiler_prefix_bytes": sample.profiler_prefix_bytes,
                    "decoded_image_bytes": sample.decoded_image_bytes, "image_resources": sample.image_resources,
                    "process_id": std::process::id()
                });
                #[cfg(target_os = "windows")]
                let report = {
                    let mut report = report;
                    if let Some(memory) = process_memory() {
                        report["process_memory"] = memory;
                    }
                    report
                };
                if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
                    let _ = std::fs::write(&path, bytes);
                }
            }
        });
    }
}
#[cfg(feature = "allocation-profile")]
pub use enabled::*;
#[cfg(not(feature = "allocation-profile"))]
pub struct Guard;
#[cfg(not(feature = "allocation-profile"))]
pub fn enter(_: Domain) -> Guard {
    Guard
}
