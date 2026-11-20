//! Giving the DMA region back, kept independent of UEFI so the firmware's
//! failures can be tested.
//!
//! PciIo `FreeBuffer` requires the pages to be unmapped first: while a
//! mapping (or an IOMMU translation) for them remains, freeing them would
//! let the allocator hand out memory the device can still reach. So the
//! buffer is freed only after `Unmap` succeeded; when `Unmap` fails, the
//! allocation and its mapping token are kept (leaked until reboot) and the
//! caller says so (review finding cbfb1142d3f0, #10).

/// The two PciIo calls that give a mapped common buffer back.
pub trait Mapped {
    type Status: Copy + core::fmt::Debug;
    /// `Unmap` of the region's mapping token.
    fn unmap(&mut self) -> Result<(), Self::Status>;
    /// `FreeBuffer` of the region's pages.
    fn free(&mut self) -> Result<(), Self::Status>;
}

/// What became of the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released<S> {
    /// Unmapped and freed.
    Freed,
    /// Unmapped, but `FreeBuffer` failed: the pages stay allocated, with no
    /// mapping.
    NotFreed(S),
    /// `Unmap` failed: `FreeBuffer` was not called; the pages stay allocated
    /// and mapped.
    Kept(S),
}

/// Unmap, and free only if that worked. Only once the device no longer
/// reads or writes the region.
pub fn release<M: Mapped>(m: &mut M) -> Released<M::Status> {
    if let Err(s) = m.unmap() {
        return Released::Kept(s);
    }
    match m.free() {
        Ok(()) => Released::Freed,
        Err(s) => Released::NotFreed(s),
    }
}
