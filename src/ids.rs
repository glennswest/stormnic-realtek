//! The PCI functions this driver binds, checked against pci.ids
//! (pci-ids.ucw.cz, 2026-11-07) and OpenBSD rge(4)'s match table.
//!
//! RTL8127 (10ec:8127, 10 GbE) is in rge(4)'s table but not here: it uses
//! the v2 descriptor format and interrupt vectors this driver does not have.
//! The RTL8111/8168 gigabit family (10ec:8168) is #1.

/// PCI class: network controller.
pub const CLASS_NETWORK: u8 = 0x02;

pub struct Nic {
    pub device: u16,
    pub name: &'static str,
}

pub const SUPPORTED: &[Nic] = &[
    Nic { device: 0x8125, name: "RTL8125 2.5GbE" },
    Nic { device: 0x8126, name: "RTL8126 5GbE" },
    Nic { device: 0x3000, name: "Killer E3000 2.5GbE" },
    Nic { device: 0x5000, name: "Killer E5000 5GbE" },
];

pub fn lookup(vendor: u16, device: u16) -> Option<&'static Nic> {
    if vendor != crate::rtl::VENDOR {
        return None;
    }
    SUPPORTED.iter().find(|n| n.device == device)
}
