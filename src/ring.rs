//! The receive and transmit descriptor rings, polled, one 2 KiB
//! driver-owned buffer per descriptor.
//!
//! Descriptor formats (docs/hardware.md): transmit descriptors are 16
//! bytes — `cmdsts` (OWN 31, EOR 30, SOF 29, EOF 28, length 0-15),
//! `extsts`, then the 64-bit buffer address. Receive descriptors are the
//! 32-byte format RXCFG bit 24 selects: the buffer address at byte 16,
//! `extsts` at 24 and `cmdsts` at 28 (OWN 31, EOR 30, SOF 25, EOF 24, error
//! summary 20, length 0-13, CRC included). EOR marks the ring's last
//! descriptor; the chip wraps after it.
//!
//! Region layout (`DMA_BYTES`, one PciIo common buffer): the receive ring
//! at 0 (32 x 32 B), the transmit ring at 1024 (32 x 16 B; both 256-byte
//! aligned), then from 4096 the 32 receive buffers and the 32 transmit
//! buffers. Frames are copied in and out, so no caller memory is mapped
//! for DMA. Every access to the region is volatile and little-endian, and
//! the ownership hand-offs are ordered by `barrier` (#2: aarch64).

use super::barrier::{dma_rmb, dma_wmb, mmio_wmb};
use super::{Bus, Chip, Error, Link, Rings, R, TXSTART};
use core::ptr;

pub const RX_COUNT: usize = 32;
pub const TX_COUNT: usize = 32;
pub const BUF_SIZE: usize = 2048;
const RX_DESC: usize = 32;
const TX_DESC: usize = 16;
pub const RX_RING: usize = 0;
pub const TX_RING: usize = RX_COUNT * RX_DESC;
pub const RX_BUFS: usize = 4096;
pub const TX_BUFS: usize = RX_BUFS + RX_COUNT * BUF_SIZE;
pub const DMA_BYTES: usize = TX_BUFS + TX_COUNT * BUF_SIZE;
pub const DMA_PAGES: usize = DMA_BYTES.div_ceil(4096);

pub const OWN: u32 = 1 << 31;
pub const EOR: u32 = 1 << 30;
pub const TX_SOF: u32 = 1 << 29;
pub const TX_EOF: u32 = 1 << 28;
pub const RX_SOF: u32 = 1 << 25;
pub const RX_EOF: u32 = 1 << 24;
pub const RX_ERR: u32 = 1 << 20;
pub const RX_LEN: u32 = 0x3fff;
/// Receive descriptor field offsets.
pub const RX_ADDR: usize = 16;
pub const RX_EXTSTS: usize = 24;
pub const RX_CMDSTS: usize = 28;

/// Largest frame without CRC: 1514 plus a 4-byte VLAN tag.
pub const MAX_FRAME: usize = 1518;
/// Ethernet header: the smallest frame the driver will send.
pub const MIN_FRAME: usize = 14;
/// Short frames are padded to this (64 bytes on the wire with the CRC the
/// MAC appends).
pub const PAD_TO: usize = 60;
const CRC: usize = 4;

/// The DMA region: `DMA_BYTES` at `host`, which the device reaches at `device`.
#[derive(Clone, Copy, Debug)]
pub struct Dma {
    pub host: *mut u8,
    pub device: u64,
}

/// A frame's Ethernet header fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ethertype: u16,
}

pub fn frame(f: &[u8]) -> Frame {
    let mut d = [0u8; 6];
    let mut s = [0u8; 6];
    d.copy_from_slice(&f[0..6]);
    s.copy_from_slice(&f[6..12]);
    Frame { destination: d, source: s, ethertype: u16::from_be_bytes([f[12], f[13]]) }
}

pub struct Net {
    dma: Dma,
    pub chip: Chip,
    pub mac: [u8; 6],
    /// The receive descriptor to look at next.
    rx_next: usize,
    /// Transmit descriptors handed to the chip and not yet seen back:
    /// `tx_cons` up to (not including) `tx_prod`, counted modulo 2^usize.
    tx_prod: usize,
    tx_cons: usize,
    pub running: bool,
    /// Receive descriptors that came back with an error or a frame split
    /// over several buffers, since `start`.
    pub rx_errors: usize,
}

impl Net {
    /// # Safety
    /// `dma.host` is `DMA_BYTES` of memory the device reaches at
    /// `dma.device`, used by nothing else while this `Net` lives.
    pub unsafe fn new(dma: Dma, chip: Chip, mac: [u8; 6]) -> Net {
        Net { dma, chip, mac, rx_next: 0, tx_prod: 0, tx_cons: 0, running: false, rx_errors: 0 }
    }

    pub fn rings(&self) -> Rings {
        Rings { rx: self.dma.device + RX_RING as u64, tx: self.dma.device + TX_RING as u64 }
    }

    fn r32(&self, off: usize) -> u32 {
        // SAFETY: `off` + 4 is inside the region (callers index the rings).
        u32::from_le(unsafe { ptr::read_volatile(self.dma.host.add(off) as *const u32) })
    }

    fn w32(&self, off: usize, v: u32) {
        // SAFETY: as `r32`.
        unsafe { ptr::write_volatile(self.dma.host.add(off) as *mut u32, v.to_le()) }
    }

    fn w64(&self, off: usize, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
    }

    fn rx_desc(i: usize) -> usize { RX_RING + i * RX_DESC }
    fn tx_desc(i: usize) -> usize { TX_RING + i * TX_DESC }

    /// Give receive descriptor `i` (back) to the chip.
    fn post_rx(&self, i: usize) {
        let d = Self::rx_desc(i);
        self.w64(d + RX_ADDR, self.dma.device + (RX_BUFS + i * BUF_SIZE) as u64);
        self.w32(d + RX_EXTSTS, 0);
        dma_wmb();
        let eor = if i == RX_COUNT - 1 { EOR } else { 0 };
        self.w32(d + RX_CMDSTS, OWN | eor | BUF_SIZE as u32);
    }

    /// The whole bring-up: `chip_init`, `mac_init` with these rings, every
    /// receive buffer posted, RX and TX on. On failure the chip is
    /// quiesced again and nothing runs.
    pub fn start<B: Bus>(&mut self, b: &mut B, promiscuous: bool) -> R<(), B::Error> {
        self.running = false;
        let r = self.bring_up(b, promiscuous);
        if r.is_err() {
            let _ = super::quiesce(b, self.chip);
        }
        r
    }

    fn bring_up<B: Bus>(&mut self, b: &mut B, promiscuous: bool) -> R<(), B::Error> {
        super::chip_init(b, self.chip)?;
        // The chip is reset: the rings are ours to rewrite.
        // SAFETY: the two rings are inside the region.
        unsafe { ptr::write_bytes(self.dma.host, 0, RX_BUFS) };
        for i in 0..TX_COUNT {
            let d = Self::tx_desc(i);
            self.w64(d + 8, self.dma.device + (TX_BUFS + i * BUF_SIZE) as u64);
        }
        self.w32(Self::tx_desc(TX_COUNT - 1), EOR);
        for i in 0..RX_COUNT { self.post_rx(i); }
        self.rx_next = 0;
        self.tx_prod = 0;
        self.tx_cons = 0;
        self.rx_errors = 0;
        super::mac_init(b, self.chip, self.mac, self.rings(), promiscuous)?;
        mmio_wmb();
        super::enable(b)?;
        self.running = true;
        Ok(())
    }

    /// Quiesce the chip (#2). Ok: it does no DMA any more.
    pub fn stop<B: Bus>(&mut self, b: &mut B) -> R<(), B::Error> {
        self.running = false;
        super::quiesce(b, self.chip)
    }

    pub fn link<B: Bus>(&self, b: &mut B) -> R<Link, B::Error> { super::link(b) }

    pub fn set_promiscuous<B: Bus>(&self, b: &mut B, on: bool) -> R<(), B::Error> {
        if self.running { super::set_filter(b, on)?; }
        Ok(())
    }

    /// Transmit descriptors the chip still owns.
    pub fn tx_pending(&self) -> usize { self.tx_prod.wrapping_sub(self.tx_cons) }

    /// Retire the transmit descriptors the chip has finished with.
    pub fn reclaim(&mut self) {
        while self.tx_pending() > 0 {
            let i = self.tx_cons % TX_COUNT;
            if self.r32(Self::tx_desc(i)) & OWN != 0 { break; }
            self.tx_cons = self.tx_cons.wrapping_add(1);
        }
        dma_rmb();
    }

    /// Copy `f` into the next transmit buffer and hand it to the chip.
    /// Ok(false) when every descriptor is still in flight.
    pub fn transmit<B: Bus>(&mut self, b: &mut B, f: &[u8]) -> R<bool, B::Error> {
        if f.len() < MIN_FRAME || f.len() > MAX_FRAME { return Err(Error::Frame { len: f.len() }); }
        self.reclaim();
        if self.tx_pending() == TX_COUNT {
            // Every descriptor is in flight. Ring the doorbell again in case
            // the chip went idle before seeing the last ones, then look once
            // more.
            mmio_wmb();
            b.w16(TXSTART, 1)?;
            self.reclaim();
            if self.tx_pending() == TX_COUNT { return Ok(false); }
        }
        let i = self.tx_prod % TX_COUNT;
        let buf = TX_BUFS + i * BUF_SIZE;
        let len = f.len().max(PAD_TO);
        // SAFETY: a 2 KiB buffer inside the region that the chip does not
        // own (its descriptor is retired); `len` <= 1518.
        unsafe {
            let dst = self.dma.host.add(buf);
            for (k, byte) in f.iter().enumerate() { ptr::write_volatile(dst.add(k), *byte); }
            for k in f.len()..len { ptr::write_volatile(dst.add(k), 0); }
        }
        let d = Self::tx_desc(i);
        self.w64(d + 8, self.dma.device + buf as u64);
        self.w32(d + 4, 0);
        dma_wmb();
        let eor = if i == TX_COUNT - 1 { EOR } else { 0 };
        self.w32(d, OWN | eor | TX_SOF | TX_EOF | len as u32);
        self.tx_prod = self.tx_prod.wrapping_add(1);
        mmio_wmb();
        b.w16(TXSTART, 1)?;
        Ok(true)
    }

    /// The next good received frame: its length (CRC removed) and
    /// destination, without taking it. Descriptors with errors, split
    /// frames and runts are given back to the chip on the way.
    pub fn peek(&mut self) -> Option<(usize, [u8; 6])> {
        if !self.running { return None; }
        loop {
            let d = Self::rx_desc(self.rx_next);
            let s = self.r32(d + RX_CMDSTS);
            if s & OWN != 0 { return None; }
            dma_rmb();
            let len = (s & RX_LEN) as usize;
            let whole = s & (RX_SOF | RX_EOF) == RX_SOF | RX_EOF;
            if s & RX_ERR != 0 || !whole || len < MIN_FRAME + CRC || len > BUF_SIZE {
                self.rx_errors += 1;
                self.skip();
                continue;
            }
            let mut dst = [0u8; 6];
            let buf = RX_BUFS + self.rx_next * BUF_SIZE;
            for (k, byte) in dst.iter_mut().enumerate() {
                // SAFETY: inside the received buffer, which the chip
                // handed back (OWN clear, ordered by dma_rmb).
                *byte = unsafe { ptr::read_volatile(self.dma.host.add(buf + k)) };
            }
            return Some((len - CRC, dst));
        }
    }

    /// Give the current receive descriptor back without reading it.
    pub fn skip(&mut self) {
        self.post_rx(self.rx_next);
        self.rx_next = (self.rx_next + 1) % RX_COUNT;
    }

    /// Take the next good frame into `out`: Ok(Some(len)), Ok(None) when
    /// nothing is waiting. A frame longer than `out` stays queued
    /// (`Error::Frame` with its length).
    pub fn receive<E>(&mut self, out: &mut [u8]) -> R<Option<usize>, E> {
        let Some((len, _)) = self.peek() else { return Ok(None) };
        if len > out.len() { return Err(Error::Frame { len }); }
        let buf = RX_BUFS + self.rx_next * BUF_SIZE;
        for (k, byte) in out[..len].iter_mut().enumerate() {
            // SAFETY: as in `peek`.
            *byte = unsafe { ptr::read_volatile(self.dma.host.add(buf + k)) };
        }
        self.skip();
        Ok(Some(len))
    }
}

/// The frame `check` sends: broadcast, ethertype 0x88b5 (IEEE local
/// experimental), padded to 60 bytes.
pub fn check_frame(mac: [u8; 6]) -> [u8; 60] {
    let mut f = [0u8; 60];
    f[0..6].copy_from_slice(&[0xff; 6]);
    f[6..12].copy_from_slice(&mac);
    f[12..14].copy_from_slice(&0x88b5u16.to_be_bytes());
    let text = b"stormnic-realtek DMA check";
    f[14..14 + text.len()].copy_from_slice(text);
    f
}

/// Send the check frame and wait up to 100 ms for the chip to clear its
/// OWN bit: it fetched the descriptor and the buffer and wrote the
/// descriptor back, so DMA works both ways. Ok(Some(ms)) when it did.
pub fn check<B: Bus>(b: &mut B, net: &mut Net) -> R<Option<usize>, B::Error> {
    if !net.transmit(b, &check_frame(net.mac))? { return Ok(None); }
    for ms in 0..=100 {
        net.reclaim();
        if net.tx_pending() == 0 { return Ok(Some(ms)); }
        b.delay_us(1000);
    }
    Ok(None)
}
