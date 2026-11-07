# CLAUDE.md — stormnic-realtek

A `no_std` UEFI driver giving firmware an `EFI_SIMPLE_NETWORK_PROTOCOL` for
the **Realtek RTL8125 / RTL8126** family (2.5 and 5 GbE; 10ec:8125, 10ec:8126,
and the Killer-branded 10ec:3000 / 10ec:5000), for **x86_64 and aarch64**.
Loaded by stormbootx from `\stormboot\drivers` like its siblings
stormnic-ixgbe, stormnic-mlx4 and stormnic-virtio. Its first target is the
Radxa Orion O6N's 2.5GbE (stormcos docs/HW-RADXA-ORION-O6N.md, stormcos#665).
Read README.md first.

Read the cross-project rules in `../CLAUDE.md` first. In particular, **build
with `sc-build` after pushing, never on this VM and never as root**, and
scratch files go in `tmp/`.

## Rules for this crate

- **Sources:** register layouts, descriptor formats and bring-up sequences
  from OpenBSD's rge(4) (`sys/dev/pci/if_rge.c`, `if_rgereg.h`, ISC licence,
  Kevin Lo; notice in NOTICE), and the UEFI specification for the binding and
  SNP. **Never translated from Linux r8169 or Realtek's r8125 (GPL)**: the
  crate is MIT. Reading them for behaviour is fine. Like stormnic-ixgbe (its
  #2: the owner accepted a permissive vendor/BSD source), the Rust is written
  here; docs/hardware.md describes the hardware in its own words.
- No MAC/PHY MCU patches and no firmware files: rge(4) loads Realtek's
  microcode blobs; this driver relies on the patch state the platform
  firmware (or power-on default) left, and touches only the MAC and the PHY's
  standard MII registers plus the Realtek speed-advertisement register.
- **Flow control is off** (owner, #2, 2026-10-28: the O6N flooded the LAN
  with pause frames): the PHY never advertises PAUSE/ASYM_PAUSE unless the EFI
  variable `StormnicRealtekPause` (StormnicVerbose's vendor GUID) is set to a
  non-zero byte.
- **The NIC is quiet whenever nobody drives it**: RX/TX disabled, MAC reset,
  interrupts masked, at every Start failure, SNP Shutdown/Stop, binding Stop
  and ExitBootServices. Start leaves it reset; only SNP Initialize enables RX
  and TX. A reset that does not complete turns bus mastering off.
- **Arm:** DMA memory comes from PciIo AllocateBuffer + Map(CommonBuffer),
  so a non-coherent platform gets uncached memory; descriptor writes are
  ordered with `dmb oshst` before the OWN bit, `dmb oshld` after reading it,
  and `dsb st` before the TX doorbell (src/barrier.rs).
- Same layout as stormnic-virtio: the hardware core (`src/rtl.rs`,
  `src/ring.rs`, `src/snp_core.rs`) is UEFI-independent and tested on the
  host against a simulated chip (`test/`); `src/binding.rs`, `src/snp.rs`,
  `src/pci_io.rs` are the UEFI glue.
- One line per NIC on the console by default, the trace behind
  `StormnicVerbose` / `--features verbose` (stormnic-ixgbe#22).

## Build

```bash
sc-build scripts/check-driver.sh     # x86_64 + aarch64 builds, PE32+ subsystem 11 checks
sc-build scripts/test-host.sh        # simulated-chip tests
```

## Test

- Host: `test/*.rs`, compiled with plain `rustc --test` (scripts/test-host.sh).
- No emulator has an RTL8125 (QEMU has only the RTL8139), so the only
  end-to-end check is real hardware: the O6N (aarch64) via stormbootx-arm64's
  media with `prefer_media_drivers = realtek` (stormbootx#172).

## Version

`Cargo.toml` → `package.version`. Current: `v0.1.0`.

## Work plan

- [ ] #2 aarch64 + O6N (P0; covers #1's driver for the RTL8125/8126 family)
  - [ ] Scaffold: Cargo.toml, build.rs (subsystem 11), Cargo.lock, console,
        trace, decode, pci_io from stormnic-virtio
  - [ ] Hardware core from rge(4): chip ID (TXCFG), reset + exit OOB, MAC,
        MAC/PHY OCP + CSI access, MAC init, rings (RX 32-byte v1, TX 16-byte),
        PHY advertisement without pause, link/speed/pause from PHYSTAT, quiesce
  - [ ] SNP core + UEFI glue (binding, SNP, ExitBootServices quiesce)
  - [ ] Host tests against a simulated chip
  - [ ] sc-build: x86_64-unknown-uefi and aarch64-unknown-uefi, no warnings
  - [ ] stormbootx: pin the commit in build-nic-drivers.sh and put it on the
        arm64 media (filed on stormbootx, stormbootx#172's part)
  - [ ] On the O6N: lspci confirms 10ec:8125 (owner); boot shows the
        driver's line, DHCP lease and an NVMe/TCP claim
- [ ] #1 RTL8111/8168 (10ec:8168): not in this pass (different MAC init;
      FreeBSD re(4) would be the source). Stays P3, paused.
