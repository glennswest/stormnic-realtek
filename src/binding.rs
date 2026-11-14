//! `EFI_DRIVER_BINDING_PROTOCOL`: which controllers this driver takes.
//! The same glue as stormnic-virtio's, for the Realtek RTL8125/8126.
//!
//! Supported looks at every PCI function the firmware offers. It reads the
//! vendor/device ID with a non-exclusive (GET_PROTOCOL) open and, for one of
//! ours, tries the BY_DRIVER open that Start will hold. A platform driver
//! that already owns the NIC (the O6N firmware's own Realtek driver) holds
//! that open, so ours fails and the platform's driver wins; stormbootx's
//! `prefer_media_drivers = realtek` disconnects it first.
//!
//! Start holds `EFI_PCI_IO_PROTOCOL` BY_DRIVER | EXCLUSIVE (`Held`), enables
//! memory decode and bus mastering (`decode`), finds the register BAR,
//! identifies the chip (`rtl::identify`), maps one DMA region, brings the
//! NIC up once (which takes it from the management firmware and makes the
//! PHY advertise without pause, #2), runs the DMA check when the link is up
//! and quiesces it again. Last it makes the SNP child handle (`snp`): nothing
//! DMAs until the SNP's user initializes it. Every failure after the chip is
//! identified quiesces it before Start returns.
//!
//! Every Realtek network function Supported sees is logged (trace), matched
//! or not, so a verbose boot names what the machine really has.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::time::Duration;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::mem::memory_map::MemoryType;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{Handle, Identify, Result, Status};
use uefi_raw::protocol::device_path::DevicePathProtocol;
use uefi_raw::protocol::driver::DriverBindingProtocol;

use crate::console;
use crate::decode::{self, Enabled};
use crate::ids::{self, Nic};
use crate::pci_io::{Location, PciIo, Width};
use crate::rtl::ring::{self, Dma, Net};
use crate::rtl::{self, Bus, Chip, Identity, Link};
use crate::snp;

/// The chip's registers through a PciIo memory BAR, and its config space.
pub struct Bars<'a> {
    pub pci: &'a PciIo,
    pub bar: u8,
}

impl Bus for Bars<'_> {
    type Error = Status;
    fn r8(&mut self, offset: u32) -> core::result::Result<u8, Status> {
        self.pci.mem_read(Width::U8, self.bar, offset).map(|v| v as u8).map_err(|e| e.status())
    }
    fn r16(&mut self, offset: u32) -> core::result::Result<u16, Status> {
        self.pci.mem_read(Width::U16, self.bar, offset).map(|v| v as u16).map_err(|e| e.status())
    }
    fn r32(&mut self, offset: u32) -> core::result::Result<u32, Status> {
        self.pci.mem_read(Width::U32, self.bar, offset).map_err(|e| e.status())
    }
    fn w8(&mut self, offset: u32, value: u8) -> core::result::Result<(), Status> {
        self.pci.mem_write(Width::U8, self.bar, offset, value.into()).map_err(|e| e.status())
    }
    fn w16(&mut self, offset: u32, value: u16) -> core::result::Result<(), Status> {
        self.pci.mem_write(Width::U16, self.bar, offset, value.into()).map_err(|e| e.status())
    }
    fn w32(&mut self, offset: u32, value: u32) -> core::result::Result<(), Status> {
        self.pci.mem_write(Width::U32, self.bar, offset, value).map_err(|e| e.status())
    }
    fn config_r32(&mut self, offset: u32) -> core::result::Result<u32, Status> {
        self.pci.config_read_u32(offset).map_err(|e| e.status())
    }
    fn config_w32(&mut self, offset: u32, value: u32) -> core::result::Result<(), Status> {
        self.pci.config_write_u32(offset, value).map_err(|e| e.status())
    }
    fn delay_us(&mut self, micros: usize) {
        boot::stall(Duration::from_micros(micros as u64));
    }
}

/// The register BAR: rge(4) maps BAR 2 (64-bit memory, config 0x18) and
/// falls back to BAR 1 (32-bit memory, 0x14). PciIo refuses a memory access
/// to an I/O or missing BAR, so the first BAR whose TXCFG reads is it.
fn register_bar(pci: &PciIo) -> Option<u8> {
    [2u8, 1].into_iter().find(|&bar| pci.mem_read(Width::U32, bar, rtl::TXCFG).is_ok())
}

/// A controller this driver has started.
struct Bound {
    controller: Handle,
    location: Option<Location>,
    /// How Start enabled memory decode and bus mastering; Stop undoes it.
    decode: Enabled<Status>,
    /// The rings and buffers, mapped for DMA.
    dma: DmaRegion,
    /// The SNP, its child handle and the rings (`snp::create`).
    port: *mut snp::Port,
    /// The BY_DRIVER | EXCLUSIVE open; dropping it closes the protocol.
    pci: Held,
}

/// `ring::DMA_PAGES` from AllocateBuffer, mapped as one common buffer.
struct DmaRegion {
    host: *mut u8,
    device: u64,
    mapping: *mut c_void,
}

impl DmaRegion {
    fn new(pci: &PciIo) -> Result<Self> {
        let host = pci.allocate_buffer(ring::DMA_PAGES)?;
        // SAFETY: `host` is DMA_PAGES pages from AllocateBuffer.
        match unsafe { pci.map_common(host, ring::DMA_PAGES * 4096) } {
            Ok((device, mapping)) => Ok(DmaRegion { host, device, mapping }),
            Err(e) => {
                // SAFETY: allocated above, never mapped or used.
                let _ = unsafe { pci.free_buffer(ring::DMA_PAGES, host) };
                Err(e)
            }
        }
    }

    /// Unmap and free. Only once the NIC is quiesced.
    fn release(self, pci: &PciIo) {
        // SAFETY: the caller quiesced the NIC, so it no longer reads or
        // writes the region; nothing else holds the pointer after this.
        unsafe {
            if let Err(e) = pci.unmap(self.mapping) {
                say!("stormnic-realtek: could not unmap the DMA region: {:?}", e.status());
            }
            if let Err(e) = pci.free_buffer(ring::DMA_PAGES, self.host) {
                say!("stormnic-realtek: could not free the DMA region: {:?}", e.status());
            }
        }
    }
}

#[derive(Default)]
pub struct RealtekDriver {
    bound: Vec<Bound>,
}

impl RealtekDriver {
    pub const fn new() -> Self {
        RealtekDriver { bound: Vec::new() }
    }
}

/// What config space says about a function.
struct Ident {
    vendor: u16,
    device: u16,
    class: u8,
}

fn ident(pci: &PciIo) -> Result<Ident> {
    let id = pci.config_read_u32(0x00)?;
    let class = pci.config_read_u32(0x08)?;
    Ok(Ident { vendor: id as u16, device: (id >> 16) as u16, class: (class >> 24) as u8 })
}

/// `EFI_OPEN_PROTOCOL_BY_DRIVER | EFI_OPEN_PROTOCOL_EXCLUSIVE`.
const BY_DRIVER_EXCLUSIVE: u32 = 0x30;

/// Start's PciIo open, BY_DRIVER | EXCLUSIVE, closed on drop.
///
/// EXCLUSIVE because ConnectController goes on trying the other drivers
/// after ours has started, and one that opens PciIo EXCLUSIVE itself (QEMU's
/// iPXE option ROM did, on a pve NIC for stormnic-virtio) would otherwise force ours off the
/// function (#1, the second pve run). It never displaces a driver already
/// there: Supported declines any function held BY_DRIVER.
pub struct Held {
    iface: *const PciIo,
    agent: Handle,
    controller: Handle,
}

impl core::ops::Deref for Held {
    type Target = PciIo;
    fn deref(&self) -> &PciIo {
        // SAFETY: the open interface, valid until `drop` closes it (the
        // firmware calls Stop before it would remove it).
        unsafe { &*self.iface }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: closes the open made in `hold`.
        let _ = unsafe {
            (crate::snp::bs().close_protocol)(self.controller.as_ptr(), &PciIo::GUID, self.agent.as_ptr(), self.controller.as_ptr())
        };
    }
}

fn hold(agent: Handle, controller: Handle) -> Result<Held> {
    let mut iface: *mut c_void = core::ptr::null_mut();
    // SAFETY: an open tracked by the firmware, closed by Held's drop.
    let st = unsafe {
        (crate::snp::bs().open_protocol)(controller.as_ptr(), &PciIo::GUID, &mut iface, agent.as_ptr(),
            controller.as_ptr(), BY_DRIVER_EXCLUSIVE)
    };
    if st.is_error() || iface.is_null() {
        return Err(if st.is_error() { st } else { Status::DEVICE_ERROR }.into());
    }
    Ok(Held { iface: iface as *const PciIo, agent, controller })
}

fn open(agent: Handle, controller: Handle, attrs: OpenProtocolAttributes) -> Result<ScopedProtocol<PciIo>> {
    let params = OpenProtocolParams { handle: controller, agent, controller: Some(controller) };
    // SAFETY: GET_PROTOCOL opens are dropped before Supported/Start return;
    // a BY_DRIVER open is tracked by the firmware, which calls Stop before
    // it removes the interface.
    unsafe { boot::open_protocol::<PciIo>(params, attrs) }
}

pub fn at(location: Option<Location>) -> impl core::fmt::Display {
    struct At(Option<Location>);
    impl core::fmt::Display for At {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self.0 {
                Some(l) => write!(f, "{l}"),
                None => f.write_str("(location unknown)"),
            }
        }
    }
    At(location)
}

pub fn mac_str(m: [u8; 6]) -> impl core::fmt::Display {
    struct M([u8; 6]);
    impl core::fmt::Display for M {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            let m = self.0;
            write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
        }
    }
    M(m)
}

fn rtl_error(e: &rtl::Error<Status>) -> impl core::fmt::Display + '_ {
    struct D<'a>(&'a rtl::Error<Status>);
    impl core::fmt::Display for D<'_> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self.0 {
                rtl::Error::Io(s) => write!(f, "register access failed: {s:?}"),
                rtl::Error::Removed => f.write_str("registers read all ones (function gone or BAR not decoded)"),
                rtl::Error::UnknownChip { txcfg } => write!(
                    f, "unknown MAC revision (TXCFG {txcfg:#010x}, HWREV {:#05x}); RTL8127 and newer are not supported",
                    (txcfg & rtl::TXCFG_HWREV) >> 20
                ),
                rtl::Error::NoMac => f.write_str("no unicast station address in 0x19e0 or MAC0"),
                rtl::Error::ResetTimeout { cmd } => write!(f, "MAC reset did not complete (CMD {cmd:#04x})"),
                rtl::Error::PhyTimeout { reg } => write!(f, "PHY OCP access to {reg:#06x} timed out"),
                rtl::Error::Frame { len } => write!(f, "bad frame length {len}"),
            }
        }
    }
    D(e)
}

impl RealtekDriver {
    fn is_bound(&self, controller: Handle) -> bool {
        self.bound.iter().any(|b| b.controller == controller)
    }

    /// The controller's entry, if it is one of ours.
    fn ours(&self, agent: Handle, controller: Handle) -> Result<(&'static Nic, Option<Location>)> {
        // No PciIo: not a PCI function; the common case, and silent.
        let pci = open(agent, controller, OpenProtocolAttributes::GetProtocol)?;
        let id = ident(&pci)?;
        if id.vendor != rtl::VENDOR || id.class != ids::CLASS_NETWORK {
            return Err(Status::UNSUPPORTED.into());
        }
        let location = pci.location().ok();
        let Some(nic) = ids::lookup(id.vendor, id.device) else {
            trace!("stormnic-realtek: {} 10ec:{:04x}: Realtek network function, not one this driver binds", at(location), id.device);
            return Err(Status::UNSUPPORTED.into());
        };
        Ok((nic, location))
    }

    fn supported(&mut self, agent: Handle, controller: Handle) -> Result {
        if self.is_bound(controller) {
            return Err(Status::ALREADY_STARTED.into());
        }
        let (nic, location) = self.ours(agent, controller)?;
        match open(agent, controller, OpenProtocolAttributes::ByDriver) {
            Ok(_pci) => {
                trace!("stormnic-realtek: {} 10ec:{:04x} {}: Supported", at(location), nic.device, nic.name);
                Ok(())
            }
            Err(e) => {
                trace!(
                    "stormnic-realtek: {} 10ec:{:04x} {}: already driven by another driver ({:?}); leaving it",
                    at(location), nic.device, nic.name, e.status()
                );
                Err(e)
            }
        }
    }

    fn start(&mut self, agent: Handle, controller: Handle) -> Result {
        console::begin();
        let (nic, location) = self.ours(agent, controller)?;
        let (at, dev) = (at(location), nic.device);
        let pci = match hold(agent, controller) {
            Ok(pci) => pci,
            Err(e) => {
                fail!("stormnic-realtek: {at} 10ec:{dev:04x}: Start could not open PciIo BY_DRIVER | EXCLUSIVE: {:?}", e.status());
                return Err(e);
            }
        };
        let attributes = match decode::enable(&*pci) {
            Ok(e) => e,
            Err(decode::Error::Config(status)) => {
                fail!("stormnic-realtek: {at} 10ec:{dev:04x}: Start could not enable memory decode and bus mastering: command register access failed: {status:?}");
                return Err(Status::UNSUPPORTED.into());
            }
            Err(decode::Error::NotSet(command, _)) => {
                fail!("stormnic-realtek: {at} 10ec:{dev:04x}: Start could not enable memory decode and bus mastering: command {command:#06x}");
                return Err(Status::UNSUPPORTED.into());
            }
        };
        let Some(bar) = register_bar(&pci) else {
            fail!("stormnic-realtek: {at} 10ec:{dev:04x}: no memory BAR (2 or 1) answers; releasing");
            restore(&pci, &attributes);
            return Err(Status::UNSUPPORTED.into());
        };
        let mut bars = Bars { pci: &pci, bar };
        let id = match rtl::identify(&mut bars) {
            Ok(id) => id,
            Err(e) => {
                // Not quiesced: an unknown chip's sequences are not known.
                fail!("stormnic-realtek: {at} 10ec:{dev:04x} {}: {}; releasing", nic.name, rtl_error(&e));
                restore(&pci, &attributes);
                return Err(Status::UNSUPPORTED.into());
            }
        };
        trace!(
            "stormnic-realtek: {at} 10ec:{dev:04x}: BAR {bar}, {} (TXCFG {:#010x}), MAC {}, link {} before reset",
            id.chip, id.txcfg, mac_str(id.mac), id.link
        );
        let (dma, net, link, waited_ms) = match dma_up(&pci, bar, &at, dev, &id) {
            Ok(r) => r,
            Err(()) => {
                restore(&pci, &attributes);
                return Err(Status::DEVICE_ERROR.into());
            }
        };
        let port = match snp::create(agent.as_ptr(), controller.as_ptr(), &pci, bar, net, link, location) {
            Ok(p) => p,
            Err((status, in_use)) => {
                fail!("stormnic-realtek: {at} 10ec:{dev:04x}: SNP not installed: {status:?}; releasing");
                if in_use {
                    keep_dma(&pci, &at);
                } else {
                    dma.release(&pci);
                }
                restore(&pci, &attributes);
                return Err(Status::DEVICE_ERROR.into());
            }
        };
        self.bound.push(Bound { controller, location, decode: attributes, dma, port, pci });
        // The one default line per port (stormnic-ixgbe#22), in the stormnic
        // contract's form (#5): driver, PCI address, MAC, media, speed, FEC,
        // link; a port without link is an error line.
        say!(
            "{}",
            rtl::PortLine {
                version: env!("CARGO_PKG_VERSION"), at: &at, device: dev, chip: id.chip,
                mac: id.mac, link, waited_ms,
            }
        );
        trace!("stormnic-realtek: {at} 10ec:{dev:04x}: link {link}, SNP installed");
        console::begin();
        Ok(())
    }

    /// With `children`, remove the SNP child; without, release the NIC.
    fn stop(&mut self, agent: Handle, controller: Handle, children: &[uefi_raw::Handle]) -> Result {
        let Some(i) = self.bound.iter().position(|b| b.controller == controller) else {
            say!("stormnic-realtek: Stop for a controller this driver never started");
            return Err(Status::DEVICE_ERROR.into());
        };
        let (port, location) = (self.bound[i].port, self.bound[i].location);
        // SAFETY: `port` is live until `snp::destroy` below.
        let child = unsafe { (*port).child };
        if !children.is_empty() || child.is_some() {
            if child.is_some_and(|c| children.is_empty() || children.contains(&c)) {
                // SAFETY: as above.
                if let Err(s) = unsafe { snp::remove_child(agent.as_ptr(), controller.as_ptr(), port) } {
                    say!("stormnic-realtek: {}: Stop: SNP child still in use ({s:?}); kept", at(location));
                    return Err(Status::DEVICE_ERROR.into());
                }
                trace!("stormnic-realtek: {}: Stop: SNP child removed", at(location));
            }
            if !children.is_empty() {
                return Ok(());
            }
        }
        let b = self.bound.swap_remove(i);
        // SAFETY: the child is gone, so nothing else reaches the Port.
        match unsafe { snp::destroy(b.port) } {
            Ok(()) => b.dma.release(&b.pci),
            Err(_) => keep_dma(&b.pci, &at(b.location)),
        }
        restore(&b.pci, &b.decode);
        trace!("stormnic-realtek: {}: Stop: released", at(b.location));
        Ok(())
    }
}

/// Map the region, bring the NIC up once (OOB exit, MAC init, the PHY's
/// advertisement without pause), wait for the link if autonegotiation was
/// restarted, run the DMA check when the link is up, and quiesce the NIC.
/// Everything is logged; Err(()) means Start fails (the region is released,
/// or kept with bus mastering off if the NIC would not quiesce).
fn dma_up(pci: &PciIo, bar: u8, at: &impl core::fmt::Display, dev: u16, id: &Identity)
    -> core::result::Result<(DmaRegion, Net, Link, u32), ()> {
    let mut bars = Bars { pci, bar };
    let region = match DmaRegion::new(pci) {
        Ok(r) => r,
        Err(e) => {
            fail!("stormnic-realtek: {at} 10ec:{dev:04x}: DMA region not mapped: {:?}; releasing", e.status());
            let _ = rtl::quiesce(&mut bars, id.chip);
            return Err(());
        }
    };
    // SAFETY: the region is DMA_BYTES (rounded to pages) from AllocateBuffer,
    // mapped at `device`, and only this Net uses it until it is released.
    let mut net = unsafe { Net::new(Dma { host: region.host, device: region.device }, id.chip, id.mac) };
    let checked = bring_up(&mut bars, &mut net, id.chip, at, dev, region.device);
    if let Err(e) = &checked {
        fail!("stormnic-realtek: {at} 10ec:{dev:04x}: bring-up failed: {}", rtl_error(e));
    }
    match net.stop(&mut bars) {
        Ok(()) => match checked {
            Ok((link, waited_ms)) => Ok((region, net, link, waited_ms)),
            Err(_) => {
                region.release(pci);
                say!("stormnic-realtek: {at} 10ec:{dev:04x}: NIC quiesced, DMA region released; releasing");
                Err(())
            }
        },
        Err(e) => {
            fail!("stormnic-realtek: {at} 10ec:{dev:04x}: quiesce failed: {}", rtl_error(&e));
            keep_dma(pci, at);
            Err(())
        }
    }
}

fn bring_up(bars: &mut Bars, net: &mut Net, chip: Chip, at: &impl core::fmt::Display, dev: u16, device: u64)
    -> core::result::Result<(Link, u32), rtl::Error<Status>> {
    let pause = console::pause();
    net.start(bars, false)?;
    let before = rtl::advertised(bars)?;
    let restarted = rtl::advertise(bars, chip, pause)?;
    if restarted {
        let after = rtl::wanted(chip, before, pause);
        trace!(
            "stormnic-realtek: {at} 10ec:{dev:04x}: PHY advertisement ANAR {:#06x} -> {:#06x}, 2.5G/5G {:#06x} -> {:#06x}; autonegotiation restarted",
            before.anar, after.anar, before.adv, after.adv
        );
    } else {
        trace!("stormnic-realtek: {at} 10ec:{dev:04x}: PHY advertisement already right (ANAR {:#06x})", before.anar);
    }
    let waited_ms = if restarted { 5000 } else { 0 };
    let link = rtl::wait_link(bars, waited_ms as usize)?;
    trace!(
        "stormnic-realtek: {at} 10ec:{dev:04x}: DMA: {} pages at device {device:#x}, rings RX {} / TX {} x {} B; link {link}",
        ring::DMA_PAGES, ring::RX_COUNT, ring::TX_COUNT, ring::BUF_SIZE
    );
    if link.tx_pause && !pause {
        say!("stormnic-realtek: {at} 10ec:{dev:04x}: warning: the link resolved to TX pause although none is advertised");
    }
    if link.up {
        match ring::check(bars, net)? {
            Some(ms) => trace!("stormnic-realtek: {at} 10ec:{dev:04x}: DMA check: broadcast frame sent, descriptor back after {ms} ms"),
            None => say!("stormnic-realtek: {at} 10ec:{dev:04x}: DMA check: the NIC did not return the frame's descriptor within 100 ms"),
        }
    } else {
        trace!("stormnic-realtek: {at} 10ec:{dev:04x}: DMA check skipped: link down");
    }
    Ok((link, waited_ms))
}

/// The NIC would not quiesce: it might still DMA into the region, so it is
/// never freed. Bus mastering is turned off at the PCI level instead; the
/// pages stay allocated until reboot.
fn keep_dma(pci: &PciIo, at: &impl core::fmt::Display) {
    let off = decode::stop_bus_master(pci);
    fail!(
        "stormnic-realtek: {at}: could not quiesce the NIC; bus mastering {}, DMA region kept allocated",
        if off.is_ok() { "disabled" } else { "could not be disabled" }
    );
}

fn restore(pci: &PciIo, e: &Enabled<Status>) {
    if let Err(s) = decode::release(pci, e) {
        say!("stormnic-realtek: could not undo the PCI decode and bus-master changes: {s:?}");
    }
}

/// The driver binding interface and the driver behind it. `protocol` first:
/// firmware's `This` is the Binding.
#[repr(C)]
struct Binding {
    protocol: DriverBindingProtocol,
    driver: RealtekDriver,
}

/// # Safety
/// `this` is the `protocol` of the leaked Binding; the firmware serializes
/// binding calls.
unsafe fn binding<'a>(this: *const DriverBindingProtocol) -> &'a mut Binding {
    unsafe { &mut *(this as *mut Binding) }
}

fn handles(this: *const DriverBindingProtocol, controller: uefi_raw::Handle) -> Option<(Handle, Handle)> {
    if this.is_null() { return None; }
    // SAFETY: the firmware passes our interface and a controller handle.
    unsafe {
        let agent = Handle::from_ptr((*this).driver_binding_handle)?;
        Some((agent, Handle::from_ptr(controller)?))
    }
}

unsafe extern "efiapi" fn binding_supported(this: *const DriverBindingProtocol, controller: uefi_raw::Handle,
    _remaining: *const DevicePathProtocol) -> Status {
    let Some((agent, controller)) = handles(this, controller) else { return Status::INVALID_PARAMETER };
    match unsafe { binding(this) }.driver.supported(agent, controller) {
        Ok(()) => Status::SUCCESS,
        Err(e) => e.status(),
    }
}

unsafe extern "efiapi" fn binding_start(this: *const DriverBindingProtocol, controller: uefi_raw::Handle,
    _remaining: *const DevicePathProtocol) -> Status {
    let Some((agent, controller)) = handles(this, controller) else { return Status::INVALID_PARAMETER };
    match unsafe { binding(this) }.driver.start(agent, controller) {
        Ok(()) => Status::SUCCESS,
        Err(e) => e.status(),
    }
}

unsafe extern "efiapi" fn binding_stop(this: *const DriverBindingProtocol, controller: uefi_raw::Handle,
    count: usize, children: *const uefi_raw::Handle) -> Status {
    let Some((agent, controller)) = handles(this, controller) else { return Status::INVALID_PARAMETER };
    if count > 0 && children.is_null() { return Status::INVALID_PARAMETER; }
    let children = if count == 0 { &[][..] } else {
        // SAFETY: the firmware passes `count` child handles.
        unsafe { core::slice::from_raw_parts(children, count) }
    };
    match unsafe { binding(this) }.driver.stop(agent, controller, children) {
        Ok(()) => Status::SUCCESS,
        Err(e) => e.status(),
    }
}

/// Install `EFI_DRIVER_BINDING_PROTOCOL` on the image handle. The image
/// must have been loaded as a boot-service driver (its code and data stay
/// after the entry point returns).
pub fn install() -> Result {
    let image = boot::image_handle();
    {
        let loaded = boot::open_protocol_exclusive::<LoadedImage>(image)?;
        if loaded.code_type() != MemoryType::BOOT_SERVICES_CODE || loaded.data_type() != MemoryType::BOOT_SERVICES_DATA {
            return Err(Status::UNSUPPORTED.into());
        }
    }
    let b = Box::into_raw(Box::new(Binding {
        protocol: DriverBindingProtocol {
            supported: binding_supported,
            start: binding_start,
            stop: binding_stop,
            version: 1,
            image_handle: image.as_ptr(),
            driver_binding_handle: image.as_ptr(),
        },
        driver: RealtekDriver::new(),
    }));
    // SAFETY: the Binding is leaked, so the interface lives as long as the
    // image; the GUID matches the interface.
    let r = unsafe {
        boot::install_protocol_interface(Some(image), &DriverBindingProtocol::GUID, (&raw const (*b).protocol).cast())
    };
    if r.is_err() {
        // SAFETY: not installed, so nothing else holds it.
        drop(unsafe { Box::from_raw(b) });
    }
    r.map(|_| ())
}
