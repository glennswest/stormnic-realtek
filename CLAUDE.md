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
  - [x] Scaffold, hardware core, SNP core + UEFI glue, host tests (54d55b4, c161a1f)
  - [x] sc-build 2026-11-07: `scripts/test-host.sh` at c161a1f, 36 tests pass;
        `scripts/check-driver.sh` at c9f3a1a: x86_64 (60,416 B) and aarch64
        (56,320 B) PE32+ subsystem 11, clippy -D warnings clean (#3 was the
        one clippy finding, closed)
  - [ ] stormbootx: pin c9f3a1a in build-nic-drivers.sh, built for
        aarch64-unknown-uefi, on the arm64 media with
        `prefer_media_drivers = realtek` (stormbootx#172, asked there);
        #2 is proposed after it
  - [ ] On the O6N (owner's hands; not a registered test machine yet):
        `lspci -nn` confirms 10ec:8125 or 8126; the stormbootx-arm64 boot
        shows `stormnic-realtek 0.1.0: … RTL8125…: MAC …, link up … flow
        control off, SNP installed`, a DHCP lease and an NVMe/TCP claim; no
        pause frames on the switch port while it sits in stormbootx
- [ ] #1 the whole family, pinned in stormbootx's nic-drivers (P3)
  - [x] RTL8125/8126: written under #2 (fdd6d60)
  - [x] RTL8111/8168 split off to #4 (P3, paused until a machine with that
        port exists; re(4) licence question waits there) — master's decision
        on #1, 2026-11-07, option 1
  - [ ] #1 closes when the RTL8125/8126 driver binds on a real chip (the O6N,
        #2) and is pinned in stormbootx's nic-drivers (stormbootx#172);
        proposed after #2
