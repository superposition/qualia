//! Host↔GPU copy helpers shared by the macOS compute paths.
//!
//! Every buffer here uses `StorageModeShared`, so `contents()` can be treated as
//! an ordinary pointer on either side of the bus. These helpers exist so the
//! two macOS modules do not each hand-roll the same pointer arithmetic.

use metal::Buffer;

/// Copies a host slice into a shared-memory buffer.
///
/// # Safety
/// `destination` must own at least `source.len()` elements of `T`; the caller
/// knows that because it allocated the buffer for exactly this payload.
pub(crate) unsafe fn upload<T>(destination: &Buffer, source: &[T]) {
    std::ptr::copy_nonoverlapping(
        source.as_ptr(),
        destination.contents() as *mut T,
        source.len(),
    );
}

/// Copies a shared-memory buffer back into a host slice.
///
/// # Safety
/// `source` must hold at least `destination.len()` elements of `T`.
pub(crate) unsafe fn download<T>(source: &Buffer, destination: &mut [T]) {
    std::ptr::copy_nonoverlapping(
        source.contents() as *const T,
        destination.as_mut_ptr(),
        destination.len(),
    );
}

/// Copies `count` elements from one shared-memory buffer to another.
///
/// # Safety
/// Both buffers must own at least `count` elements of `T`.
pub(crate) unsafe fn copy<T>(source: &Buffer, destination: &Buffer, count: usize) {
    std::ptr::copy_nonoverlapping(
        source.contents() as *const T,
        destination.contents() as *mut T,
        count,
    );
}
