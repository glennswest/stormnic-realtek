# stormnic-realtek

A Rust `no_std` UEFI driver that gives firmware an
`EFI_SIMPLE_NETWORK_PROTOCOL` (SNP) for the **Realtek RTL8125 / RTL8126**
family (2.5 and 5 GbE), built for **x86_64 and aarch64**. stormbootx loads
it from `\stormboot\drivers` and runs its own TCP/IP (smoltcp) on it, as with
its siblings stormnic-ixgbe, stormnic-mlx4 and stormnic-virtio. Its first
target is the 2.5GbE on the Radxa Orion O6N (CIX P1, arm64; stormcos#665).

Hardware facts from OpenBSD's rge(4) (ISC, see NOTICE and
docs/hardware.md), the binding and SNP from the UEFI specification. MIT.

Status: v0.1.0, verified against a simulated chip only. No emulator has an
RTL8125, so the first real run is the O6N (#2).

## What it binds

| PCI ID | pci.ids | MAC revisions (TXCFG HWREV) |
|---|---|---|
| `10ec:8125` | RTL8125 2.5GbE Controller | RTL8125A (0x609), RTL8125B (0x641), RTL8125D (0x688, 0x689) |
| `10ec:3000` | Killer E3000 2.5GbE Controller | as 8125 |
| `10ec:8126` | RTL8126 5GbE Controller | RTL8126A (0x649, 0x64a) |
| `10ec:5000` | Killer E5000 5GbE Controller | as 8126 |

An unknown revision (RTL8127, 0x6c9, among them) is refused before anything
is written. The RTL8111/8168 gigabit family (`10ec:8168`) is not bound: it waits for a
test machine (#4).

The driver declines a function another driver already holds `BY_DRIVER`, so
the platform's own driver wins. The O6N's firmware has one; stormbootx takes
the NIC from it with `prefer_media_drivers = realtek` (stormbootx#172).

## How it works

- **Start**: PciIo `BY_DRIVER | EXCLUSIVE`; memory decode and bus mastering
  (`src/decode.rs`); the register BAR (2, else 1); the chip revision and
  the factory MAC (0x19e0 backup, else MAC0). One DMA region (PciIo
  `AllocateBuffer` + `Map` common buffer, 33 pages) holds both rings and
  their buffers. The NIC is brought up once: out of OOB (management
  firmware) mode, MAC reset, the revision's MAC init, the PHY's
  advertisement corrected (below). If the link is up, a DMA check sends one
  broadcast frame (EtherType 0x88B5, `stormnic-realtek DMA check`) and waits
  for its descriptor to come back. Then the NIC is **quiesced** and the SNP
  goes on a child handle with the controller's device path plus a MAC node.
- **Flow control off** (#2): the PHY advertises every speed the chip has
  (10/100/1000, 2.5G, and 5G on the RTL8126) and **no 802.3x pause**, so
  the link resolves to no flow control. A NIC whose ring nobody empties
  then drops frames instead of sending pause frames to the switch (what
  flooded the LAN from the O6N's firmware). Autonegotiation is restarted
  only if the advertisement was different. The EFI variable
  `StormnicRealtekPause` (GUID below, first byte not 0) advertises pause.
- **Quiesced whenever nobody drives it** (#2): interrupts masked and
  acknowledged, nothing accepted, a stop request until both FIFOs are
  empty, then a MAC reset (RX and TX off). This runs at every Start failure
  after the chip is identified, SNP Shutdown and Stop, binding Stop, and the
  ExitBootServices event — in every SNP state, so a chip an earlier stage
  left running stops too. RX and TX are on only between SNP Initialize and
  Shutdown. A reset that does not complete turns bus mastering off and
  keeps the DMA pages allocated.
- **Rings**: 32 receive descriptors (32-byte format) and 32 transmit
  descriptors (16 bytes), one 2 KiB buffer each; polled, no interrupts.
  Frames are copied in and out of the driver's own buffers, so no caller
  memory is mapped for DMA. Short frames are padded to 60 bytes.
- **aarch64**: descriptor ownership and the doorbell are ordered with
  `dmb oshst` / `dmb oshld` / `dsb st` (`src/barrier.rs`); DMA memory is
  the firmware's common buffer, uncached on a non-coherent platform.
- **SNP** (`src/snp_core.rs`, from stormnic-virtio): the chip accepts its
  address, broadcast and all multicast (and everything when promiscuous);
  the SNP's receive filters are applied in software. The station address
  is the chip's own (`MacAddressChangeable` FALSE). Statistics and NvData:
  `EFI_UNSUPPORTED`.

## Console

One line per NIC by default:

```
stormnic-realtek 0.1.0: 0000:01:00.0 10ec:8125 RTL8125B: MAC 00:e0:4c:68:01:25, link up 2500 Mb/s full duplex, flow control off, SNP installed
```

Warnings and failures always print, with the trace steps that led to them.
The full trace (BAR, TXCFG, advertisement before and after, DMA region,
DMA check) prints with `--features verbose`, or when the EFI variable
`StormnicVerbose` exists with a non-zero first byte:

```
setvar StormnicVerbose -guid ce1479a2-eab9-4176-b0ad-c909ea5b8e0b -bs =01
setvar StormnicRealtekPause -guid ce1479a2-eab9-4176-b0ad-c909ea5b8e0b -bs =01   # only to turn pause on
```

A link that resolves to TX pause although none is advertised prints a
warning.

## Build and test

```bash
sc-build scripts/check-driver.sh   # both targets, clippy -D warnings, PE machine + subsystem 11
sc-build scripts/test-host.sh      # host tests against the simulated chip (test/sim.rs)
```

`check-driver.sh` leaves `dist/stormnic-realtek-x86_64.efi`,
`dist/stormnic-realtek-aarch64.efi` and `dist/SHA256SUMS`.

## Shipping

stormbootx builds the driver at a pinned commit (`scripts/build-nic-drivers.sh`)
into its media; the arm64 media gets it with `prefer_media_drivers =
realtek` (stormbootx#172). This repository has no image of its own.

## Licence

MIT (see LICENSE). NOTICE acknowledges rge(4), the source of the hardware
facts; no code was copied.
