//! The MAC/PHY sequences and the rings against the simulated chip in `sim.rs`.
#[path = "../src/rtl.rs"]
#[allow(dead_code)]
mod rtl;
#[allow(dead_code)]
mod sim;
use rtl::ring::{self, Net};
use rtl::{Chip, Error, Link};
use sim::*;

const ALL: [Chip; 6] = [Chip::R25, Chip::R25B, Chip::R25D(1), Chip::R25D(2), Chip::R26(1), Chip::R26(2)];

#[test]
fn every_known_revision_is_identified_with_its_address() {
    for chip in ALL {
        let mut d = Dev::new(chip);
        let id = rtl::identify(&mut d).unwrap();
        assert_eq!((id.chip, id.mac), (chip, MAC));
        assert!(id.link.up);
        assert!(d.ops.is_empty(), "identify writes nothing");
    }
}

#[test]
fn rtl8127_and_unknown_revisions_are_refused_and_a_dead_function_is_removed() {
    let mut d = Dev::new(Chip::R25B);
    d.regs[0x40..0x44].copy_from_slice(&0x6c90_0700u32.to_le_bytes());
    assert_eq!(rtl::identify(&mut d), Err(Error::UnknownChip { txcfg: 0x6c90_0700 }));
    d.regs[0x40..0x44].copy_from_slice(&[0xff; 4]);
    assert_eq!(rtl::identify(&mut d), Err(Error::Removed));
}

#[test]
fn the_backup_address_wins_then_mac0_then_none() {
    let mut d = Dev::new(Chip::R25B);
    d.regs[0..6].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]); // a changed MAC0
    assert_eq!(rtl::read_mac(&mut d), Ok(MAC));
    d.regs[0x19e0..0x19e6].fill(0);
    assert_eq!(rtl::read_mac(&mut d), Ok([0x02, 1, 2, 3, 4, 5]));
    d.regs[0..6].fill(0xff);
    assert_eq!(rtl::read_mac(&mut d), Err(Error::NoMac));
}

#[test]
fn phystat_decodes_speed_duplex_and_pause() {
    let l = Link::from_phystat(rtl::PHYSTAT_LINK | rtl::PHYSTAT_FDX | rtl::PHYSTAT_2500);
    assert_eq!(l, Link { up: true, mbps: 2500, full_duplex: true, rx_pause: false, tx_pause: false });
    assert_eq!(format!("{l}"), "up 2500 Mb/s full duplex, flow control off");
    let l = Link::from_phystat(rtl::PHYSTAT_LINK | rtl::PHYSTAT_1000 | rtl::PHYSTAT_TXFLOW);
    assert_eq!(format!("{l}"), "up 1000 Mb/s half duplex, flow control rx off tx on");
    assert_eq!(format!("{}", Link::from_phystat(0)), "down");
}

// #2: flow control.

#[test]
fn the_firmware_s_pause_advertisement_is_withdrawn_and_the_link_has_no_flow_control() {
    for chip in ALL {
        let mut d = Dev::new(chip);
        assert!(rtl::link(&mut d).unwrap().tx_pause, "{chip:?}: the firmware's link had pause");
        assert_eq!(rtl::advertise(&mut d, chip, false), Ok(true));
        let anar = d.phy[&mii(rtl::MII_ANAR)];
        assert_eq!(anar & (rtl::ANAR_PAUSE | rtl::ANAR_ASYM_PAUSE), 0, "{chip:?}");
        assert_eq!(anar & 0x1f, 1, "selector kept");
        assert_eq!(anar & 0x1e0, 0x1e0, "10/100 half and full");
        assert_eq!(d.phy[&mii(rtl::MII_GTCR)] & 0x300, 0x300, "1000 half and full");
        let fast = d.phy[&rtl::PHY_ADV];
        assert_eq!(fast & rtl::ADV_2500, rtl::ADV_2500);
        assert_eq!(fast & rtl::ADV_5000 != 0, matches!(chip, Chip::R26(_)), "{chip:?}: 5G only on the RTL8126");
        assert_eq!(d.an_restarts, 1);
        assert!(!rtl::link(&mut d).unwrap().up, "renegotiating");
        let l = rtl::wait_link(&mut d, 5000).unwrap();
        assert!(l.up && !l.rx_pause && !l.tx_pause, "{chip:?}: {l}");
        assert_eq!(l.mbps, chip.max_mbps());
    }
}

#[test]
fn an_advertisement_that_is_already_right_leaves_the_link_alone() {
    let mut d = Dev::new(Chip::R25B);
    rtl::advertise(&mut d, Chip::R25B, false).unwrap();
    d.now += AN_US;
    let writes = d.writes(rtl::PHYOCP).len();
    assert_eq!(rtl::advertise(&mut d, Chip::R25B, false), Ok(false));
    assert_eq!(d.an_restarts, 1);
    assert!(rtl::link(&mut d).unwrap().up);
    assert_eq!(d.writes(rtl::PHYOCP).len(), writes + 4, "four reads (ANAR, GTCR, 2.5G, BMCR), no write");
}

#[test]
fn the_pause_setting_advertises_symmetric_and_asymmetric_pause() {
    let mut d = Dev::new(Chip::R25B);
    d.phy.insert(mii(rtl::MII_ANAR), 0x01e1);
    assert_eq!(rtl::advertise(&mut d, Chip::R25B, true), Ok(true));
    assert_eq!(d.phy[&mii(rtl::MII_ANAR)] & 0x0c00, 0x0c00);
    let l = rtl::wait_link(&mut d, 5000).unwrap();
    assert!(l.tx_pause && l.rx_pause);
}

#[test]
fn a_phy_that_never_answers_is_a_timeout_not_a_hang() {
    let mut d = Dev::new(Chip::R25B);
    d.phy_stuck = true;
    assert!(matches!(rtl::advertise(&mut d, Chip::R25B, false), Err(Error::PhyTimeout { reg: 0xa408 })));
    assert!(d.now < 100_000, "{} us", d.now);
}

// Bring-up and the rings.

#[test]
fn start_programs_the_rings_and_the_revision_s_settings_then_enables_rx_and_tx() {
    for chip in ALL {
        let (d, net) = started(chip);
        assert_eq!((d.rx_ring(), d.tx_ring()), (DEVICE_BASE, DEVICE_BASE + 1024), "{chip:?}");
        let rxcfg = d.get32(rtl::RXCFG);
        assert_eq!(rxcfg & !rtl::RXCFG_ACCEPT, chip.rxcfg());
        assert_eq!(rxcfg & rtl::RXCFG_ACCEPT, rtl::RXCFG_INDIV | rtl::RXCFG_BROAD | rtl::RXCFG_MULTI);
        assert_eq!((d.get32(rtl::MAR0), d.get32(rtl::MAR4)), (u32::MAX, u32::MAX));
        assert_eq!(d.get32(rtl::TXCFG), rtl::TXCFG_CONFIG);
        assert_eq!(d.get16(rtl::RXMAXSIZE), 2048);
        assert_eq!(d.get32(rtl::IMR), 0, "polled: no interrupts");
        assert_eq!(&d.regs[0..6], &MAC);
        assert_eq!(d.get8(rtl::EECMD) & 0xc0, 0, "config registers locked again");
        assert!(d.tx_enabled && d.rx_enabled && net.running);
        assert!(d.resets >= 2, "OOB exit and chip_init both reset");
        assert_eq!(d.config[&0x70c], 0x2700_0000);
        for i in 0..ring::RX_COUNT as u64 {
            let s = d.mem32(DEVICE_BASE + 32 * i + 28);
            let eor = if i == 31 { ring::EOR } else { 0 };
            assert_eq!(s, ring::OWN | eor | 2048, "{chip:?} rx {i}");
            assert_eq!(d.mem32(DEVICE_BASE + 32 * i + 16), (DEVICE_BASE + 4096 + 2048 * i) as u32);
        }
        assert_eq!(d.mem32(DEVICE_BASE + 1024 + 16 * 31), ring::EOR, "the transmit ring ends");
        // RX and TX come on only after everything else.
        let cmd = d.writes(rtl::CMD);
        assert_eq!(*cmd.last().unwrap(), u32::from(rtl::CMD_TXENB | rtl::CMD_RXENB));
        assert_eq!(*d.ops.last().unwrap(), (rtl::CMD, 0x0c));
    }
}

#[test]
fn a_frame_round_trips_and_the_crc_is_removed() {
    let (mut d, mut net) = started(Chip::R25B);
    d.loopback = true;
    let f = frame_to(MAC, 7);
    assert_eq!(net.transmit(&mut d, &f), Ok(true));
    assert_eq!(d.wire, vec![f.clone()]);
    assert_eq!(d.early_doorbells, 0);
    assert_eq!(net.peek(), Some((60, MAC)));
    let mut out = [0u8; 1600];
    assert_eq!(net.receive::<()>(&mut out), Ok(Some(60)));
    assert_eq!(&out[..60], &f[..]);
    assert_eq!(net.peek(), None);
    net.reclaim();
    assert_eq!(net.tx_pending(), 0);
}

#[test]
fn short_frames_are_padded_to_60_bytes() {
    let (mut d, mut net) = started(Chip::R26(2));
    let mut f = vec![0u8; 14];
    f[0..6].copy_from_slice(&[0xff; 6]);
    f[12] = 0x08;
    f[13] = 0x06;
    net.transmit(&mut d, &f).unwrap();
    assert_eq!(d.wire[0].len(), 60);
    assert_eq!(&d.wire[0][..14], &f[..]);
    assert!(d.wire[0][14..].iter().all(|b| *b == 0));
}

#[test]
fn both_rings_wrap_at_eor() {
    let (mut d, mut net) = started(Chip::R25);
    d.loopback = true;
    let mut out = [0u8; 2048];
    for n in 0..100u8 {
        assert_eq!(net.transmit(&mut d, &frame_to(MAC, n)), Ok(true), "frame {n}");
        assert_eq!(net.receive::<()>(&mut out), Ok(Some(60)), "frame {n}");
        assert_eq!(out[14], n);
    }
    assert_eq!(d.dropped, 0);
}

#[test]
fn a_full_transmit_ring_says_so_instead_of_overwriting() {
    let (mut d, mut net) = started(Chip::R25B);
    d.tx_stalled = true;
    for n in 0..32u8 { assert_eq!(net.transmit(&mut d, &frame_to(MAC, n)), Ok(true)); }
    assert_eq!(net.transmit(&mut d, &frame_to(MAC, 99)), Ok(false));
    assert_eq!(net.tx_pending(), 32);
    // A full ring rings the doorbell again: a chip that missed one catches up.
    d.tx_stalled = false;
    assert_eq!(net.transmit(&mut d, &frame_to(MAC, 98)), Ok(true));
    assert_eq!(d.wire.len(), 33);
    assert_eq!(d.wire[32][14], 98);
}

#[test]
fn bad_frame_lengths_are_refused() {
    let (mut d, mut net) = started(Chip::R25B);
    assert_eq!(net.transmit(&mut d, &[0; 13]), Err(Error::Frame { len: 13 }));
    assert_eq!(net.transmit(&mut d, &[0; 1519]), Err(Error::Frame { len: 1519 }));
}

#[test]
fn errored_and_split_receive_descriptors_are_given_back() {
    let (mut d, mut net) = started(Chip::R25B);
    d.deliver_raw(ring::RX_SOF | ring::RX_EOF | ring::RX_ERR | 64);
    d.deliver_raw(ring::RX_SOF | 2048);
    d.deliver_raw(ring::RX_EOF | 100);
    d.deliver_raw(ring::RX_SOF | ring::RX_EOF | 10);
    d.deliver(&frame_to([0xff; 6], 5));
    assert_eq!(net.peek(), Some((60, [0xff; 6])));
    assert_eq!(net.rx_errors, 4);
    for i in 0..4u64 {
        assert_eq!(d.mem32(DEVICE_BASE + 32 * i + 28), ring::OWN | 2048, "descriptor {i} is the chip's again");
    }
}

#[test]
fn a_frame_too_big_for_the_caller_stays_queued() {
    let (mut d, mut net) = started(Chip::R25B);
    d.deliver(&frame_to(MAC, 1));
    let mut small = [0u8; 20];
    assert_eq!(net.receive::<()>(&mut small), Err(Error::Frame { len: 60 }));
    let mut out = [0u8; 100];
    assert_eq!(net.receive::<()>(&mut out), Ok(Some(60)));
}

// #2: quiesce.

#[test]
fn stop_quiesces_the_chip_and_nothing_is_received_after() {
    let (mut d, mut net) = started(Chip::R25B);
    d.put_isr(0x25);
    let resets = d.resets;
    net.stop(&mut d).unwrap();
    assert!(!d.tx_enabled && !d.rx_enabled && !net.running);
    assert_eq!(d.resets, resets + 1);
    assert_eq!(d.get32(rtl::IMR), 0);
    assert_eq!(d.get32(rtl::ISR), 0, "pending interrupts acknowledged");
    assert_eq!(d.get32(rtl::RXCFG) & rtl::RXCFG_ACCEPT, 0, "nothing accepted");
    d.deliver(&frame_to(MAC, 1));
    assert_eq!(d.dropped, 1);
    assert_eq!(net.peek(), None);
    let stopreq = d.writes(rtl::CMD).iter().rposition(|v| *v & 0x80 != 0).unwrap();
    let reset = d.writes(rtl::CMD).iter().rposition(|v| *v == 0x10).unwrap();
    assert!(stopreq < reset, "stop request before the reset");
}

#[test]
fn a_reset_that_never_completes_is_reported() {
    let (mut d, mut net) = started(Chip::R25B);
    d.reset_stuck = true;
    assert_eq!(net.stop(&mut d), Err(Error::ResetTimeout { cmd: 0x10 }));
}

#[test]
fn a_failed_bring_up_leaves_the_chip_quiesced() {
    let mut d = Dev::new(Chip::R25B);
    let mut net: Net = d.net(Chip::R25B);
    d.phy_stuck = true;
    assert!(net.start(&mut d, false).is_err());
    assert!(!d.tx_enabled && !d.rx_enabled && !net.running);
}

#[test]
fn the_dma_check_waits_for_the_descriptor() {
    let (mut d, mut net) = started(Chip::R25B);
    assert_eq!(ring::check(&mut d, &mut net), Ok(Some(0)));
    assert_eq!(rtl::ring::frame(&d.wire[0]).ethertype, 0x88b5);
    d.tx_stalled = true;
    assert_eq!(ring::check(&mut d, &mut net), Ok(None));
}

#[test]
fn promiscuous_sets_allphys_only_while_running() {
    let (mut d, mut net) = started(Chip::R25B);
    net.set_promiscuous(&mut d, true).unwrap();
    assert_eq!(d.get32(rtl::RXCFG) & rtl::RXCFG_ALLPHYS, rtl::RXCFG_ALLPHYS);
    net.set_promiscuous(&mut d, false).unwrap();
    assert_eq!(d.get32(rtl::RXCFG) & rtl::RXCFG_ALLPHYS, 0);
    net.stop(&mut d).unwrap();
    net.set_promiscuous(&mut d, true).unwrap();
    assert_eq!(d.get32(rtl::RXCFG) & rtl::RXCFG_ACCEPT, 0, "a stopped chip accepts nothing");
}

trait Isr { fn put_isr(&mut self, v: u32); }
impl Isr for Dev {
    fn put_isr(&mut self, v: u32) { self.regs[0x3c..0x40].copy_from_slice(&v.to_le_bytes()); }
}
