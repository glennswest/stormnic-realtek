//! stormnic-realtek: an `EFI_SIMPLE_NETWORK_PROTOCOL` driver for the
//! Realtek RTL8125 / RTL8126 family, in Rust, for x86_64 and aarch64.
//!
//! An EFI boot-service driver (see build.rs). The entry point installs an
//! `EFI_DRIVER_BINDING_PROTOCOL` on the image handle (binding.rs) and returns;
//! the firmware's `ConnectController` then calls Supported/Start for each
//! controller. Start identifies the chip, takes it from the management
//! firmware, makes the PHY advertise without pause (#2), runs a DMA check
//! when the link is up, quiesces the NIC and puts an
//! `EFI_SIMPLE_NETWORK_PROTOCOL` on a child handle; the user (stormbootx's
//! smoltcp) opens it EXCLUSIVE, and only its Initialize turns RX/TX on.
#![no_main]
#![no_std]

extern crate alloc;

// First: its macros (say!, trace!, fail!, note!) are used by the modules below.
#[macro_use]
mod console;
mod binding;
mod decode;
mod ids;
mod pci_io;
mod rtl;
mod snp;
mod trace;

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::LOAD_ERROR;
    }
    console::init();
    log::set_max_level(if console::verbose() { log::LevelFilter::Info } else { log::LevelFilter::Warn });
    let version = env!("CARGO_PKG_VERSION");
    match binding::install() {
        Ok(()) => {
            trace!(
                "stormnic-realtek {version}: driver binding installed ({} Realtek device IDs), flow control {}",
                ids::SUPPORTED.len(), if console::pause() { "advertised (StormnicRealtekPause)" } else { "off" }
            );
            Status::SUCCESS
        }
        Err(e) => {
            say!("stormnic-realtek {version}: driver binding not installed: {:?}", e.status());
            e.status()
        }
    }
}
