//! A simulated RTL8125/8126 that DMAs, shared by the host tests: a byte
//! register file with the registers that act (CMD's reset, the FIFO and
//! TWI status bits, MAC/PHY OCP and CSI windows, PHYSTAT from a modelled
//! link, the TX doorbell), descriptor rings walked the way docs/hardware.md
//! describes, and a link partner whose pause advertisement the PHY's is
//! resolved against (IEEE 802.3 Annex 28B: pause only when both offer it).
use crate::rtl::ring::{self, Dma, Net, DMA_BYTES};
use crate::rtl::{self, Bus, Chip};
use std::collections::HashMap;

pub const MAC: [u8; 6] = [0x00, 0xe0, 0x4c, 0x68, 0x01, 0x25];
/// Where the simulated IOMMU puts the region.
pub const DEVICE_BASE: u64 = 0x1_2345_0000;
/// What a firmware driver typically leaves in ANAR: 10/100 half and full,
/// PAUSE and ASYM_PAUSE, selector 802.3.
pub const FIRMWARE_ANAR: u16 = 0x0de1;
/// Autonegotiation time after a restart.
pub const AN_US: u64 = 2_000_000;

pub struct Dev {
    base: *mut u8,
    pub regs: Vec<u8>,
    pub mac_ocp: HashMap<u16, u16>,
    pub phy: HashMap<u16, u16>,
    pub csi: HashMap<u32, u32>,
    pub config: HashMap<u32, u32>,
    mac_ocp_addr: u16,
    phy_readback: u32,
    csi_readback: u32,
    /// Microseconds of Stall so far.
    pub now: u64,
    pub cable: bool,
    pub partner_pause: bool,
    /// Autonegotiation completes at this time.
    an_done_at: u64,
    pub an_restarts: usize,
    pub tx_enabled: bool,
    pub rx_enabled: bool,
    pub resets: usize,
    /// CMD's reset bit never clears.
    pub reset_stuck: bool,
    /// PHY OCP accesses never finish.
    pub phy_stuck: bool,
    /// The doorbell is ignored (a hung transmitter).
    pub tx_stalled: bool,
    tx_idx: usize,
    rx_idx: usize,
    pub wire: Vec<Vec<u8>>,
    pub loopback: bool,
    pub dropped: usize,
    /// Every register write as (offset, value).
    pub ops: Vec<(u32, u32)>,
    /// Doorbells that found a descriptor not yet owned by the chip at the
    /// producer index (would be a lost frame on hardware).
    pub early_doorbells: usize,
}

impl Dev {
    pub fn new(chip: Chip) -> Dev {
        let layout = std::alloc::Layout::from_size_align(DMA_BYTES, 4096).unwrap();
        // SAFETY: a non-zero layout; the memory is leaked for the test.
        let base = unsafe { std::alloc::alloc_zeroed(layout) };
        let txcfg: u32 = match chip {
            Chip::R25 => 0x6090_0000,
            Chip::R25B => 0x6410_0000,
            Chip::R25D(1) => 0x6880_0000,
            Chip::R25D(_) => 0x6890_0000,
            Chip::R26(1) => 0x6490_0000,
            Chip::R26(_) => 0x64a0_0000,
        };
        let mut d = Dev {
            base,
            regs: vec![0; 0x8000],
            mac_ocp: HashMap::new(),
            phy: HashMap::new(),
            csi: HashMap::new(),
            config: HashMap::new(),
            mac_ocp_addr: 0,
            phy_readback: 0,
            csi_readback: 0,
            now: 0,
            cable: true,
            partner_pause: true,
            an_done_at: 0,
            an_restarts: 0,
            tx_enabled: false,
            rx_enabled: false,
            resets: 0,
            reset_stuck: false,
            phy_stuck: false,
            tx_stalled: false,
            tx_idx: 0,
            rx_idx: 0,
            wire: Vec::new(),
            loopback: false,
            dropped: 0,
            ops: Vec::new(),
            early_doorbells: 0,
        };
        d.put32(rtl::TXCFG, txcfg | 0x0300_0700);
        d.put32(rtl::ADDR0, u32::from_le_bytes([MAC[0], MAC[1], MAC[2], MAC[3]]));
        d.put16(rtl::ADDR1, u16::from_le_bytes([MAC[4], MAC[5]]));
        for (i, b) in MAC.iter().enumerate() { d.regs[i] = *b; }
        d.phy.insert(rtl::PHY_STATE, 3);
        d.phy.insert(mii(rtl::MII_BMCR), 0x1040);
        d.phy.insert(mii(rtl::MII_ANAR), FIRMWARE_ANAR);
        d.phy.insert(mii(rtl::MII_GTCR), 0x0300);
        d.phy.insert(rtl::PHY_ADV, 0x0080);
        d.phy.insert(0xc416, 0x0500);
        d.config.insert(0x70c, 0x1700_0000);
        d
    }

    pub fn dma(&self) -> Dma { Dma { host: self.base, device: DEVICE_BASE } }

    /// A Net over this device's region.
    pub fn net(&self, chip: Chip) -> Net {
        // SAFETY: the region is DMA_BYTES, leaked, used only by this Net and
        // the simulated DMA below.
        unsafe { Net::new(self.dma(), chip, MAC) }
    }

    fn put16(&mut self, r: u32, v: u16) { self.regs[r as usize..r as usize + 2].copy_from_slice(&v.to_le_bytes()); }
    fn put32(&mut self, r: u32, v: u32) { self.regs[r as usize..r as usize + 4].copy_from_slice(&v.to_le_bytes()); }
    pub fn get8(&self, r: u32) -> u8 { self.regs[r as usize] }
    pub fn get16(&self, r: u32) -> u16 { u16::from_le_bytes([self.regs[r as usize], self.regs[r as usize + 1]]) }
    pub fn get32(&self, r: u32) -> u32 {
        let r = r as usize;
        u32::from_le_bytes([self.regs[r], self.regs[r + 1], self.regs[r + 2], self.regs[r + 3]])
    }

    fn host(&self, device: u64, len: usize) -> *mut u8 {
        let off = device.checked_sub(DEVICE_BASE).expect("DMA below the region") as usize;
        assert!(off + len <= DMA_BYTES, "DMA past the region: {off:#x}+{len}");
        // SAFETY: inside the region (checked).
        unsafe { self.base.add(off) }
    }
    pub fn mem32(&self, device: u64) -> u32 {
        // SAFETY: inside the region.
        u32::from_le(unsafe { std::ptr::read_volatile(self.host(device, 4) as *const u32) })
    }
    fn set_mem32(&self, device: u64, v: u32) {
        // SAFETY: inside the region.
        unsafe { std::ptr::write_volatile(self.host(device, 4) as *mut u32, v.to_le()) }
    }
    fn mem64(&self, device: u64) -> u64 { u64::from(self.mem32(device)) | u64::from(self.mem32(device + 4)) << 32 }

    pub fn rx_ring(&self) -> u64 { u64::from(self.get32(rtl::RXDESC_LO)) | u64::from(self.get32(rtl::RXDESC_HI)) << 32 }
    pub fn tx_ring(&self) -> u64 { u64::from(self.get32(rtl::TXDESC_LO)) | u64::from(self.get32(rtl::TXDESC_HI)) << 32 }

    fn advertised_pause(&self) -> bool { self.phy.get(&mii(rtl::MII_ANAR)).copied().unwrap_or(0) & rtl::ANAR_PAUSE != 0 }

    pub fn link_up(&self) -> bool { self.cable && self.now >= self.an_done_at }

    fn phystat(&self) -> u16 {
        if !self.link_up() { return 0; }
        let adv = self.phy.get(&rtl::PHY_ADV).copied().unwrap_or(0);
        let speed = if adv & rtl::ADV_5000 != 0 { rtl::PHYSTAT_5000 }
            else if adv & rtl::ADV_2500 != 0 { rtl::PHYSTAT_2500 } else { rtl::PHYSTAT_1000 };
        let pause = if self.advertised_pause() && self.partner_pause { rtl::PHYSTAT_RXFLOW | rtl::PHYSTAT_TXFLOW } else { 0 };
        rtl::PHYSTAT_LINK | rtl::PHYSTAT_FDX | speed | pause
    }

    fn phy_write(&mut self, reg: u16, v: u16) {
        if reg == mii(rtl::MII_BMCR) && v & (rtl::BMCR_RESET | rtl::BMCR_STARTNEG) != 0 {
            self.an_restarts += 1;
            self.an_done_at = self.now + AN_US;
            self.phy.insert(reg, v & !(rtl::BMCR_RESET | rtl::BMCR_STARTNEG));
            return;
        }
        if reg == 0xb820 {
            // PHY-patch mode request: 0xb800 bit 6 follows it.
            let ready = if v & 0x0010 != 0 { 0x0040 } else { 0 };
            self.phy.insert(0xb800, ready);
        }
        self.phy.insert(reg, v);
    }

    fn cmd_write(&mut self, v: u8) {
        if v & rtl::CMD_RESET != 0 {
            self.resets += 1;
            self.tx_enabled = false;
            self.rx_enabled = false;
            self.tx_idx = 0;
            self.rx_idx = 0;
            self.regs[rtl::CMD as usize] = if self.reset_stuck { rtl::CMD_RESET } else { 0 };
            return;
        }
        self.tx_enabled = v & rtl::CMD_TXENB != 0;
        self.rx_enabled = v & rtl::CMD_RXENB != 0;
        self.regs[rtl::CMD as usize] = v;
    }

    /// The doorbell: send every descriptor the chip owns from its index.
    fn transmit(&mut self) {
        if !self.tx_enabled || self.tx_stalled { return; }
        let ring = self.tx_ring();
        let mut sent = 0;
        loop {
            let d = ring + 16 * self.tx_idx as u64;
            let s = self.mem32(d);
            if s & ring::OWN == 0 { break; }
            assert_eq!(s & (ring::TX_SOF | ring::TX_EOF), ring::TX_SOF | ring::TX_EOF, "one buffer per frame");
            let len = (s & 0xffff) as usize;
            let addr = self.mem64(d + 8);
            let p = self.host(addr, len);
            // SAFETY: inside the region (checked by `host`).
            let f = unsafe { std::slice::from_raw_parts(p, len) }.to_vec();
            self.set_mem32(d, s & !ring::OWN);
            self.tx_idx = if s & ring::EOR != 0 { 0 } else { self.tx_idx + 1 };
            sent += 1;
            if self.loopback { self.deliver(&f); }
            self.wire.push(f);
            assert!(sent <= ring::TX_COUNT, "the ring has no EOR");
        }
        if sent == 0 { self.early_doorbells += 1; }
    }

    /// A frame from the network (CRC appended, as the MAC stores it).
    pub fn deliver(&mut self, f: &[u8]) {
        if !self.rx_enabled { self.dropped += 1; return; }
        let d = self.rx_ring() + 32 * self.rx_idx as u64;
        let s = self.mem32(d + ring::RX_CMDSTS as u64);
        if s & ring::OWN == 0 { self.dropped += 1; return; }
        let size = (s & ring::RX_LEN) as usize;
        let len = f.len() + 4;
        assert!(len <= size, "frame longer than the posted buffer");
        let addr = self.mem64(d + ring::RX_ADDR as u64);
        let p = self.host(addr, len);
        // SAFETY: inside the region.
        unsafe {
            std::ptr::copy_nonoverlapping(f.as_ptr(), p, f.len());
            std::ptr::copy_nonoverlapping([0xde, 0xad, 0xbe, 0xef].as_ptr(), p.add(f.len()), 4);
        }
        self.set_mem32(d + ring::RX_CMDSTS as u64, (s & ring::EOR) | ring::RX_SOF | ring::RX_EOF | len as u32);
        self.rx_idx = if s & ring::EOR != 0 { 0 } else { self.rx_idx + 1 };
    }

    /// Hand back the next receive descriptor with these status bits.
    pub fn deliver_raw(&mut self, status: u32) {
        let d = self.rx_ring() + 32 * self.rx_idx as u64;
        let s = self.mem32(d + ring::RX_CMDSTS as u64);
        assert!(s & ring::OWN != 0);
        self.set_mem32(d + ring::RX_CMDSTS as u64, (s & ring::EOR) | status);
        self.rx_idx = if s & ring::EOR != 0 { 0 } else { self.rx_idx + 1 };
    }

    fn write(&mut self, offset: u32, width: u8, v: u32) {
        self.ops.push((offset, v));
        match (offset, width) {
            (rtl::CMD, 1) => self.cmd_write(v as u8),
            (rtl::TXSTART, 2) => self.transmit(),
            (rtl::MACOCP, 4) => {
                let addr = ((v >> 16) & 0x7fff) as u16 * 2;
                if v & rtl::BUSY != 0 { self.mac_ocp.insert(addr, v as u16); } else { self.mac_ocp_addr = addr; }
            }
            (rtl::PHYOCP, 4) => {
                let addr = ((v >> 16) & 0x7fff) as u16 * 2;
                if self.phy_stuck {
                    self.phy_readback = if v & rtl::BUSY != 0 { rtl::BUSY } else { 0 };
                } else if v & rtl::BUSY != 0 {
                    self.phy_write(addr, v as u16);
                    self.phy_readback = 0;
                } else {
                    self.phy_readback = rtl::BUSY | u32::from(self.phy.get(&addr).copied().unwrap_or(0));
                }
            }
            (rtl::CSIAR, 4) => {
                let addr = v & 0xfff;
                if v & rtl::BUSY != 0 {
                    let data = self.get32(rtl::CSIDR);
                    self.csi.insert(addr, data);
                    self.csi_readback = 0;
                } else {
                    let data = self.csi.get(&addr).copied().unwrap_or(0);
                    self.put32(rtl::CSIDR, data);
                    self.csi_readback = rtl::BUSY;
                }
            }
            (rtl::ISR, 4) => { let now = self.get32(rtl::ISR); self.put32(rtl::ISR, now & !v); }
            _ => {
                let o = offset as usize;
                self.regs[o..o + width as usize].copy_from_slice(&v.to_le_bytes()[..width as usize]);
            }
        }
    }

    fn read(&mut self, offset: u32, width: u8) -> u32 {
        match (offset, width) {
            (rtl::MCUCMD, 1) => u32::from(self.get8(rtl::MCUCMD) | rtl::MCUCMD_FIFOS_EMPTY),
            (rtl::IM, 2) => u32::from(self.get16(rtl::IM) | 0x0103),
            (rtl::TWICMD, 2) => u32::from(self.get16(rtl::TWICMD) | 0x0200),
            (rtl::PHYSTAT, 2) => u32::from(self.phystat()),
            (rtl::MACOCP, 4) => u32::from(self.mac_ocp_addr / 2) << 16 | u32::from(self.mac_ocp.get(&self.mac_ocp_addr).copied().unwrap_or(0)),
            (rtl::PHYOCP, 4) => self.phy_readback,
            (rtl::CSIAR, 4) => self.csi_readback,
            _ => {
                let o = offset as usize;
                let mut b = [0u8; 4];
                b[..width as usize].copy_from_slice(&self.regs[o..o + width as usize]);
                u32::from_le_bytes(b)
            }
        }
    }

    /// Writes to `offset` so far, in order.
    pub fn writes(&self, offset: u32) -> Vec<u32> {
        self.ops.iter().filter(|o| o.0 == offset).map(|o| o.1).collect()
    }
}

pub fn mii(reg: u16) -> u16 { rtl::PHY_MII_BASE + 2 * reg }

impl Bus for Dev {
    type Error = ();
    fn r8(&mut self, o: u32) -> Result<u8, ()> { Ok(self.read(o, 1) as u8) }
    fn r16(&mut self, o: u32) -> Result<u16, ()> { Ok(self.read(o, 2) as u16) }
    fn r32(&mut self, o: u32) -> Result<u32, ()> { Ok(self.read(o, 4)) }
    fn w8(&mut self, o: u32, v: u8) -> Result<(), ()> { self.write(o, 1, v.into()); Ok(()) }
    fn w16(&mut self, o: u32, v: u16) -> Result<(), ()> { self.write(o, 2, v.into()); Ok(()) }
    fn w32(&mut self, o: u32, v: u32) -> Result<(), ()> { self.write(o, 4, v); Ok(()) }
    fn config_r32(&mut self, o: u32) -> Result<u32, ()> { self.config.get(&o).copied().ok_or(()) }
    fn config_w32(&mut self, o: u32, v: u32) -> Result<(), ()> { self.config.insert(o, v); Ok(()) }
    fn delay_us(&mut self, us: usize) { self.now += us as u64; }
}

/// A chip and a Net brought up on it.
pub fn started(chip: Chip) -> (Dev, Net) {
    let mut d = Dev::new(chip);
    let mut net = d.net(chip);
    net.start(&mut d, false).unwrap();
    (d, net)
}

/// A minimum-size frame to `dst`, its payload tagged with `tag`.
pub fn frame_to(dst: [u8; 6], tag: u8) -> Vec<u8> {
    let mut f = vec![0u8; 60];
    f[0..6].copy_from_slice(&dst);
    f[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 9]);
    f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    f[14] = tag;
    f
}
