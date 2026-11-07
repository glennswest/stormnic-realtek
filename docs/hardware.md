# RTL8125 / RTL8126: what the driver relies on

The facts below were learned from OpenBSD's rge(4) (ISC; see NOTICE) and
are described here in this project's own words. Offsets are into the
memory BAR (BAR 2, 64-bit; BAR 1 if that is absent). `src/rtl.rs` and
`src/ring.rs` implement them.

## Identifying the chip

TXCFG (0x40) bits 20-30 (mask `0x7cf00000`) name the MAC revision:

| HWREV | chip | notes |
|---|---|---|
| 0x609 | RTL8125A | 2.5G |
| 0x641 | RTL8125B | 2.5G; the common one |
| 0x688, 0x689 | RTL8125D | 2.5G |
| 0x649, 0x64a | RTL8126A | 5G |
| 0x6c9 | RTL8127 | 10G; **not bound** (v2 descriptors, MSI-X layout) |

Anything else is refused before a single write. TXCFG reading all ones
means the function is gone or its BAR is not decoded.

The factory address is at 0x19e0 (4 bytes) and 0x19e4 (2 bytes), a copy
that survives a previous driver changing the station address in MAC0-5
(0x00-0x05). The driver uses the backup when it is a unicast address and
writes it back to MAC0/MAC4 (with the config registers unlocked through
EECMD 0x50 = 0xc0).

## Register windows

- **MAC OCP** (0xb0, 32-bit): a 16-bit register file. Address halved into
  bits 16-30; a write sets bit 31 with the value in bits 0-15; a read
  writes the address alone and reads the value back at once.
- **PHY OCP** (0xb8): the same layout for the built-in PHY, but
  asynchronous: a write is complete when bit 31 reads clear, a read when
  bit 31 reads set. The driver gives either 20 ms.
- The PHY's standard MII registers sit in the PHY OCP space at
  0xa400 + 2 x register (BMCR 0xa400, ANAR 0xa408, 1000BASE-T control
  0xa412). The 2.5G / 5G advertisement is at 0xa5d4 (bit 7: 2.5G, bit 8:
  5G). The low 3 bits of 0xa420 are the PHY's state (3: ready).
- **CSI** (0x68 address/flag, 0x64 data): the PCIe configuration space as
  the MAC sees it, 32 bits at a time; bit 31 is the done flag (set by the
  driver on a write, set by the chip when a read is ready).

## Stopping and resetting

The stop sequence the driver uses everywhere (Start failures, SNP
Shutdown/Stop, binding Stop, ExitBootServices):

1. IMR (0x38) = 0, ISR (0x3c) acknowledged, the four timer interrupts
   (0x58, 0x5c, 0x8c, 0xf4) = 0;
2. RXCFG (0x44) accept bits 0-5 cleared, so no new frame is taken;
3. PPSW (0xf2) bit 3 set (the RXDV gate);
4. CMD (0x37) bit 7 (stop request), then wait for MCUCMD (0xd3) bits 4-5
   (RX and TX FIFOs empty) and, except on the RTL8125A, for 0xe2 bits
   0x0103;
5. CMD = 0x10 (soft reset); the bit clears itself within 10 ms. The reset
   disables RX (bit 3) and TX (bit 2).

If the reset never completes, Start or Stop turns bus mastering off at the
PCI level and leaves the DMA pages allocated, so the chip can never write
memory someone else owns.

Taking the MAC from the management firmware ("exit OOB"): MAC OCP 0xc0bc =
0x00ff (RealWoW off), reset, MCUCMD bit 7 cleared, MAC OCP 0xe8de bit 14
cleared, then 0xc0aa = 0x07d0, 0xc0a6 = 0x01b5, 0xc01e = 0x5555, with
waits on TWICMD (0xd2) bit 9; and if MAC OCP 0xd42c bit 8 says the chip
came back from ultra power saving, wait for PHY state 2 and clear the
UPS flags (0xd42c bit 8, PHY 0xa466 bit 0 except RTL8125A, PHY 0xa468
bits 1 and 3).

## Bring-up

`rtl::chip_init` then `rtl::mac_init`, in rge(4)'s order. What is skipped:
the MAC and PHY microcode patches (Realtek firmware files), RSS, multiple
queues, interrupt moderation, checksum offload and VLAN stripping. Per
revision the important values are RXCFG (receive DMA burst and FIFO
threshold, bit 24 selecting the 32-byte receive descriptor):

| chip | RXCFG |
|---|---|
| RTL8125A | 0x41000700 |
| RTL8125B | 0x41000c00 |
| RTL8125D | 0x41200c00 |
| RTL8126A | 0x41200d00 |

and TXCFG = 0x03000700. The receive filter accepts the station address,
broadcast and every multicast (MAR 0x08/0x0c all ones); promiscuous adds
bit 0. RXMAXSIZE (0xda) = 2048, one buffer. RX and TX are turned on (CMD =
0x0c) only after the rings are filled.

## Flow control (#2)

The MAC sends and honours 802.3x pause frames according to the
autonegotiated result, which PHYSTAT (0x6c) reports in bits 5 (RX pause)
and 6 (TX pause). A Realtek MAC whose receive ring nobody empties sends
pause frames for as long as it stays full; the Orion O6N did that to the
whole LAN while sitting in firmware (owner, 2026-10-28). So the driver
writes ANAR without PAUSE (bit 10) and ASYM_PAUSE (bit 11) and restarts
autonegotiation if the advertisement was different; the link then
resolves to no flow control whatever the partner offers (IEEE 802.3 Annex
28B). The EFI variable `StormnicRealtekPause` (non-zero first byte)
advertises both instead.

## Descriptors

Transmit, 16 bytes: `cmdsts` (bit 31 OWN, 30 EOR, 29 first segment, 28
last segment, 0-15 length), `extsts` (offloads; 0), the 64-bit buffer
address, little-endian. The doorbell is a 16-bit write of 1 to 0x90.

Receive, 32 bytes with RXCFG bit 24: the buffer address at byte 16,
`extsts` at 24, `cmdsts` at 28 (bit 31 OWN, 30 EOR, 25 first segment, 24
last segment, 20 error summary, 0-13 length including the 4-byte CRC). The
driver posts a descriptor with OWN | buffer size (| EOR on the last).

EOR marks the last descriptor of a ring; the chip wraps after it. The ring
bases go in 0xe4/0xe8 (receive) and 0x20/0x24 (transmit), 256-byte aligned.

## Memory ordering on aarch64

See `src/barrier.rs`: `dmb oshst` between filling a descriptor and setting
OWN, `dmb oshld` between seeing OWN clear and reading the frame, `dsb st`
before the doorbell. DMA memory is PciIo AllocateBuffer +
Map(BusMasterCommonBuffer): on a platform whose PCIe is not cache coherent
the firmware hands out uncached memory for it.
