//! `qualia-shm`: the fixed shared-memory arena every qualia runner maps.
//!
//! One process — `qualia-init` — creates the region; every other runner
//! attaches to it by name, and readers and writers then share the mapped bytes.
//! The arena is always [`SHM_SIZE`] bytes and its byte layout is ABI: the
//! [`ShmHeader`] sits at offset zero, the per-layer [`LayerSlot`]s follow at
//! [`LAYER_SLOTS_OFFSET`], then the [`LedgerEntry`] ring, then the world,
//! thought, lore, sensor and JEPA regions in the order the offset constants
//! below declare. Nothing in this crate owns state — the bytes are the state,
//! and `qualia-types` declares the atomics and seqlocks that guard it.

pub use qualia_types::*;

#[cfg(not(windows))]
use std::ffi::CString;
use std::sync::atomic::Ordering;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a region could not be created, attached or validated.
pub enum ShmError {
    /// The operating system refused the call; the payload is the raw `errno` or
    /// `GetLastError` value it reported.
    OsError(i32),
    /// The mapped bytes do not open with [`SHM_MAGIC`], so they are not ours.
    BadMagic,
    /// The backing object is not exactly [`SHM_SIZE`] bytes.
    SizeMismatch,
    /// Creator and attacher disagree about [`SHM_VERSION`].
    VersionMismatch { expected: u32, found: u32 },
    /// The header parsed, but its derived layout disagrees with this build.
    LayoutMismatch,
}

impl std::fmt::Debug for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OsError(code) => write!(f, "ShmError::OsError({code})"),
            Self::BadMagic => f.write_str("ShmError::BadMagic"),
            Self::SizeMismatch => f.write_str("ShmError::SizeMismatch"),
            Self::VersionMismatch { expected, found } => write!(
                f,
                "ShmError::VersionMismatch {{ expected: {expected}, found: {found} }}"
            ),
            Self::LayoutMismatch => f.write_str("ShmError::LayoutMismatch"),
        }
    }
}

impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OsError(code) => write!(f, "shared memory OS error {code}"),
            Self::BadMagic => f.write_str("shared memory header carries a foreign magic number"),
            Self::SizeMismatch => f.write_str("shared memory backing size mismatch"),
            Self::VersionMismatch { expected, found } => write!(
                f,
                "shared memory version mismatch: this build speaks {expected}, the region speaks {found}"
            ),
            Self::LayoutMismatch => f.write_str("shared memory layout contract mismatch"),
        }
    }
}

impl std::error::Error for ShmError {}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Size of the arena: 64 MiB, fixed for the lifetime of the ABI.
pub const SHM_SIZE: usize = 64 * 1024 * 1024;

const fn align_offset(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Bytes reserved for the header, which lives at offset zero.
pub const HEADER_SIZE: usize = std::mem::size_of::<ShmHeader>();
/// Bytes in one layer slot, including its two belief buffers and weights.
pub const LAYER_SLOT_SIZE: usize = std::mem::size_of::<LayerSlot>();

/// First byte after the header page, where layer slot 0 begins.
pub const LAYER_SLOTS_OFFSET: usize = 4096;

/// Where the ledger ring starts: after the header page and every layer slot.
pub const LEDGER_OFFSET: usize = LAYER_SLOTS_OFFSET + NUM_LAYERS * LAYER_SLOT_SIZE;

/// Bytes reserved for the ledger ring.
pub const LEDGER_SIZE: usize = 16 * 1024 * 1024;

/// Ledger entries the ring can hold before it starts overwriting.
pub const MAX_LEDGER_ENTRIES: usize = LEDGER_SIZE / std::mem::size_of::<LedgerEntry>();

/// The semantic world model starts where the ledger ends.
pub const WORLD_MODEL_OFFSET: usize = LEDGER_OFFSET + LEDGER_SIZE;

/// The thought ring follows the world model.
pub const THOUGHT_BUFFER_OFFSET: usize = WORLD_MODEL_OFFSET + std::mem::size_of::<WorldModel>();

/// The lore ring follows the thought ring.
pub const LORE_BUFFER_OFFSET: usize = THOUGHT_BUFFER_OFFSET + std::mem::size_of::<ThoughtBuffer>();

/// The room-scale voxel grid follows the lore ring.
pub const WORLD_VOXELS_OFFSET: usize = LORE_BUFFER_OFFSET + std::mem::size_of::<LoreBuffer>();

/// The latest raw lidar scan follows the voxel grid.
pub const LIDAR_SCAN_OFFSET: usize = WORLD_VOXELS_OFFSET + std::mem::size_of::<WorldVoxels>();

/// The native lidar occupancy grid follows the raw scan.
pub const LIDAR_GRID_OFFSET: usize = LIDAR_SCAN_OFFSET + std::mem::size_of::<LidarScan>();

/// The persistent 2D map follows the lidar occupancy grid.
pub const MAP_GRID_OFFSET: usize = LIDAR_GRID_OFFSET + std::mem::size_of::<LidarOccupancyGrid>();

/// The derived binary map follows the persistent map.
pub const BINARY_MAP_OFFSET: usize = MAP_GRID_OFFSET + std::mem::size_of::<PersistentMapGrid>();

/// The camera thumbnail follows the binary map.
pub const CAMERA_FRAME_OFFSET: usize = BINARY_MAP_OFFSET + std::mem::size_of::<BinaryMapGrid>();

/// The bounded operator preview follows the thumbnail.
pub const CAMERA_PREVIEW_OFFSET: usize =
    CAMERA_FRAME_OFFSET + std::mem::size_of::<CameraFrame>();

/// The VSLAM frontend state follows the preview.
pub const VSLAM_FRONTEND_OFFSET: usize =
    CAMERA_PREVIEW_OFFSET + std::mem::size_of::<CameraPreview>();

/// The camera-derived floor grid follows the VSLAM frontend state.
pub const CAMERA_FLOOR_OFFSET: usize =
    VSLAM_FRONTEND_OFFSET + std::mem::size_of::<VslamFrontendState>();

/// Completed action intervals follow every legacy sensor slot, aligned for the
/// atomic pair they carry.
pub const APPLIED_ACTION_OFFSET: usize = align_offset(
    CAMERA_FLOOR_OFFSET + std::mem::size_of::<CameraFloorGrid>(),
    std::mem::align_of::<AppliedActionSlot>(),
);

/// The versioned JEPA region is append-only and starts after the legacy slots.
pub const JEPA_REGION_OFFSET: usize = align_offset(
    APPLIED_ACTION_OFFSET + std::mem::size_of::<AppliedActionSlot>(),
    64,
);
/// The coherent evidence snapshot opens the JEPA region.
pub const JEPA_EVIDENCE_OFFSET: usize = JEPA_REGION_OFFSET;
/// The runtime telemetry snapshot follows the evidence snapshot.
pub const JEPA_TELEMETRY_OFFSET: usize = align_offset(
    JEPA_EVIDENCE_OFFSET + std::mem::size_of::<JepaEvidenceSlot>(),
    std::mem::align_of::<JepaTelemetrySlot>(),
);
/// The lossless applied-action history follows the telemetry snapshot.
pub const APPLIED_ACTION_HISTORY_OFFSET: usize = align_offset(
    JEPA_TELEMETRY_OFFSET + std::mem::size_of::<JepaTelemetrySlot>(),
    std::mem::align_of::<AppliedActionHistory>(),
);
/// Extent of the append-only JEPA region, from [`JEPA_REGION_OFFSET`] to its end.
pub const JEPA_REGION_SIZE: usize = APPLIED_ACTION_HISTORY_OFFSET
    + std::mem::size_of::<AppliedActionHistory>()
    - JEPA_REGION_OFFSET;

// ShmRegion: the mapped arena and its typed accessors.

/// A mapped handle to the arena.
///
/// `create` owns the name and unlinks it when the last creator handle drops;
/// `open` only borrows a region some creator established. Both map the same
/// bytes, so a mutation made through one handle is visible through any other.
pub struct ShmRegion {
    ptr: *mut u8,
    len: usize,
    #[cfg(not(windows))]
    name: String,
    #[cfg(not(windows))]
    owner: bool,
    #[cfg(windows)]
    handle: HANDLE,
}

// SAFETY: every mutable field inside the arena is reached through the atomics
// and seqlocks declared by `qualia-types`, never through a Rust alias; the raw
// pointer is to a mapping that outlives every reference derived from it.
unsafe impl Send for ShmRegion {}
unsafe impl Sync for ShmRegion {}

impl ShmRegion {
    /// Create a fresh region. Only the stack supervisor calls this.
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

    /// Attach to a region a supervisor already created.
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

    /// Borrow the `T` stored at `offset` inside the arena.
    ///
    /// # Safety
    /// `offset` must be one of the layout constants above: each is aligned for
    /// its `T` and its whole extent lies within [`SHM_SIZE`].
    unsafe fn at<T>(&self, offset: usize) -> &T {
        &*(self.ptr.add(offset) as *const T)
    }

    /// Mutably borrow the `T` stored at `offset`, for slots with a single writer.
    ///
    /// # Safety
    /// Same address contract as [`Self::at`], plus the caller must be the only
    /// writer for that slot at this moment.
    unsafe fn at_mut<T>(&self, offset: usize) -> &mut T {
        &mut *(self.ptr.add(offset) as *mut T)
    }

    /// The ABI header at offset zero.
    pub fn header(&self) -> &ShmHeader {
        unsafe { self.at(0) }
    }

    /// The `layer`-th layer slot, `0..NUM_LAYERS`.
    pub fn layer_slot(&self, layer: usize) -> &LayerSlot {
        assert!(layer < NUM_LAYERS, "layer index {layer} out of range");
        unsafe { self.at(LAYER_SLOTS_OFFSET + layer * LAYER_SLOT_SIZE) }
    }

    /// Ledger row `index`, addressed by its absolute slot in the ring.
    pub fn ledger_entry(&self, index: usize) -> &LedgerEntry {
        assert!(index < MAX_LEDGER_ENTRIES, "ledger index {index} out of range");
        unsafe { self.at(LEDGER_OFFSET + index * std::mem::size_of::<LedgerEntry>()) }
    }

    /// Claim the next ledger slot and write `entry` into it.
    ///
    /// The sequence is bumped first, so concurrent writers land on distinct
    /// slots; a reader that trails the writer can always tell where it is.
    pub fn append_ledger(&self, entry: &LedgerEntry) {
        let seq = self
            .header()
            .ledger_write_seq
            .fetch_add(1, Ordering::AcqRel);
        let index = (seq as usize) % MAX_LEDGER_ENTRIES;
        // SAFETY: `index` is in bounds; the slot is plain data written whole.
        unsafe {
            let dst = self
                .ptr
                .add(LEDGER_OFFSET + index * std::mem::size_of::<LedgerEntry>())
                as *mut LedgerEntry;
            std::ptr::write(dst, *entry);
        }
    }

    /// Number of ledger rows appended since the region was created.
    pub fn ledger_seq(&self) -> u64 {
        self.header().ledger_write_seq.load(Ordering::Acquire)
    }

    /// The shared world model.
    pub fn world_model(&self) -> &WorldModel {
        unsafe { self.at(WORLD_MODEL_OFFSET) }
    }

    /// The shared world model, mutably. Only the vision runner writes it.
    pub fn world_model_mut(&self) -> &mut WorldModel {
        unsafe { self.at_mut(WORLD_MODEL_OFFSET) }
    }

    /// Publish a new canonical pose and advance the nav sequence.
    pub fn set_robot_pose(&self, pose: NavPose) {
        let world = self.world_model_mut();
        world.robot_pose = pose;
        world.nav_seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Publish a new goal and advance the nav sequence.
    pub fn set_nav_goal(&self, goal: NavGoal) {
        let world = self.world_model_mut();
        world.nav_goal = goal;
        world.nav_seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Copy a `tile_size`-square block out of a layer's weight matrix.
    pub fn read_weight_tile(
        &self,
        layer: usize,
        tile_row: usize,
        tile_col: usize,
        tile_size: usize,
    ) -> Vec<f32> {
        assert!(tile_size > 0, "tile_size must be > 0");
        let first_row = tile_row * tile_size;
        let first_col = tile_col * tile_size;
        assert!(first_row + tile_size <= STATE_DIM, "tile row out of range");
        assert!(first_col + tile_size <= STATE_DIM, "tile col out of range");

        let weights = &self.layer_slot(layer).weights;
        let mut tile = Vec::with_capacity(tile_size * tile_size);
        for row in 0..tile_size {
            let start = (first_row + row) * STATE_DIM + first_col;
            tile.extend_from_slice(&weights[start..start + tile_size]);
        }
        tile
    }

    /// Write a `tile_size`-square block into a layer's weight matrix.
    pub fn write_weight_tile(
        &self,
        layer: usize,
        tile_row: usize,
        tile_col: usize,
        tile_size: usize,
        values: &[f32],
    ) {
        assert!(tile_size > 0, "tile_size must be > 0");
        assert_eq!(
            values.len(),
            tile_size * tile_size,
            "tile values length mismatch"
        );
        let first_row = tile_row * tile_size;
        let first_col = tile_col * tile_size;
        assert!(first_row + tile_size <= STATE_DIM, "tile row out of range");
        assert!(first_col + tile_size <= STATE_DIM, "tile col out of range");

        // SAFETY: the tile lies inside this layer's weight matrix, and tile
        // materialization is the only writer for the bytes it covers.
        let weights = unsafe {
            let slot = self.at_mut::<LayerSlot>(LAYER_SLOTS_OFFSET + layer * LAYER_SLOT_SIZE);
            &mut slot.weights
        };
        for row in 0..tile_size {
            let src = row * tile_size;
            let dst = (first_row + row) * STATE_DIM + first_col;
            weights[dst..dst + tile_size].copy_from_slice(&values[src..src + tile_size]);
        }
    }

    /// The thought ring.
    pub fn thought_buffer(&self) -> &ThoughtBuffer {
        unsafe { self.at(THOUGHT_BUFFER_OFFSET) }
    }

    /// Append one thought, truncating `text` to leave a NUL terminator.
    pub fn emit_thought(&self, layer: u8, kind: u8, vfe: f32, text: &str) {
        // SAFETY: a layer emits only through this path, and the ring index is
        // claimed atomically so two writers never share an entry.
        let buffer = unsafe { self.at_mut::<ThoughtBuffer>(THOUGHT_BUFFER_OFFSET) };
        let seq = buffer.write_seq.fetch_add(1, Ordering::AcqRel);
        let entry = &mut buffer.entries[(seq as usize) % MAX_THOUGHTS];
        entry.seq = seq;
        entry.layer = layer;
        entry.kind = kind;
        entry.vfe = vfe;
        entry.timestamp_ns = now_ns();
        entry.text = [0u8; MAX_THOUGHT_LEN];
        let bytes = text.as_bytes();
        let len = bytes.len().min(MAX_THOUGHT_LEN - 1);
        entry.text[..len].copy_from_slice(&bytes[..len]);
    }

    /// The lore ring.
    pub fn lore_buffer(&self) -> &LoreBuffer {
        unsafe { self.at(LORE_BUFFER_OFFSET) }
    }

    /// Append one answered question to the lore ring, truncating both strings
    /// to leave NUL terminators.
    pub fn emit_lore(
        &self,
        question: &str,
        answer: &str,
        layer: u8,
        reason: u8,
        embedding_delta: f32,
        effectiveness: f32,
    ) {
        // SAFETY: as in `emit_thought`, the ring index is claimed atomically.
        let buffer = unsafe { self.at_mut::<LoreBuffer>(LORE_BUFFER_OFFSET) };
        let seq = buffer.write_seq.fetch_add(1, Ordering::AcqRel);
        let entry = &mut buffer.entries[(seq as usize) % MAX_LORE_ENTRIES];
        entry.seq = seq;
        entry.layer = layer;
        entry.reason = reason;
        entry.embedding_delta = embedding_delta;
        entry.effectiveness = effectiveness;
        entry.timestamp_ns = now_ns();

        entry.question = [0u8; MAX_LORE_QUESTION];
        let bytes = question.as_bytes();
        let len = bytes.len().min(MAX_LORE_QUESTION - 1);
        entry.question[..len].copy_from_slice(&bytes[..len]);

        entry.answer = [0u8; MAX_LORE_TEXT];
        let bytes = answer.as_bytes();
        let len = bytes.len().min(MAX_LORE_TEXT - 1);
        entry.answer[..len].copy_from_slice(&bytes[..len]);
    }

    /// The room-scale voxel grid.
    pub fn world_voxels(&self) -> &WorldVoxels {
        unsafe { self.at(WORLD_VOXELS_OFFSET) }
    }

    /// The voxel grid, mutably. Only the vision runner writes it.
    pub fn world_voxels_mut(&self) -> &mut WorldVoxels {
        unsafe { self.at_mut(WORLD_VOXELS_OFFSET) }
    }

    /// The latest raw lidar scan.
    pub fn lidar_scan(&self) -> &LidarScan {
        unsafe { self.at(LIDAR_SCAN_OFFSET) }
    }

    /// The latest raw lidar scan, mutably. Only the lidar runner writes it.
    pub fn lidar_scan_mut(&self) -> &mut LidarScan {
        unsafe { self.at_mut(LIDAR_SCAN_OFFSET) }
    }

    /// The native lidar occupancy grid.
    pub fn lidar_grid(&self) -> &LidarOccupancyGrid {
        unsafe { self.at(LIDAR_GRID_OFFSET) }
    }

    /// The lidar occupancy grid, mutably.
    pub fn lidar_grid_mut(&self) -> &mut LidarOccupancyGrid {
        unsafe { self.at_mut(LIDAR_GRID_OFFSET) }
    }

    /// The persistent occupancy map.
    pub fn map_grid(&self) -> &PersistentMapGrid {
        unsafe { self.at(MAP_GRID_OFFSET) }
    }

    /// The persistent occupancy map, mutably. Only the map runner writes it.
    pub fn map_grid_mut(&self) -> &mut PersistentMapGrid {
        unsafe { self.at_mut(MAP_GRID_OFFSET) }
    }

    /// The derived binary occupancy map.
    pub fn binary_map(&self) -> &BinaryMapGrid {
        unsafe { self.at(BINARY_MAP_OFFSET) }
    }

    /// The derived binary occupancy map, mutably.
    pub fn binary_map_mut(&self) -> &mut BinaryMapGrid {
        unsafe { self.at_mut(BINARY_MAP_OFFSET) }
    }

    /// The latest camera thumbnail and its metadata.
    pub fn camera_frame(&self) -> &CameraFrame {
        unsafe { self.at(CAMERA_FRAME_OFFSET) }
    }

    /// The camera thumbnail, mutably. Only the camera runner writes it.
    pub fn camera_frame_mut(&self) -> &mut CameraFrame {
        unsafe { self.at_mut(CAMERA_FRAME_OFFSET) }
    }

    /// The latest bounded encoded operator preview.
    pub fn camera_preview(&self) -> &CameraPreview {
        unsafe { self.at(CAMERA_PREVIEW_OFFSET) }
    }

    /// The operator preview, mutably.
    pub fn camera_preview_mut(&self) -> &mut CameraPreview {
        unsafe { self.at_mut(CAMERA_PREVIEW_OFFSET) }
    }

    /// The latest VSLAM frontend state.
    pub fn vslam_frontend(&self) -> &VslamFrontendState {
        unsafe { self.at(VSLAM_FRONTEND_OFFSET) }
    }

    /// The VSLAM frontend state, mutably.
    pub fn vslam_frontend_mut(&self) -> &mut VslamFrontendState {
        unsafe { self.at_mut(VSLAM_FRONTEND_OFFSET) }
    }

    /// The latest camera-derived floor confidence grid.
    pub fn camera_floor(&self) -> &CameraFloorGrid {
        unsafe { self.at(CAMERA_FLOOR_OFFSET) }
    }

    /// The floor confidence grid, mutably.
    pub fn camera_floor_mut(&self) -> &mut CameraFloorGrid {
        unsafe { self.at_mut(CAMERA_FLOOR_OFFSET) }
    }

    /// The latest completed post-safety action interval.
    pub fn applied_action(&self) -> &AppliedActionSlot {
        unsafe { self.at(APPLIED_ACTION_OFFSET) }
    }

    /// The append-only history of completed post-safety intervals.
    pub fn applied_action_history(&self) -> &AppliedActionHistory {
        unsafe { self.at(APPLIED_ACTION_HISTORY_OFFSET) }
    }

    /// The coherent JEPA encoder/predictor/grounding snapshot.
    pub fn jepa_evidence(&self) -> &JepaEvidenceSlot {
        unsafe { self.at(JEPA_EVIDENCE_OFFSET) }
    }

    /// The JEPA runtime counters and accelerator timing snapshot.
    pub fn jepa_telemetry(&self) -> &JepaTelemetrySlot {
        unsafe { self.at(JEPA_TELEMETRY_OFFSET) }
    }

    /// Base address of the mapping.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
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

impl Drop for ShmRegion {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        {
            // SAFETY: `ptr` and `len` describe the mapping this handle owns.
            unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
            if self.owner {
                if let Ok(name) = CString::new(self.name.as_str()) {
                    // SAFETY: the name is the one this handle created; unlinking
                    // removes the name, not the mappings other handles hold.
                    unsafe { libc::shm_unlink(name.as_ptr()) };
                }
            }
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

// ---------------------------------------------------------------------------
// Platform paths
// ---------------------------------------------------------------------------

/// Zero a freshly created mapping and stamp the ABI header into it.
///
/// # Safety
/// `base` must point at the start of a writable [`SHM_SIZE`] mapping.
unsafe fn initialize(base: *mut u8) {
    std::ptr::write_bytes(base, 0, SHM_SIZE);
    let header = &mut *(base as *mut ShmHeader);
    header.magic = SHM_MAGIC;
    header.version = SHM_VERSION;
    header.num_layers = NUM_LAYERS as u32;
    header.layer_slot_size = LAYER_SLOT_SIZE as u64;
    header.ledger_offset = LEDGER_OFFSET as u64;
    header.ledger_capacity = MAX_LEDGER_ENTRIES as u64;
    header.ledger_write_seq.store(0, Ordering::Release);
    header.total_size = SHM_SIZE as u64;
    header.jepa_region_offset = JEPA_REGION_OFFSET as u64;
    header.jepa_region_size = JEPA_REGION_SIZE as u64;
    header.jepa_abi_version = JEPA_ABI_VERSION;
    header._pad = [0; 4];
}

#[cfg(not(windows))]
fn create_posix(name: &str) -> Result<ShmRegion, ShmError> {
    let c_name = CString::new(name).map_err(|_| ShmError::OsError(libc::EINVAL))?;

    // SAFETY: O_EXCL makes creation atomic; `c_name` is a valid C string.
    let fd = unsafe {
        libc::shm_open(
            c_name.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_EXCL,
            0o600,
        )
    };
    if fd < 0 {
        return Err(ShmError::OsError(errno()));
    }

    // SAFETY: `fd` is a live descriptor for a shared-memory object.
    if unsafe { libc::ftruncate(fd, SHM_SIZE as libc::off_t) } != 0 {
        let error = errno();
        unsafe {
            libc::close(fd);
            libc::shm_unlink(c_name.as_ptr());
        }
        return Err(ShmError::OsError(error));
    }

    // SAFETY: maps the object once, shared, read-write.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            SHM_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    // SAFETY: the mapping holds its own reference to the object.
    unsafe { libc::close(fd) };

    if mapped == libc::MAP_FAILED {
        let error = errno();
        // SAFETY: the name exists and creation failed after it was made.
        unsafe { libc::shm_unlink(c_name.as_ptr()) };
        return Err(ShmError::OsError(error));
    }

    let ptr = mapped as *mut u8;
    // SAFETY: `ptr` is the base of the writable mapping created above.
    unsafe { initialize(ptr) };

    Ok(ShmRegion {
        ptr,
        len: SHM_SIZE,
        name: name.to_owned(),
        owner: true,
    })
}

#[cfg(not(windows))]
fn open_posix(name: &str) -> Result<ShmRegion, ShmError> {
    let c_name = CString::new(name).map_err(|_| ShmError::OsError(libc::EINVAL))?;

    // SAFETY: opens an object that must already exist; `c_name` is a C string.
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDWR, 0) };
    if fd < 0 {
        return Err(ShmError::OsError(errno()));
    }

    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is live and `status` points at writable storage.
    if unsafe { libc::fstat(fd, status.as_mut_ptr()) } != 0 {
        let error = errno();
        unsafe { libc::close(fd) };
        return Err(ShmError::OsError(error));
    }
    // SAFETY: `fstat` initialized `status` on success.
    let status = unsafe { status.assume_init() };
    if status.st_size != SHM_SIZE as libc::off_t {
        // SAFETY: `fd` was opened above and is not used again.
        unsafe { libc::close(fd) };
        return Err(ShmError::SizeMismatch);
    }

    // SAFETY: maps the existing object once, shared, read-write.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            SHM_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    // SAFETY: the mapping holds its own reference to the object.
    unsafe { libc::close(fd) };

    if mapped == libc::MAP_FAILED {
        return Err(ShmError::OsError(errno()));
    }

    let ptr = mapped as *mut u8;
    // SAFETY: `ptr` is the base of the mapping; the creator wrote the header.
    let header = unsafe { &*(ptr as *const ShmHeader) };
    if let Err(error) = validate_header(header) {
        // SAFETY: `ptr` and `SHM_SIZE` describe the mapping created above.
        unsafe { libc::munmap(ptr as *mut libc::c_void, SHM_SIZE) };
        return Err(error);
    }

    Ok(ShmRegion {
        ptr,
        len: SHM_SIZE,
        name: name.to_owned(),
        owner: false,
    })
}

#[cfg(windows)]
fn create_windows(name: &str) -> Result<ShmRegion, ShmError> {
    let c_name = mapping_name_wide(name)?;

    // SAFETY: an unnamed (INVALID_HANDLE_VALUE) mapping of SHM_SIZE bytes backed
    // by the page file, published under `c_name`.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (SHM_SIZE as u64 >> 32) as u32,
            (SHM_SIZE as u64 & 0xFFFF_FFFF) as u32,
            c_name.as_ptr(),
        )
    };
    if handle.is_null() {
        return Err(ShmError::OsError(errno()));
    }

    // SAFETY: the handle refers to a mapping of at least SHM_SIZE bytes.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, SHM_SIZE) };
    if view.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(ShmError::OsError(errno()));
    }

    let ptr = view.Value as *mut u8;
    // SAFETY: `ptr` is the base of the writable view created above.
    unsafe { initialize(ptr) };

    Ok(ShmRegion {
        ptr,
        len: SHM_SIZE,
        handle,
    })
}

#[cfg(windows)]
fn open_windows(name: &str) -> Result<ShmRegion, ShmError> {
    let c_name = mapping_name_wide(name)?;

    // SAFETY: opens a mapping that must already exist under `c_name`.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, c_name.as_ptr()) };
    if handle.is_null() {
        return Err(ShmError::OsError(errno()));
    }

    // SAFETY: the handle refers to the creator's mapping.
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, SHM_SIZE) };
    if view.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(ShmError::OsError(errno()));
    }

    let ptr = view.Value as *mut u8;
    // SAFETY: `ptr` is the base of the mapping; the creator wrote the header.
    let header = unsafe { &*(ptr as *const ShmHeader) };
    if let Err(error) = validate_header(header) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: ptr as *mut core::ffi::c_void,
            });
            CloseHandle(handle);
        }
        return Err(error);
    }

    Ok(ShmRegion {
        ptr,
        len: SHM_SIZE,
        handle,
    })
}

/// Check that the header describes the region this build expects.
fn validate_header(header: &ShmHeader) -> Result<(), ShmError> {
    if header.magic != SHM_MAGIC {
        return Err(ShmError::BadMagic);
    }
    if header.version != SHM_VERSION {
        return Err(ShmError::VersionMismatch {
            expected: SHM_VERSION,
            found: header.version,
        });
    }
    if header.num_layers != NUM_LAYERS as u32
        || header.layer_slot_size != LAYER_SLOT_SIZE as u64
        || header.ledger_offset != LEDGER_OFFSET as u64
        || header.ledger_capacity != MAX_LEDGER_ENTRIES as u64
        || header.total_size != SHM_SIZE as u64
        || header.jepa_region_offset != JEPA_REGION_OFFSET as u64
        || header.jepa_region_size != JEPA_REGION_SIZE as u64
        || header.jepa_abi_version != JEPA_ABI_VERSION
    {
        return Err(ShmError::LayoutMismatch);
    }
    Ok(())
}

/// The last OS error code, in the platform's own numbering.
fn errno() -> i32 {
    #[cfg(windows)]
    {
        // SAFETY: `GetLastError` takes no arguments and reads thread state.
        unsafe { GetLastError() as i32 }
    }
    #[cfg(not(windows))]
    {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
    }
}

/// Turn a region name into a kernel object name on Windows.
///
/// POSIX names carry a leading `/` and may contain more separators; Windows
/// object names use `\` or nothing, so every `/` collapses to `_`.
#[cfg(windows)]
fn mapping_name_wide(name: &str) -> Result<Vec<u16>, ShmError> {
    let normalised = name.trim_start_matches('/').replace('/', "_");
    if normalised.is_empty() || normalised.contains('\0') {
        return Err(ShmError::OsError(ERROR_INVALID_PARAMETER));
    }
    let mut wide: Vec<u16> = normalised.encode_utf16().collect();
    wide.push(0);
    Ok(wide)
}

/// `ERROR_INVALID_PARAMETER`, the Windows answer for a name that cannot be a
/// kernel object name.
#[cfg(windows)]
const ERROR_INVALID_PARAMETER: i32 = 87;

/// Wall-clock nanoseconds for stamped entries; zero if the clock is before the
/// epoch, which no correct host clock is.
fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Double-buffered layer access
// ---------------------------------------------------------------------------

/// Write side of a layer's double buffer.
///
/// A writer fills the back buffer and then publishes, flipping the index. The
/// slot's index is the only synchronization point, so exactly one writer must
/// exist per layer.
pub struct LayerWriter<'a> {
    slot: &'a LayerSlot,
}

impl<'a> LayerWriter<'a> {
    pub fn new(slot: &'a LayerSlot) -> Self {
        Self { slot }
    }

    /// The buffer readers are not looking at. Fill it, then [`Self::publish`].
    pub fn back_buffer(&self) -> &mut BeliefSlot {
        let front = self.slot.write_idx.load(Ordering::Acquire) & 1;
        // SAFETY: this crate's writer is the only one for the layer, and it
        // writes precisely the buffer the readers' index does not select.
        unsafe {
            let slot = self.slot as *const LayerSlot as *mut LayerSlot;
            &mut (*slot).buffers[1 - front]
        }
    }

    /// Flip the index so readers see what the back buffer just received.
    pub fn publish(&self) {
        let front = self.slot.write_idx.load(Ordering::Acquire) & 1;
        self.slot.write_idx.store(1 - front, Ordering::Release);
    }
}

/// Read side of a layer's double buffer.
pub struct LayerReader<'a> {
    slot: &'a LayerSlot,
}

impl<'a> LayerReader<'a> {
    pub fn new(slot: &'a LayerSlot) -> Self {
        Self { slot }
    }

    /// The most recently published belief state.
    pub fn read(&self) -> &BeliefSlot {
        let front = self.slot.write_idx.load(Ordering::Acquire) & 1;
        &self.slot.buffers[front]
    }
}
