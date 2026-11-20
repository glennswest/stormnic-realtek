//! Memory decode and bus mastering (`src/decode.rs`) against a simulated PCI
//! function: what `enable` sets, what `release` puts back, and that a NIC
//! whose DMA region is kept never gets bus mastering back (#6, review
//! finding 6025a824371b).
#[path = "../src/decode.rs"]
#[allow(dead_code)]
mod decode;
use decode::*;
use std::cell::Cell;

/// A function whose PciIo attributes and command register track each other
/// as EDK2's do: setting the attribute sets the command bit.
struct Fn {
    attributes: Cell<u64>,
    command: Cell<u16>,
    /// The attribute calls answer UNSUPPORTED (AMI Aptio 4).
    attributes_broken: bool,
}

const BOTH_A: u64 = ATTRIBUTE_MEMORY | ATTRIBUTE_BUS_MASTER;
const BOTH_C: u16 = COMMAND_MEMORY | COMMAND_BUS_MASTER;

impl Fn {
    fn new(attributes: u64, attributes_broken: bool) -> Self {
        let f = Fn { attributes: Cell::new(0), command: Cell::new(0), attributes_broken };
        f.apply(attributes); // the firmware's own state, set before Start
        f
    }
    fn apply(&self, a: u64) {
        self.attributes.set(a);
        let mut c = self.command.get() & !BOTH_C;
        if a & ATTRIBUTE_MEMORY != 0 { c |= COMMAND_MEMORY; }
        if a & ATTRIBUTE_BUS_MASTER != 0 { c |= COMMAND_BUS_MASTER; }
        self.command.set(c);
    }
    fn bme(&self) -> bool { self.command.get() & COMMAND_BUS_MASTER != 0 }
    fn mse(&self) -> bool { self.command.get() & COMMAND_MEMORY != 0 }
}

impl Pci for Fn {
    type Status = &'static str;
    fn get(&self) -> Result<u64, Self::Status> {
        if self.attributes_broken { Err("unsupported") } else { Ok(self.attributes.get()) }
    }
    fn supported(&self) -> Result<u64, Self::Status> {
        if self.attributes_broken { Err("unsupported") } else { Ok(BOTH_A) }
    }
    fn enable(&self, a: u64) -> Result<(), Self::Status> {
        if self.attributes_broken { return Err("unsupported"); }
        self.apply(self.attributes.get() | a);
        Ok(())
    }
    fn disable(&self, a: u64) -> Result<(), Self::Status> {
        if self.attributes_broken { return Err("unsupported"); }
        self.apply(self.attributes.get() & !a);
        Ok(())
    }
    fn set(&self, a: u64) -> Result<(), Self::Status> {
        if self.attributes_broken { return Err("unsupported"); }
        self.apply(a);
        Ok(())
    }
    fn config_read_u16(&self, offset: u32) -> Result<u16, Self::Status> {
        assert_eq!(offset, COMMAND);
        Ok(self.command.get())
    }
    fn config_write_u16(&self, offset: u32, value: u16) -> Result<(), Self::Status> {
        assert_eq!(offset, COMMAND);
        self.command.set(value);
        Ok(())
    }
}

/// Every starting state Start can meet: the attributes the firmware left,
/// and whether its attribute calls work.
fn every_start() -> Vec<Fn> {
    let mut v = Vec::new();
    for broken in [false, true] {
        for a in [0, ATTRIBUTE_MEMORY, ATTRIBUTE_BUS_MASTER, BOTH_A] {
            v.push(Fn::new(a, broken));
        }
    }
    v
}

#[test]
fn enable_sets_memory_decode_and_bus_mastering_from_every_start() {
    for f in every_start() {
        enable(&f).unwrap();
        assert!(f.mse() && f.bme(), "command {:#06x}", f.command.get());
    }
}

#[test]
fn release_puts_back_what_the_firmware_had() {
    for f in every_start() {
        let (mse, bme) = (f.mse(), f.bme());
        let e = enable(&f).unwrap();
        release(&f, &e).unwrap();
        assert_eq!((f.mse(), f.bme()), (mse, bme), "attributes broken: {}", f.attributes_broken);
    }
}

#[test]
fn a_kept_dma_region_never_gets_bus_mastering_back() {
    // The quiesce-failed path: keep_dma turns BME off, then the PCI state is
    // put back. Firmware that had bus mastering on must not get it back.
    for f in every_start() {
        let mse = f.mse();
        let e = enable(&f).unwrap();
        stop_bus_master(&f).unwrap();
        release_master_off(&f, &e).unwrap();
        assert!(!f.bme(), "BME back on; attributes broken: {}, command {:#06x}", f.attributes_broken, f.command.get());
        // The PciIo attributes say so too where the firmware's attribute
        // calls work (where they answer UNSUPPORTED, the command register
        // above is the only state there is).
        if !f.attributes_broken {
            assert_eq!(f.attributes.get() & ATTRIBUTE_BUS_MASTER, 0);
        }
        assert_eq!(f.mse(), mse, "memory decode put back as found");
    }
}

#[test]
fn a_kept_dma_region_gets_bus_mastering_off_even_without_keep_dma_first() {
    // release_master_off checks BME itself, so it holds whatever ran before.
    for f in every_start() {
        let e = enable(&f).unwrap();
        release_master_off(&f, &e).unwrap();
        assert!(!f.bme());
    }
}

#[test]
fn plain_release_after_stop_bus_master_would_turn_it_back_on() {
    // The defect #6 fixes, kept as a guard that the test sees it: with the
    // firmware's bus mastering on, release restores it.
    let f = Fn::new(BOTH_A, false);
    let e = enable(&f).unwrap();
    stop_bus_master(&f).unwrap();
    release(&f, &e).unwrap();
    assert!(f.bme());
}
