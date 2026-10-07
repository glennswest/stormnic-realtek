# stormnic-realtek

A Rust `no_std` UEFI Simple Network Protocol (SNP) driver for **Realtek RTL8111/8168, RTL8125, RTL8126** (10ec:8168, 8125, 8126), loaded by
[stormbootx](https://github.com/glennswest/stormbootx) so it can claim and boot images over these NICs without iPXE.
A sibling of stormnic-ixgbe, stormnic-mlx4 and stormnic-virtio, built and pinned the same way (stormbootx `build-nic-drivers.sh`).

Status: not started (P3, paused; owner 2026-10-07).

## Licence

MIT (see LICENSE). NOTICE acknowledges the sources the hardware facts were learned from; no code was copied.
