# Changelog

## [Unreleased]

### 2026-11-20
- **fix:** a failed PHY link read during SNP Initialize/Reset left the NIC running (#11, review findings a02063c26ea9, e1f05b9e82f7): every failure after `net.start` (advertisement, link read, and a failed start itself) now quiesces the NIC again and leaves the interface Started, Reset included. A quiesce that fails (on that path, Shutdown, Stop, re-Initialize or halt) marks the core `wedged`: `src/snp.rs` turns bus mastering off once (`binding::keep_dma`) and Initialize/Reset return DEVICE_ERROR (`Fail::Wedged`). Host tests in `test/snp.rs` with a PHYSTAT fault in the simulator.
- **fix:** GetStatus could hand back transmit tokens from an earlier run (#11, review finding aea66f5ba24a): Initialize, Reset, Shutdown, Stop and halt drop the pending tokens and the TRANSMIT interrupt bit. Host test in `test/snp.rs`. sc-build 2026-11-21 at cd8d63b: test-host.sh 53 pass, check-driver.sh both targets clean.
- **fix:** a DMA check that timed out still installed the SNP (#10, review finding 4d2cf4e1502a): `ring::check` now returns `rtl::Error::DmaCheck` when the check frame's descriptor is not back within 100 ms (or the ring will not take it), so Start quiesces the NIC, releases the region and fails with DEVICE_ERROR, keeping nothing so a later ConnectController can retry. Host tests in `test/rtl.rs`.
- **fix:** the DMA region was freed even when its Unmap failed (#10, review finding cbfb1142d3f0): `dma::release` (new `src/dma.rs`) calls FreeBuffer only after Unmap succeeded; on an Unmap failure the pages stay allocated and mapped until reboot and the console says so. Host tests in `test/dma.rs`, run by `scripts/test-host.sh`.
- **fix:** a NIC that would not quiesce got bus mastering back (#6, review finding 6025a824371b; a902f1c, 5b8158e, e3bd0de): `keep_dma` turned BME off, then `restore` put the firmware's attributes and command register back, BME included, with the DMA region kept and the NIC possibly still holding descriptors into it. The three kept-DMA paths (Start's quiesce failure, SNP install failure with the region in use, binding Stop when the port cannot be destroyed) now use `decode::release_master_off`: memory decode back as found, bus mastering off and read back clear. Host tests for `decode` (`test/decode.rs`, run by `scripts/test-host.sh`).
- **chore:** #9 re-filed the same finding (6025a824371b, `src/binding.rs:345` at 9fa5ce6): already fixed by #6; that line now reads `restore` only on the quiesced (`Region::Released`) path, so its text is still in the file and the finding did not close by itself. No code change: the source at main is e3bd0de's (test-host.sh 44 pass there; check-driver.sh at 5b8158e, which differs only in test/decode.rs).

### 2026-11-14
- **docs:** refreshed README/docs/CLAUDE.md against the code since 2026-11-07 (#5 per-port line, `rtl::standard_fec`/`media`/`PortLine`, error line wording, v0.2.0, 39 host tests): all claims match; no unmet promises, so no issues filed. Remaining open items (#2, #1, #5 on-hardware proof) need the O6N.

## [v0.2.0] — 2026-11-13

### Added
- the stormnic contract (#5): the driver owns its port's bring-up
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
