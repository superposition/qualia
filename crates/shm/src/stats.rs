//! The stats region: a small mapping beside the data arena carrying one
//! [`RunnerStats`] frame per runner.
//!
//! The arena's bytes are the data; this is the record of the work that produced
//! them, and it lives in its own mapping so a telemetry frame never perturbs the
//! fixed arena layout (`crates/types` `SHM_VERSION`) that every runner and the
//! reference ABI already agree on. The name is the arena's own plus
//! [`qualia_types::STATS_REGION_SUFFIX`], or `QUALIA_STATS_SHM_NAME` when the
//! deployment sets one.
//!
//! `qualia-init` creates it beside the arena it owns and clears a stale one
//! first; a producer that is started on its own creates it just the same
//! ([`StatsRegion::create`] is create-or-attach), and a reader that finds no
//! region reports that rather than drawing zeroes. Nothing in the publish path
//! allocates, formats or blocks: [`StatsWriter`] copies counters into the mapped
//! frame and leaves.

use std::sync::atomic::Ordering;

use qualia_types::{
    stats_region_name, RunnerStats, RunnerStatsSnapshot, StatsHeader, RUNNER_STATS_FLAG_ERRORED,
    RUNNER_STATS_FLAG_PUBLISHING, RUNNER_STATS_SLOTS, STATS_REGION_SIZE,
};

use crate::ShmError;

#[cfg(not(windows))]
use std::ffi::CString;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// How often a [`StatsWriter`] writes its frame: at most ten frames a second,
/// whatever the runner's own cadence is, so a kilohertz loop costs ten small
/// stores per second rather than a thousand.
pub const RUNNER_STATS_PUBLISH_INTERVAL_NS: u64 = 100_000_000;

/// The stats region name beside `data_region`, honouring
/// `QUALIA_STATS_SHM_NAME`.
pub fn stats_region_name_from_env(data_region: &str) -> String {
    stats_region_name(
        data_region,
        std::env::var("QUALIA_STATS_SHM_NAME").ok().as_deref(),
    )
}

/// A mapped handle to the stats region.
///
/// `create` attaches to an existing region or makes one; `open` only attaches.
/// Both map the same bytes, so a frame written through one handle is visible
/// through any other.
pub struct StatsRegion {
    ptr: *mut u8,
    len: usize,
    #[cfg(windows)]
    handle: HANDLE,
}

// SAFETY: every field a writer touches is reached through the atomics and
// seqlock `qualia-types` declares; the raw pointer is to a mapping that
// outlives every reference derived from it.
unsafe impl Send for StatsRegion {}
unsafe impl Sync for StatsRegion {}

impl StatsRegion {
    /// Attach to the region at `name`, creating and initialising it when it does
    /// not exist yet.
    pub fn create(name: &str) -> Result<Self, ShmError> {
        #[cfg(windows)]
        {
            create_windows(name)
        }
        #[cfg(not(windows))]
        {
            create_posix(name)
        }
    }

    /// Attach to a region that already exists.
    pub fn open(name: &str) -> Result<Self, ShmError> {
        #[cfg(windows)]
        {
            open_windows(name)
        }
        #[cfg(not(windows))]
        {
            open_posix(name)
        }
    }

    /// Remove a stale region before a supervisor creates a fresh one.
    ///
    /// POSIX shared memory outlives the process that made it, so a crash leaves
    /// a name behind holding another run's counters; unlinking it is what makes
    /// the next start begin from zero. Windows destroys the mapping with its
    /// last handle, so there is nothing to do.
    pub fn unlink(name: &str) -> Result<(), ShmError> {
        #[cfg(not(windows))]
        {
            let c_name = CString::new(name).map_err(|_| ShmError::OsError(libc::EINVAL))?;
            // SAFETY: `c_name` is a valid C string; an absent name is not an
            // error here.
            unsafe { libc::shm_unlink(c_name.as_ptr()) };
            Ok(())
        }
        #[cfg(windows)]
        {
            let _ = name;
            Ok(())
        }
    }

    /// The region header at offset zero.
    pub fn header(&self) -> &StatsHeader {
        // SAFETY: the mapping is at least `STATS_REGION_SIZE` bytes, which the
        // header begins.
        unsafe { &*(self.ptr as *const StatsHeader) }
    }

    /// The number of slots a producer has claimed.
    pub fn slots_claimed(&self) -> usize {
        self.header().claimed()
    }

    /// Runner slot `index`, or `None` when the index is past the layout.
    pub fn slot(&self, index: usize) -> Option<&RunnerStats> {
        // SAFETY: the pointer is in range and aligned; the seqlock admits only
        // whole reads.
        Some(unsafe { &*self.slot_ptr(index)? })
    }

    /// Runner slot `index` mutably. Only the producer that claimed the slot
    /// writes it ([`StatsWriter`]); every other reader uses [`Self::slot`].
    pub fn slot_mut(&self, index: usize) -> Option<&mut RunnerStats> {
        // SAFETY: the pointer is in range and aligned, and the caller is the
        // single writer for this slot.
        Some(unsafe { &mut *self.slot_ptr(index)? })
    }

    fn slot_ptr(&self, index: usize) -> Option<*mut RunnerStats> {
        if index >= RUNNER_STATS_SLOTS {
            return None;
        }
        // SAFETY: `index` is in range and each slot is aligned for its type by
        // `#[repr(C)]` on the header and the slot.
        Some(unsafe {
            self.ptr
                .add(qualia_types::STATS_HEADER_SIZE + index * qualia_types::RUNNER_STATS_SIZE)
                .cast::<RunnerStats>()
        })
    }

    /// Claim a slot for this process. Panics only if the region is full, which
    /// a stack with more producers than [`RUNNER_STATS_SLOTS`] has already
    /// outgrown the ABI.
    pub fn claim(&self) -> usize {
        let index = self
            .header()
            .slots_claimed
            .fetch_add(1, Ordering::AcqRel) as usize;
        assert!(
            index < RUNNER_STATS_SLOTS,
            "stats region holds {RUNNER_STATS_SLOTS} runners; a {index}th producer has nowhere to write"
        );
        index
    }

    /// Every published frame, in slot order: one snapshot attempt per claimed
    /// slot, with torn or unclaimed slots left out.
    pub fn poll(&self, max_attempts: usize) -> Vec<RunnerStatsSnapshot> {
        let mut frames = Vec::with_capacity(self.slots_claimed());
        for index in 0..self.slots_claimed() {
            let Some(slot) = self.slot(index) else {
                continue;
            };
            if let Ok(frame) = slot.snapshot(max_attempts) {
                frames.push(frame);
            }
        }
        frames
    }

    /// Length of the mapping in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the mapping is empty; always false for a live region.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for StatsRegion {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        {
            // SAFETY: `ptr` and `len` describe the mapping this handle owns. The
            // name is deliberately not unlinked here: the region outlives any
            // one producer, and the supervisor clears a stale one at start-up.
            unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
        }
        #[cfg(windows)]
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr as *mut core::ffi::c_void,
            });
            CloseHandle(self.handle);
        }
    }
}

/// A producer's handle on its own stats slot.
///
/// Attaching never fails a runner: [`StatsWriter::attach`] returns `None` when
/// the region cannot be mapped, and a runner that gets `None` simply publishes
/// no frame — the console names that runner as a gap instead of the runner
/// dying for a telemetry problem.
///
/// The writer stamps the frame's `started_at_ns` with its attach instant, so a
/// panel can show the runner's uptime, and clearing the publishing flag when it
/// is dropped, so a reader sees a runner that stopped rather than the last
/// state it was in.
pub struct StatsWriter {
    region: StatsRegion,
    index: usize,
    frame: RunnerStatsSnapshot,
    window_start_ns: u64,
    window_ticks: u64,
    window_bytes: u64,
    last_publish_ns: u64,
    last_op_failed: bool,
}

impl StatsWriter {
    /// Attach to the stats region beside `data_region` and claim a slot.
    pub fn attach(data_region: &str, runner: &str) -> Option<Self> {
        let name = stats_region_name_from_env(data_region);
        Self::attach_named(&name, runner)
    }

    /// Attach to the stats region at `name` and claim a slot.
    pub fn attach_named(name: &str, runner: &str) -> Option<Self> {
        let region = StatsRegion::create(name).ok()?;
        Some(Self::from_region(region, runner))
    }

    fn from_region(region: StatsRegion, runner: &str) -> Self {
        let index = region.claim();
        let now = crate::now_ns();
        // The runner's start is the instant it attached: the frame carries it so
        // an operator's panel can show the uptime, and a restart reads as a new
        // start rather than the counters continuing.
        let mut frame = RunnerStatsSnapshot::new(runner);
        frame.started_at_ns = now;
        Self {
            region,
            index,
            frame,
            window_start_ns: now,
            window_ticks: 0,
            window_bytes: 0,
            last_publish_ns: 0,
            last_op_failed: false,
        }
    }

    /// The frame as this writer last wrote it, for a runner that wants to assert
    /// what it published without reading the mapping back.
    pub fn frame(&self) -> &RunnerStatsSnapshot {
        &self.frame
    }

    /// Count one unit of work: a tick, a frame, a scan.
    pub fn tick(&mut self) {
        self.tick_n(1);
    }

    /// Count `n` units of work.
    pub fn tick_n(&mut self, n: u64) {
        let now = crate::now_ns();
        self.frame.ticks = self.frame.ticks.saturating_add(n);
        self.advance(now);
        self.publish_if_due(now);
    }

    /// Count bytes that moved — a frame written, a segment sealed.
    pub fn add_bytes(&mut self, bytes: u64) {
        self.frame.bytes = self.frame.bytes.saturating_add(bytes);
    }

    /// Record the queue depth or backlog the runner is carrying right now.
    pub fn set_backlog(&mut self, backlog: u32) {
        self.frame.backlog = backlog;
    }

    /// Count a failed operation.
    pub fn record_error(&mut self) {
        self.frame.errors = self.frame.errors.saturating_add(1);
        self.last_op_failed = true;
    }

    /// Record one of the values the runner last emitted, with its label.
    pub fn set_value(&mut self, index: usize, label: &str, value: f32) {
        if index >= qualia_types::RUNNER_VALUE_COUNT {
            return;
        }
        self.frame.values[index] = value;
        // Allocate only when the label changes: a loop that republishes the same
        // label costs one comparison.
        if self.frame.labels[index] != label {
            self.frame.labels[index] = label.to_owned();
        }
    }

    /// Write the current frame now, whatever the publish interval says.
    pub fn publish(&mut self) {
        let now = crate::now_ns();
        self.advance(now);
        self.frame.published_at_ns = now;
        self.frame.flags = RUNNER_STATS_FLAG_PUBLISHING
            | if self.last_op_failed {
                RUNNER_STATS_FLAG_ERRORED
            } else {
                0
            };
        self.last_op_failed = false;
        if let Some(slot) = self.region.slot_mut(self.index) {
            let _ = slot.publish(&self.frame);
        }
        self.last_publish_ns = now;
    }

    fn publish_if_due(&mut self, now: u64) {
        if now.saturating_sub(self.last_publish_ns) >= RUNNER_STATS_PUBLISH_INTERVAL_NS {
            self.publish();
        }
    }

    /// Recompute the one-second rate windows.
    fn advance(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.window_start_ns);
        if elapsed < 1_000_000_000 {
            return;
        }
        let ticks = self.frame.ticks.saturating_sub(self.window_ticks) as u128;
        let bytes = self.frame.bytes.saturating_sub(self.window_bytes) as u128;
        self.frame.rate_milli_hz =
            ((ticks * 1_000_000_000_000) / u128::from(elapsed)).min(u128::from(u32::MAX)) as u32;
        self.frame.bytes_per_sec =
            ((bytes * 1_000_000_000) / u128::from(elapsed)).min(u128::from(u64::MAX)) as u64;
        self.window_start_ns = now;
        self.window_ticks = self.frame.ticks;
        self.window_bytes = self.frame.bytes;
    }
}

impl Drop for StatsWriter {
    /// A runner that stops publishing says so: dropping the writer clears
    /// [`RUNNER_STATS_FLAG_PUBLISHING`] in its slot and stamps the frame, so a
    /// reader shows `stopped` rather than the last frame's live state.
    ///
    /// A runner that is killed cannot run this, and a writer that never
    /// published leaves its slot empty: that frame ages out to `stale` on the
    /// reader's own clock, which is the other half of the flag's contract.
    fn drop(&mut self) {
        if self.last_publish_ns == 0 {
            return;
        }
        self.frame.flags &= !RUNNER_STATS_FLAG_PUBLISHING;
        self.frame.published_at_ns = crate::now_ns();
        if let Some(slot) = self.region.slot_mut(self.index) {
            let _ = slot.publish(&self.frame);
        }
    }
}

/// Zero a freshly created mapping and stamp the stats header into it.
///
/// # Safety
/// `base` must point at the start of a writable [`STATS_REGION_SIZE`] mapping.
unsafe fn initialize(base: *mut u8) {
    std::ptr::write_bytes(base, 0, STATS_REGION_SIZE);
    let header = &mut *(base as *mut StatsHeader);
    header.magic = qualia_types::STATS_REGION_MAGIC;
    header.version = qualia_types::STATS_REGION_VERSION;
    header.slot_count = RUNNER_STATS_SLOTS as u32;
    header.slots_claimed.store(0, Ordering::Release);
    header._pad = 0;
}

/// Whether the bytes at `base` already hold a stats region this build can read.
///
/// # Safety
/// `base` must point at the start of a readable [`stats header`](StatsHeader).
unsafe fn is_initialized(base: *const u8) -> bool {
    (*(base as *const StatsHeader)).is_current()
}

#[cfg(not(windows))]
fn create_posix(name: &str) -> Result<StatsRegion, ShmError> {
    let c_name = CString::new(name).map_err(|_| ShmError::OsError(libc::EINVAL))?;

    // SAFETY: O_CREAT without O_EXCL keeps this idempotent — a second caller
    // attaches to the region the first one made.
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600) };
    if fd < 0 {
        return Err(ShmError::OsError(crate::errno()));
    }

    // SAFETY: `fd` is a live descriptor for a shared-memory object.
    if unsafe { libc::ftruncate(fd, STATS_REGION_SIZE as libc::off_t) } != 0 {
        let error = crate::errno();
        unsafe { libc::close(fd) };
        return Err(ShmError::OsError(error));
    }

    // SAFETY: maps the object once, shared, read-write.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            STATS_REGION_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    // SAFETY: the mapping holds its own reference to the object.
    unsafe { libc::close(fd) };
    if mapped == libc::MAP_FAILED {
        return Err(ShmError::OsError(crate::errno()));
    }

    let ptr = mapped as *mut u8;
    // SAFETY: `ptr` is the base of the writable mapping created above.
    if !unsafe { is_initialized(ptr) } {
        unsafe { initialize(ptr) };
    }

    Ok(StatsRegion {
        ptr,
        len: STATS_REGION_SIZE,
    })
}

#[cfg(not(windows))]
fn open_posix(name: &str) -> Result<StatsRegion, ShmError> {
    let c_name = CString::new(name).map_err(|_| ShmError::OsError(libc::EINVAL))?;

    // SAFETY: opens an object that must already exist.
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDWR, 0) };
    if fd < 0 {
        return Err(ShmError::OsError(crate::errno()));
    }

    // SAFETY: maps the existing object once, shared, read-write.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            STATS_REGION_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    // SAFETY: the mapping holds its own reference to the object.
    unsafe { libc::close(fd) };
    if mapped == libc::MAP_FAILED {
        return Err(ShmError::OsError(crate::errno()));
    }

    let ptr = mapped as *mut u8;
    // SAFETY: `ptr` is the base of the mapping; the creator wrote the header.
    if !unsafe { is_initialized(ptr) } {
        unsafe { libc::munmap(ptr as *mut libc::c_void, STATS_REGION_SIZE) };
        return Err(ShmError::BadMagic);
    }

    Ok(StatsRegion {
        ptr,
        len: STATS_REGION_SIZE,
    })
}

#[cfg(windows)]
fn create_windows(name: &str) -> Result<StatsRegion, ShmError> {
    let c_name = super::mapping_name_wide(name)?;

    // SAFETY: an unnamed (INVALID_HANDLE_VALUE) mapping backed by the page file,
    // published under `c_name`. Windows returns a handle to an existing mapping
    // of the same name, which is what makes this create-or-attach.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (STATS_REGION_SIZE as u64 >> 32) as u32,
            (STATS_REGION_SIZE as u64 & 0xFFFF_FFFF) as u32,
            c_name.as_ptr(),
        )
    };
    if handle.is_null() {
        return Err(ShmError::OsError(super::errno()));
    }

    // SAFETY: the handle refers to a mapping of at least STATS_REGION_SIZE bytes.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, STATS_REGION_SIZE) };
    if view.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(ShmError::OsError(super::errno()));
    }

    let ptr = view.Value as *mut u8;
    // SAFETY: `ptr` is the base of the writable view created above.
    if !unsafe { is_initialized(ptr) } {
        unsafe { initialize(ptr) };
    }

    Ok(StatsRegion {
        ptr,
        len: STATS_REGION_SIZE,
        handle,
    })
}

#[cfg(windows)]
fn open_windows(name: &str) -> Result<StatsRegion, ShmError> {
    let c_name = super::mapping_name_wide(name)?;

    // SAFETY: opens a mapping that must already exist under `c_name`.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, c_name.as_ptr()) };
    if handle.is_null() {
        return Err(ShmError::OsError(super::errno()));
    }

    // SAFETY: the handle refers to the creator's mapping.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, STATS_REGION_SIZE) };
    if view.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(ShmError::OsError(super::errno()));
    }

    let ptr = view.Value as *mut u8;
    // SAFETY: `ptr` is the base of the mapping; the creator wrote the header.
    if !unsafe { is_initialized(ptr) } {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: ptr as *mut core::ffi::c_void,
            });
            CloseHandle(handle);
        }
        return Err(ShmError::BadMagic);
    }

    Ok(StatsRegion {
        ptr,
        len: STATS_REGION_SIZE,
        handle,
    })
}
