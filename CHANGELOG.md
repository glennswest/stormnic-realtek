# Changelog

## [Unreleased]

### 2026-11-13
- **feat:** the stormnic contract (#5): the driver owns its port's bring-up
  (advertisement, autonegotiation, link; the IEEE 802.3 FEC for the media
  and speed, `rtl::standard_fec`: none to 1000BASE-T, the PHY's own LDPC at
  2.5GBASE-T/5GBASE-T, nothing to configure) and prints one line per port:
  driver, PCI address, PCI ID, chip, MAC, media, speed, FEC, link. A port
  without link is an `error:` line. The old line's flow-control and "SNP
  installed" details moved to the trace.

### 2026-11-07
- **docs:** #1 scope: RTL8111/8168 (10ec:8168, 8161, 2600) split off to #4, P3 and paused until a test machine exists (master's decision on #1, option 1); #1 closes on the RTL8125/8126 driver verified on the O6N and pinned in stormbootx's nic-drivers.
- **feat:** the driver, for the RTL8125/RTL8126 family (10ec:8125, 8126, 3000, 5000; RTL8125A/B/D, RTL8126A by TXCFG HWREV), built for `x86_64-unknown-uefi` and `aarch64-unknown-uefi` (#2). `EFI_DRIVER_BINDING_PROTOCOL` + `EFI_SIMPLE_NETWORK_PROTOCOL` on a child handle, polled, 32 x 2 KiB rings each way, one console line per NIC (trace behind `StormnicVerbose`).
- **feat:** 802.3x flow control off: the PHY advertises no pause unless the EFI variable `StormnicRealtekPause` is set, so a NIC whose ring nobody drains drops frames instead of flooding the LAN (owner on #2, 2026-10-28).
- **feat:** the NIC is quiesced (interrupts masked, stop request, MAC reset, nothing accepted) at every Start failure, SNP Shutdown/Stop, binding Stop and ExitBootServices whatever the SNP state; RX/TX run only between SNP Initialize and Shutdown. A reset that does not complete turns bus mastering off.
- **feat:** aarch64 DMA ordering (`dmb oshst`/`dmb oshld`/`dsb st`) around descriptor ownership and the doorbell.
- **docs:** refreshed README/docs/CLAUDE.md against the code (2026-11-07): all claims (IDs, revisions, ring sizes, variables, scripts, 36 host tests) match; fixed a README typo. No unmet promises found, so no issues filed.
- **docs:** NOTICE now credits OpenBSD rge(4) (ISC) as the source of the hardware facts; docs/hardware.md; README; CLAUDE.md work plan.
- **fix:** clippy `enum_variant_names` on PciIo `MapOp` (#3).
- **test:** host tests against a simulated chip (`scripts/test-host.sh`); `scripts/check-driver.sh` builds both targets, runs clippy with `-D warnings` and checks PE machine and subsystem 11. Verified on a build VM: 36 host tests (c161a1f), both targets clean (c9f3a1a).

### 2026-10-07
- **chore:** repository created (owner: low priority, paused)
