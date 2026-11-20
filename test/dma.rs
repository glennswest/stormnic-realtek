//! Giving the DMA region back (`src/dma.rs`): FreeBuffer only after Unmap
//! succeeded (#10, review finding cbfb1142d3f0).
#[path = "../src/dma.rs"]
mod dma;
use dma::*;

/// A region whose Unmap and FreeBuffer answer as told, recording the calls.
struct Region {
    unmap: Result<(), &'static str>,
    free: Result<(), &'static str>,
    calls: Vec<&'static str>,
}

impl Mapped for Region {
    type Status = &'static str;
    fn unmap(&mut self) -> Result<(), &'static str> {
        self.calls.push("unmap");
        self.unmap
    }
    fn free(&mut self) -> Result<(), &'static str> {
        self.calls.push("free");
        self.free
    }
}

fn region(unmap: Result<(), &'static str>, free: Result<(), &'static str>) -> Region {
    Region { unmap, free, calls: Vec::new() }
}

#[test]
fn a_region_is_unmapped_then_freed() {
    let mut r = region(Ok(()), Ok(()));
    assert_eq!(release(&mut r), Released::Freed);
    assert_eq!(r.calls, ["unmap", "free"]);
}

#[test]
fn an_unmap_failure_keeps_the_allocation_and_never_frees_it() {
    let mut r = region(Err("DEVICE_ERROR"), Ok(()));
    assert_eq!(release(&mut r), Released::Kept("DEVICE_ERROR"));
    assert_eq!(r.calls, ["unmap"], "FreeBuffer must not run while the mapping may remain");
}

#[test]
fn a_free_failure_after_unmap_is_reported() {
    let mut r = region(Ok(()), Err("INVALID_PARAMETER"));
    assert_eq!(release(&mut r), Released::NotFreed("INVALID_PARAMETER"));
    assert_eq!(r.calls, ["unmap", "free"]);
}
