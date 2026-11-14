//! The Realtek RTL8125 / RTL8126 MAC and its built-in PHY, independent of
//! UEFI so every sequence can be tested against a simulated chip.
//!
//! Hardware facts (register offsets, bit fields, the per-revision values and
//! the order of the bring-up writes) were learned from OpenBSD's rge(4)
//! (`sys/dev/pci/if_rge.c`, `if_rgereg.h`, ISC licence; see NOTICE);
//! docs/hardware.md describes them. The code is written here. What rge(4)
//! does that this driver deliberately does not:
//!
//! - no MAC or PHY microcode patches (rge loads Realtek firmware files for
//!   them); the patch state the platform firmware or power-on left is kept;
//! - no interrupts (the SNP polls), no RSS, one queue, no offloads, no VLAN;
//! - **no 802.3x pause in the PHY's advertisement** unless the caller asks
//!   (#2, `StormnicRealtekPause`): the link then resolves to no flow
//!   control, so a receive ring nobody drains drops frames instead of making
//!   the MAC send pause frames to the switch.
//!
//! The caller reaches the registers through `Bus` (memory BAR) and supplies
//! the firmware's Stall. `ring` (the descriptor rings) and `snp` (the SNP
//! state machine) build on this.

#[path = "barrier.rs"]
pub mod barrier;
#[path = "ring.rs"]
pub mod ring;
#[path = "snp_core.rs"]
pub mod snp;

/// PCI vendor of every chip here.
pub const VENDOR: u16 = 0x10ec;

// MAC registers (memory BAR).
pub const MAC0: u32 = 0x00;
pub const MAC4: u32 = 0x04;
pub const MAR0: u32 = 0x08;
pub const MAR4: u32 = 0x0c;
pub const TXDESC_LO: u32 = 0x20;
pub const TXDESC_HI: u32 = 0x24;
pub const INT_CFG0: u32 = 0x34;
pub const CMD: u32 = 0x37;
pub const IMR: u32 = 0x38;
pub const ISR: u32 = 0x3c;
pub const TXCFG: u32 = 0x40;
pub const RXCFG: u32 = 0x44;
pub const EECMD: u32 = 0x50;
pub const CFG1: u32 = 0x52;
pub const CFG2: u32 = 0x53;
pub const CFG5: u32 = 0x56;
pub const TDFNR: u32 = 0x57;
pub const TIMERINT: [u32; 4] = [0x58, 0x5c, 0x8c, 0xf4];
pub const CSIDR: u32 = 0x64;
pub const CSIAR: u32 = 0x68;
pub const PHYSTAT: u32 = 0x6c;
pub const PMCH: u32 = 0x6f;
pub const INT_CFG1: u32 = 0x7a;
pub const TXSTART: u32 = 0x90;
pub const MACOCP: u32 = 0xb0;
pub const PHYOCP: u32 = 0xb8;
pub const DLLPR: u32 = 0xd0;
pub const TWICMD: u32 = 0xd2;
pub const MCUCMD: u32 = 0xd3;
pub const R26_D8: u32 = 0xd8;
pub const RXMAXSIZE: u32 = 0xda;
pub const IM: u32 = 0xe2;
pub const RXDESC_LO: u32 = 0xe4;
pub const RXDESC_HI: u32 = 0xe8;
pub const CFG_F1: u32 = 0xf1;
pub const PPSW: u32 = 0xf2;
pub const R25_0382: u32 = 0x0382;
pub const INTMITI: u32 = 0x0a00;
pub const R_1880: u32 = 0x1880;
pub const ADDR0: u32 = 0x19e0;
pub const ADDR1: u32 = 0x19e4;
pub const RSS_CTRL: u32 = 0x4500;
pub const RXQUEUE_CTRL: u32 = 0x4800;
pub const EEE_TXIDLE: u32 = 0x6048;

// CMD.
pub const CMD_TXENB: u8 = 0x04;
pub const CMD_RXENB: u8 = 0x08;
pub const CMD_RESET: u8 = 0x10;
pub const CMD_STOPREQ: u8 = 0x80;
// RXCFG: which frames are accepted.
pub const RXCFG_ALLPHYS: u32 = 0x01;
pub const RXCFG_INDIV: u32 = 0x02;
pub const RXCFG_MULTI: u32 = 0x04;
pub const RXCFG_BROAD: u32 = 0x08;
pub const RXCFG_ACCEPT: u32 = 0x3f;
/// TXCFG: the chip revision field.
pub const TXCFG_HWREV: u32 = 0x7cf0_0000;
pub const TXCFG_CONFIG: u32 = 0x0300_0700;
pub const EECMD_WRITECFG: u8 = 0xc0;
pub const MCUCMD_IS_OOB: u8 = 0x80;
pub const MCUCMD_FIFOS_EMPTY: u8 = 0x30;
// PHYSTAT.
pub const PHYSTAT_FDX: u16 = 0x0001;
pub const PHYSTAT_LINK: u16 = 0x0002;
pub const PHYSTAT_10: u16 = 0x0004;
pub const PHYSTAT_100: u16 = 0x0008;
pub const PHYSTAT_1000: u16 = 0x0010;
pub const PHYSTAT_RXFLOW: u16 = 0x0020;
pub const PHYSTAT_TXFLOW: u16 = 0x0040;
pub const PHYSTAT_2500: u16 = 0x0400;
pub const PHYSTAT_5000: u16 = 0x1000;
/// OCP and CSI access: the busy / done flag.
pub const BUSY: u32 = 0x8000_0000;

// Standard MII registers of the built-in PHY, and their bits.
pub const MII_BMCR: u16 = 0;
pub const MII_ANAR: u16 = 4;
pub const MII_GTCR: u16 = 9;
pub const BMCR_RESET: u16 = 0x8000;
pub const BMCR_AUTOEN: u16 = 0x1000;
pub const BMCR_STARTNEG: u16 = 0x0200;
pub const ANAR_10: u16 = 0x0020;
pub const ANAR_10_FD: u16 = 0x0040;
pub const ANAR_TX: u16 = 0x0080;
pub const ANAR_TX_FD: u16 = 0x0100;
pub const ANAR_PAUSE: u16 = 0x0400;
pub const ANAR_ASYM_PAUSE: u16 = 0x0800;
pub const GTCR_1000_HD: u16 = 0x0100;
pub const GTCR_1000_FD: u16 = 0x0200;
/// PHY OCP register holding the 2.5G / 5G advertisement.
pub const PHY_ADV: u16 = 0xa5d4;
pub const ADV_2500: u16 = 0x0080;
pub const ADV_5000: u16 = 0x0100;
/// PHY OCP register whose low 3 bits are the PHY's state (3: ready).
pub const PHY_STATE: u16 = 0xa420;
/// PHY OCP address of MII register 0 (register r is at +2r).
pub const PHY_MII_BASE: u16 = 0xa400;

/// The MAC revision, from TXCFG's HWREV field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// RTL8125A (HWREV 0x609).
    R25,
    /// RTL8125B (0x641).
    R25B,
    /// RTL8125D, two revisions (0x688, 0x689).
    R25D(u8),
    /// RTL8126A, two revisions (0x649, 0x64a): 5 GbE.
    R26(u8),
}

impl core::fmt::Display for Chip {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Chip::R25D(rev) | Chip::R26(rev) => write!(f, "{} rev {rev}", self.name()),
            _ => f.write_str(self.name()),
        }
    }
}

impl Chip {
    pub fn from_txcfg(txcfg: u32) -> Option<Chip> {
        match txcfg & TXCFG_HWREV {
            0x6090_0000 => Some(Chip::R25),
            0x6410_0000 => Some(Chip::R25B),
            0x6880_0000 => Some(Chip::R25D(1)),
            0x6890_0000 => Some(Chip::R25D(2)),
            0x6490_0000 => Some(Chip::R26(1)),
            0x64a0_0000 => Some(Chip::R26(2)),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Chip::R25 => "RTL8125A",
            Chip::R25B => "RTL8125B",
            Chip::R25D(_) => "RTL8125D",
            Chip::R26(_) => "RTL8126A",
        }
    }

    /// The fastest speed the chip's PHY can link at, Mb/s (the tests'
    /// expected link speed).
    #[allow(dead_code)]
    pub fn max_mbps(self) -> u32 {
        if matches!(self, Chip::R26(_)) { 5000 } else { 2500 }
    }

    fn is_r26(self) -> bool { matches!(self, Chip::R26(_)) }
    fn is_r25d(self) -> bool { matches!(self, Chip::R25D(_)) }

    /// RXCFG for this revision: receive DMA burst and FIFO settings, with
    /// the 32-byte receive descriptor format (bit 24), nothing accepted yet.
    pub fn rxcfg(self) -> u32 {
        match self {
            Chip::R25 => 0x4100_0700,
            Chip::R25B => 0x4100_0c00,
            Chip::R25D(_) => 0x4120_0c00,
            Chip::R26(_) => 0x4120_0d00,
        }
    }
}

/// The memory BAR's registers, PCI config space and the firmware's Stall.
pub trait Bus {
    type Error: Copy + core::fmt::Debug;
    fn r8(&mut self, offset: u32) -> Result<u8, Self::Error>;
    fn r16(&mut self, offset: u32) -> Result<u16, Self::Error>;
    fn r32(&mut self, offset: u32) -> Result<u32, Self::Error>;
    fn w8(&mut self, offset: u32, value: u8) -> Result<(), Self::Error>;
    fn w16(&mut self, offset: u32, value: u16) -> Result<(), Self::Error>;
    fn w32(&mut self, offset: u32, value: u32) -> Result<(), Self::Error>;
    fn config_r32(&mut self, offset: u32) -> Result<u32, Self::Error>;
    fn config_w32(&mut self, offset: u32, value: u32) -> Result<(), Self::Error>;
    fn delay_us(&mut self, micros: usize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    Io(E),
    /// TXCFG reads all ones: the function is gone (or its BAR is not decoded).
    Removed,
    /// A HWREV this driver does not know (RTL8127 among them: it needs the
    /// v2 descriptors).
    UnknownChip { txcfg: u32 },
    /// Neither the backup address (0x19e0) nor MAC0 holds a unicast address.
    NoMac,
    /// CMD's reset bit did not clear within 10 ms.
    ResetTimeout { cmd: u8 },
    /// A PHY OCP access stayed busy for 20 ms.
    PhyTimeout { reg: u16 },
    /// A frame outside 14..=1518 bytes to send.
    Frame { len: usize },
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self { Error::Io(e) }
}

pub type R<T, E> = Result<T, Error<E>>;

/// Link state, from PHYSTAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Link {
    pub up: bool,
    pub mbps: u32,
    pub full_duplex: bool,
    /// The MAC honours pause frames it receives / sends them (the
    /// negotiated result).
    pub rx_pause: bool,
    pub tx_pause: bool,
}

impl Link {
    pub fn from_phystat(s: u16) -> Link {
        let mbps = if s & PHYSTAT_5000 != 0 { 5000 }
            else if s & PHYSTAT_2500 != 0 { 2500 }
            else if s & PHYSTAT_1000 != 0 { 1000 }
            else if s & PHYSTAT_100 != 0 { 100 }
            else if s & PHYSTAT_10 != 0 { 10 }
            else { 0 };
        Link {
            up: s & PHYSTAT_LINK != 0,
            mbps,
            full_duplex: s & PHYSTAT_FDX != 0,
            rx_pause: s & PHYSTAT_RXFLOW != 0,
            tx_pause: s & PHYSTAT_TXFLOW != 0,
        }
    }
}

impl core::fmt::Display for Link {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !self.up { return f.write_str("down"); }
        write!(f, "up {} Mb/s {}", self.mbps, if self.full_duplex { "full duplex" } else { "half duplex" })?;
        match (self.rx_pause, self.tx_pause) {
            (false, false) => f.write_str(", flow control off"),
            (rx, tx) => write!(f, ", flow control rx {} tx {}", if rx { "on" } else { "off" }, if tx { "on" } else { "off" }),
        }
    }
}

/// The FEC a port runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fec {
    /// No FEC: what IEEE 802.3 specifies for 10BASE-T, 100BASE-TX and
    /// 1000BASE-T, and what is said while there is no link.
    None,
    /// The LDPC code that is part of the 2.5GBASE-T / 5GBASE-T PCS
    /// (802.3 Clause 126): always on in the PHY, nothing to configure.
    Ldpc,
}

impl Fec {
    pub fn name(self) -> &'static str {
        match self {
            Fec::None => "none",
            Fec::Ldpc => "LDPC",
        }
    }
}

/// The IEEE 802.3 FEC for a BASE-T port at `mbps` (the stormnic contract,
/// #5: the driver applies the standard itself, nothing is handed down).
/// Every port of this family is twisted-pair copper, whose FEC is part of
/// the PHY's PCS: none at 1000 Mb/s and below, LDPC at 2.5G and 5G. The
/// MAC has no FEC block, so there is nothing to set; this is what runs.
pub fn standard_fec(mbps: u32) -> Fec {
    if mbps >= 2500 { Fec::Ldpc } else { Fec::None }
}

/// The 802.3 name of the BASE-T media at `mbps`; "copper" without link.
pub fn media(mbps: u32) -> &'static str {
    match mbps {
        5000 => "5GBASE-T",
        2500 => "2.5GBASE-T",
        1000 => "1000BASE-T",
        100 => "100BASE-TX",
        10 => "10BASE-T",
        _ => "copper",
    }
}

/// The one line a port reports at bind: driver, PCI address, PCI ID, chip,
/// MAC address, media, speed, FEC and link. A port whose link did not come
/// up is an `error:` line, never a quiet one.
pub struct PortLine<'a, A: core::fmt::Display> {
    pub version: &'a str,
    pub at: A,
    pub device: u16,
    pub chip: Chip,
    pub mac: [u8; 6],
    pub link: Link,
    /// How long Start waited for the link, for the error line.
    pub waited_ms: u32,
}

impl<A: core::fmt::Display> core::fmt::Display for PortLine<'_, A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let m = self.mac;
        write!(
            f,
            "stormnic-realtek {}: {} 10ec:{:04x} {}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, ",
            self.version, self.at, self.device, self.chip, m[0], m[1], m[2], m[3], m[4], m[5]
        )?;
        let l = self.link;
        if l.up {
            write!(
                f,
                "media {}, speed {} Mb/s {}, FEC {}, link UP",
                media(l.mbps), l.mbps,
                if l.full_duplex { "full duplex" } else { "half duplex" },
                standard_fec(l.mbps).name()
            )
        } else {
            write!(f, "media {}, speed none, FEC {}, link DOWN: error: no link", media(0), Fec::None.name())?;
            match self.waited_ms {
                0 => f.write_str(" (down before the MAC reset too; check the cable and the link partner)"),
                ms => write!(f, " after {ms} ms of autonegotiation (check the cable and the link partner)"),
            }
        }
    }
}

/// What `identify` reads without changing anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub chip: Chip,
    pub txcfg: u32,
    pub mac: [u8; 6],
    pub link: Link,
}

fn unicast(m: [u8; 6]) -> bool {
    m != [0; 6] && m != [0xff; 6] && m[0] & 1 == 0
}

/// The chip revision, the station address and the link, read only.
pub fn identify<B: Bus>(b: &mut B) -> R<Identity, B::Error> {
    let txcfg = b.r32(TXCFG)?;
    if txcfg == u32::MAX { return Err(Error::Removed); }
    let chip = Chip::from_txcfg(txcfg).ok_or(Error::UnknownChip { txcfg })?;
    let mac = read_mac(b)?;
    let link = link(b)?;
    Ok(Identity { chip, txcfg, mac, link })
}

/// The factory address: the backup copy at 0x19e0 (which survives a
/// previous driver's StationAddress change), else MAC0.
pub fn read_mac<B: Bus>(b: &mut B) -> R<[u8; 6], B::Error> {
    let lo = b.r32(ADDR0)?.to_le_bytes();
    let hi = b.r16(ADDR1)?.to_le_bytes();
    let backup = [lo[0], lo[1], lo[2], lo[3], hi[0], hi[1]];
    if unicast(backup) { return Ok(backup); }
    let mut m = [0u8; 6];
    for (i, byte) in m.iter_mut().enumerate() { *byte = b.r8(MAC0 + i as u32)?; }
    if unicast(m) { Ok(m) } else { Err(Error::NoMac) }
}

pub fn link<B: Bus>(b: &mut B) -> R<Link, B::Error> {
    Ok(Link::from_phystat(b.r16(PHYSTAT)?))
}

// Read-modify-write helpers.

fn set8<B: Bus>(b: &mut B, r: u32, bits: u8) -> R<(), B::Error> { let v = b.r8(r)?; Ok(b.w8(r, v | bits)?) }
fn clr8<B: Bus>(b: &mut B, r: u32, bits: u8) -> R<(), B::Error> { let v = b.r8(r)?; Ok(b.w8(r, v & !bits)?) }
fn clr16<B: Bus>(b: &mut B, r: u32, bits: u16) -> R<(), B::Error> { let v = b.r16(r)?; Ok(b.w16(r, v & !bits)?) }
fn clr32<B: Bus>(b: &mut B, r: u32, bits: u32) -> R<(), B::Error> { let v = b.r32(r)?; Ok(b.w32(r, v & !bits)?) }

// MAC OCP: a 16-bit register file behind MACOCP; the address is halved into
// bits 16-30, a write sets bit 31 with the data in the low half.

pub fn mac_ocp_w<B: Bus>(b: &mut B, reg: u16, v: u16) -> R<(), B::Error> {
    Ok(b.w32(MACOCP, (u32::from(reg >> 1) << 16) | u32::from(v) | BUSY)?)
}

pub fn mac_ocp_r<B: Bus>(b: &mut B, reg: u16) -> R<u16, B::Error> {
    b.w32(MACOCP, u32::from(reg >> 1) << 16)?;
    Ok(b.r32(MACOCP)? as u16)
}

fn mac_modify<B: Bus>(b: &mut B, reg: u16, clear: u16, set: u16) -> R<(), B::Error> {
    let v = mac_ocp_r(b, reg)?;
    mac_ocp_w(b, reg, (v & !clear) | set)
}

// PHY OCP: the same layout behind PHYOCP, but asynchronous: a write is done
// when bit 31 clears, a read when bit 31 sets.

pub fn phy_ocp_w<B: Bus>(b: &mut B, reg: u16, v: u16) -> R<(), B::Error> {
    b.w32(PHYOCP, (u32::from(reg >> 1) << 16) | u32::from(v) | BUSY)?;
    for _ in 0..20_000 {
        b.delay_us(1);
        if b.r32(PHYOCP)? & BUSY == 0 { return Ok(()); }
    }
    Err(Error::PhyTimeout { reg })
}

pub fn phy_ocp_r<B: Bus>(b: &mut B, reg: u16) -> R<u16, B::Error> {
    b.w32(PHYOCP, u32::from(reg >> 1) << 16)?;
    for _ in 0..20_000 {
        b.delay_us(1);
        let v = b.r32(PHYOCP)?;
        if v & BUSY != 0 { return Ok(v as u16); }
    }
    Err(Error::PhyTimeout { reg })
}

fn phy_modify<B: Bus>(b: &mut B, reg: u16, clear: u16, set: u16) -> R<(), B::Error> {
    let v = phy_ocp_r(b, reg)?;
    phy_ocp_w(b, reg, (v & !clear) | set)
}

/// A standard MII register of the built-in PHY.
pub fn mii_r<B: Bus>(b: &mut B, reg: u16) -> R<u16, B::Error> { phy_ocp_r(b, PHY_MII_BASE + 2 * reg) }
pub fn mii_w<B: Bus>(b: &mut B, reg: u16, v: u16) -> R<(), B::Error> { phy_ocp_w(b, PHY_MII_BASE + 2 * reg, v) }

// CSI: the PCIe configuration space as the MAC sees it, 32 bits at a time,
// through CSIAR (address, byte enables, bit 31) and CSIDR (data).

fn csi_w<B: Bus>(b: &mut B, reg: u32, v: u32) -> R<(), B::Error> {
    b.w32(CSIDR, v)?;
    b.w32(CSIAR, (reg & 0xfff) | (0xf << 12) | BUSY)?;
    for _ in 0..20_000 {
        b.delay_us(1);
        if b.r32(CSIAR)? & BUSY == 0 { break; }
    }
    b.delay_us(20);
    Ok(())
}

fn csi_r<B: Bus>(b: &mut B, reg: u32) -> R<u32, B::Error> {
    b.w32(CSIAR, (reg & 0xfff) | (0xf << 12))?;
    for _ in 0..20_000 {
        b.delay_us(1);
        if b.r32(CSIAR)? & BUSY != 0 { break; }
    }
    b.delay_us(20);
    Ok(b.r32(CSIDR)?)
}

/// Poll `f` every `step` µs, up to `tries` times; whether it came true.
fn wait<B: Bus>(b: &mut B, tries: usize, step: usize, mut f: impl FnMut(&mut B) -> R<bool, B::Error>) -> R<bool, B::Error> {
    for _ in 0..tries {
        if f(b)? { return Ok(true); }
        b.delay_us(step);
    }
    Ok(false)
}

/// Stop the MAC: nothing accepted, the RXDV gate closed, a stop request
/// until both FIFOs are empty, then a soft reset (which clears TXENB and
/// RXENB). After it the chip does no DMA.
pub fn reset<B: Bus>(b: &mut B, chip: Chip) -> R<(), B::Error> {
    clr32(b, RXCFG, RXCFG_ACCEPT)?;
    set8(b, PPSW, 0x08)?;
    set8(b, CMD, CMD_STOPREQ)?;
    b.delay_us(200);
    wait(b, 3000, 50, |b| Ok(b.r8(MCUCMD)? & MCUCMD_FIFOS_EMPTY == MCUCMD_FIFOS_EMPTY))?;
    if chip != Chip::R25 {
        wait(b, 3000, 50, |b| Ok(b.r16(IM)? & 0x0103 == 0x0103))?;
    }
    let cmd = b.r8(CMD)?;
    b.w8(CMD, cmd & (CMD_TXENB | CMD_RXENB))?;
    b.w8(CMD, CMD_RESET)?;
    if wait(b, 100, 100, |b| Ok(b.r8(CMD)? & CMD_RESET == 0))? { return Ok(()); }
    Err(Error::ResetTimeout { cmd: b.r8(CMD)? })
}

/// Quiesce (#2): interrupts masked and acknowledged, the timers off, then
/// `reset`. Every path that leaves the NIC calls this.
pub fn quiesce<B: Bus>(b: &mut B, chip: Chip) -> R<(), B::Error> {
    b.w32(IMR, 0)?;
    let isr = b.r32(ISR)?;
    b.w32(ISR, isr)?;
    for t in TIMERINT { b.w32(t, 0)?; }
    reset(b, chip)?;
    mac_modify(b, 0xc0ac, 0x1f80, 0)
}

/// Take the MAC from the management firmware (out-of-band mode, RealWoW)
/// and reset it.
pub fn exit_oob<B: Bus>(b: &mut B, chip: Chip) -> R<(), B::Error> {
    mac_ocp_w(b, 0xc0bc, 0x00ff)?;
    reset(b, chip)?;
    clr8(b, MCUCMD, MCUCMD_IS_OOB)?;
    mac_modify(b, 0xe8de, 0x4000, 0)?;
    wait(b, 10, 100, |b| Ok(b.r16(TWICMD)? & 0x0200 != 0))?;
    mac_ocp_w(b, 0xc0aa, 0x07d0)?;
    mac_ocp_w(b, 0xc0a6, 0x01b5)?;
    mac_ocp_w(b, 0xc01e, 0x5555)?;
    wait(b, 10, 100, |b| Ok(b.r16(TWICMD)? & 0x0200 != 0))?;
    // Back from UPS (ultra power saving): wait for the PHY, then clear it.
    if mac_ocp_r(b, 0xd42c)? & 0x0100 != 0 {
        wait(b, 100, 1000, |b| Ok(phy_ocp_r(b, PHY_STATE)? & 7 == 2))?;
        mac_modify(b, 0xd42c, 0x0100, 0)?;
        if chip != Chip::R25 { phy_modify(b, 0xa466, 0x0001, 0)?; }
        phy_modify(b, 0xa468, 0x000a, 0)?;
    }
    Ok(())
}

fn unlock<B: Bus>(b: &mut B) -> R<(), B::Error> { set8(b, EECMD, EECMD_WRITECFG) }
fn lock<B: Bus>(b: &mut B) -> R<(), B::Error> { clr8(b, EECMD, EECMD_WRITECFG) }

/// ASPM and CLKREQ off (a link that sleeps under a polling driver only
/// adds latency), and the PME status cleared.
fn no_aspm<B: Bus>(b: &mut B, chip: Chip) -> R<(), B::Error> {
    let locked = b.r8(EECMD)? & EECMD_WRITECFG != EECMD_WRITECFG;
    if locked { unlock(b)?; }
    if chip.is_r26() { clr8(b, INT_CFG0, 0x08)?; } else { clr8(b, CFG2, 0x80)?; }
    clr8(b, CFG5, 0x01)?;
    if locked { lock(b)?; }
    Ok(())
}

/// The chip ready for `mac_init`: out of OOB mode, PHY powered and ready,
/// UPS off, then reset. The PHY state wait is 100 ms at most; a PHY that
/// does not come ready is left to show as link down.
pub fn chip_init<B: Bus>(b: &mut B, chip: Chip) -> R<(), B::Error> {
    exit_oob(b, chip)?;
    set8(b, PMCH, 0xc0)?;
    let bmcr = mii_r(b, MII_BMCR)?;
    mii_w(b, MII_BMCR, (bmcr | BMCR_AUTOEN) & !(BMCR_RESET | 0x0800))?;
    wait(b, 100, 1000, |b| Ok(phy_ocp_r(b, PHY_STATE)? & 7 == 3))?;
    no_aspm(b, chip)?;
    clr8(b, CFG_F1, 0x80)?;
    mac_modify(b, 0xd40a, 0x0010, 0)?;
    if chip == Chip::R25 && phy_ocp_r(b, 0xc416)? != 0x0500 {
        // PHY power saving off: in PHY-patch mode, as rge(4) does.
        phy_modify(b, 0xb820, 0, 0x0010)?;
        wait(b, 1000, 100, |b| Ok(phy_ocp_r(b, 0xb800)? & 0x0040 != 0))?;
        phy_ocp_w(b, 0xc416, 0)?;
        phy_ocp_w(b, 0xc416, 0x0500)?;
        phy_modify(b, 0xb820, 0x0010, 0)?;
        wait(b, 1000, 100, |b| Ok(phy_ocp_r(b, 0xb800)? & 0x0040 == 0))?;
    }
    let v = csi_r(b, 0x108)?;
    csi_w(b, 0x108, v | 0x0010_0000)?;
    quiesce(b, chip)
}

/// Where `mac_init` points the chip.
#[derive(Clone, Copy, Debug)]
pub struct Rings {
    pub rx: u64,
    pub tx: u64,
}

/// The largest frame the MAC takes, CRC included: one 2 KiB buffer.
pub const RX_MAX: u16 = 2048;

/// Program the MAC after `chip_init`: station address, rings, the
/// revision's RXCFG/TXCFG and MAC OCP settings, interrupts and their
/// moderation off, the receive filter (station, broadcast, every multicast;
/// promiscuous on request). RX and TX stay disabled.
pub fn mac_init<B: Bus>(b: &mut B, chip: Chip, mac: [u8; 6], rings: Rings, promiscuous: bool) -> R<(), B::Error> {
    unlock(b)?;
    b.w32(MAC0, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]))?;
    b.w32(MAC4, u32::from(mac[4]) | (u32::from(mac[5]) << 8))?;
    clr8(b, CFG_F1, 0x80)?;
    no_aspm(b, chip)?;
    b.w16(EEE_TXIDLE, 1500 + 14 + 32)?;
    b.w32(RXDESC_LO, rings.rx as u32)?;
    b.w32(RXDESC_HI, (rings.rx >> 32) as u32)?;
    b.w32(TXDESC_LO, rings.tx as u32)?;
    b.w32(TXDESC_HI, (rings.tx >> 32) as u32)?;
    b.w32(RXCFG, chip.rxcfg())?;
    b.w32(TXCFG, TXCFG_CONFIG)?;
    // Extended PCIe config registers. A firmware without extended config
    // access fails these; the chip then keeps what the firmware set.
    if let Ok(v) = b.config_r32(0x70c) { let _ = b.config_w32(0x70c, (v & !0x3f00_0000) | 0x2700_0000); }
    if chip.is_r26() {
        if let Ok(v) = b.config_r32(0x890) { let _ = b.config_w32(0x890, v & !1); }
    } else if !chip.is_r25d() {
        b.w16(R25_0382, 0x221b)?;
    }
    b.w32(RSS_CTRL, 0)?;
    clr16(b, RXQUEUE_CTRL, 0x001c)?;
    clr8(b, CFG1, 0x10)?;
    mac_ocp_w(b, 0xc140, 0xffff)?;
    mac_ocp_w(b, 0xc142, 0xffff)?;
    mac_modify(b, 0xeb58, 0, 0x0001)?;
    if chip.is_r26() { clr8(b, R26_D8, 0x02)?; }
    let e614 = match chip { Chip::R25B => 0x0200, _ => 0x0300 };
    mac_modify(b, 0xe614, 0x0700, e614)?;
    mac_modify(b, 0xe63e, 0x0c00, 0)?;
    mac_modify(b, 0xe63e, 0x0030, 0x0020)?;
    mac_modify(b, 0xc0b4, 0x0001, 0)?;
    mac_modify(b, 0xc0b4, 0, 0x0001)?;
    mac_modify(b, 0xc0b4, 0, 0x000c)?;
    mac_modify(b, 0xeb6a, 0x00ff, 0x0033)?;
    mac_modify(b, 0xeb50, 0x03e0, 0x0040)?;
    mac_modify(b, 0xe056, 0x00f0, 0)?;
    b.w8(TDFNR, 0x10)?;
    mac_modify(b, 0xe040, 0x1000, 0)?;
    mac_modify(b, 0xea1c, 0x0003, 0x0001)?;
    mac_ocp_w(b, 0xe0c0, if chip.is_r25d() { 0x4403 } else { 0x4000 })?;
    mac_modify(b, 0xe052, 0, 0x0060)?;
    mac_modify(b, 0xe052, 0x0088, 0)?;
    mac_modify(b, 0xd430, 0x0fff, 0x045f)?;
    set8(b, DLLPR, 0x40 | 0x80)?;
    set8(b, PPSW, 0x40)?;
    if chip == Chip::R25 { set8(b, MCUCMD, 0x01)?; }
    if !chip.is_r25d() { mac_modify(b, 0xe080, 0x0002, 0)?; } // EEE+ off
    mac_modify(b, 0xea1c, if chip.is_r26() { 0x0304 } else { 0x0004 }, 0)?;
    // Clear the TCAM.
    mac_modify(b, 0xeb54, 0, 0x0001)?;
    b.delay_us(1);
    mac_modify(b, 0xeb54, 0x0001, 0)?;
    clr16(b, R_1880, 0x0030)?;
    if chip != Chip::R25 { clr8(b, INT_CFG0, 0x01)?; }
    for t in TIMERINT { b.w32(t, 0)?; }
    let miti = if matches!(chip, Chip::R25B | Chip::R26(_)) { 32 } else { 64 };
    for i in 0..miti { b.w32(INTMITI + 4 * i, 0)?; }
    if chip.is_r26() {
        clr8(b, INT_CFG0, 0x02 | 0x04 | 0x10)?;
        b.w16(INT_CFG1, 0)?;
    }
    mac_modify(b, 0xc0ac, 0, 0x1f80)?;
    mac_ocp_w(b, 0xe098, 0xc302)?;
    mac_modify(b, 0xe032, 0x0003, 0)?;
    let v = csi_r(b, 0x98)?;
    csi_w(b, 0x98, v & !0x0000_ff00)?;
    b.delay_us(20);
    if chip.is_r25d() { mac_modify(b, 0xe092, 0x00ff, 0x0008)?; } else { mac_modify(b, 0xe092, 0x00ff, 0)?; }
    b.w16(RXMAXSIZE, RX_MAX)?;
    clr8(b, PPSW, 0x08)?; // RXDV gate open
    set_filter(b, promiscuous)?;
    no_aspm(b, chip)?;
    lock(b)?;
    b.delay_us(10);
    b.w32(IMR, 0)?;
    Ok(())
}

/// Accept the station address, broadcast and every multicast (the SNP
/// filters further in software), and with `promiscuous` everything.
pub fn set_filter<B: Bus>(b: &mut B, promiscuous: bool) -> R<(), B::Error> {
    let v = b.r32(RXCFG)? & !RXCFG_ACCEPT;
    let accept = RXCFG_INDIV | RXCFG_BROAD | RXCFG_MULTI | if promiscuous { RXCFG_ALLPHYS } else { 0 };
    b.w32(RXCFG, v | accept)?;
    b.w32(MAR0, u32::MAX)?;
    b.w32(MAR4, u32::MAX)?;
    Ok(())
}

/// Turn RX and TX on, after the rings are filled.
pub fn enable<B: Bus>(b: &mut B) -> R<(), B::Error> {
    Ok(b.w8(CMD, CMD_TXENB | CMD_RXENB)?)
}

/// What the PHY should advertise: every speed the chip has, and pause
/// only when `pause` (#2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Advert {
    pub anar: u16,
    pub gtcr: u16,
    pub adv: u16,
}

const ANAR_SPEEDS: u16 = ANAR_10 | ANAR_10_FD | ANAR_TX | ANAR_TX_FD;
const ANAR_PAUSES: u16 = ANAR_PAUSE | ANAR_ASYM_PAUSE;

pub fn wanted(chip: Chip, current: Advert, pause: bool) -> Advert {
    let anar = (current.anar & !(ANAR_SPEEDS | ANAR_PAUSES)) | ANAR_SPEEDS | if pause { ANAR_PAUSES } else { 0 };
    let gtcr = current.gtcr | GTCR_1000_FD | GTCR_1000_HD;
    let fast = if chip.is_r26() { ADV_2500 | ADV_5000 } else { ADV_2500 };
    let adv = (current.adv & !(ADV_2500 | ADV_5000)) | fast;
    Advert { anar, gtcr, adv }
}

pub fn advertised<B: Bus>(b: &mut B) -> R<Advert, B::Error> {
    Ok(Advert { anar: mii_r(b, MII_ANAR)?, gtcr: mii_r(b, MII_GTCR)?, adv: phy_ocp_r(b, PHY_ADV)? })
}

/// Make the PHY advertise `wanted` (pause only with `pause`). If it already
/// does with autonegotiation on, nothing is written and the link stays
/// (Ok(false)); otherwise the registers are written and autonegotiation
/// restarted (Ok(true): the link drops for a few seconds).
pub fn advertise<B: Bus>(b: &mut B, chip: Chip, pause: bool) -> R<bool, B::Error> {
    let now = advertised(b)?;
    let want = wanted(chip, now, pause);
    let bmcr = mii_r(b, MII_BMCR)?;
    if now == want && bmcr & BMCR_AUTOEN != 0 { return Ok(false); }
    // Gigabit Lite off.
    phy_modify(b, 0xa428, 0x0200, 0)?;
    phy_modify(b, 0xa5ea, if chip.is_r26() { 0x0007 } else { 0x0001 }, 0)?;
    mii_w(b, MII_ANAR, want.anar)?;
    mii_w(b, MII_GTCR, want.gtcr)?;
    phy_ocp_w(b, PHY_ADV, want.adv)?;
    mii_w(b, MII_BMCR, BMCR_RESET | BMCR_AUTOEN | BMCR_STARTNEG)?;
    Ok(true)
}

/// Wait up to `ms` for the link; the last state read.
pub fn wait_link<B: Bus>(b: &mut B, ms: usize) -> R<Link, B::Error> {
    let mut l = link(b)?;
    for _ in 0..ms / 10 {
        if l.up { break; }
        b.delay_us(10_000);
        l = link(b)?;
    }
    Ok(l)
}
