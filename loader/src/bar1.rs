// SPDX-License-Identifier: GPL-2.0-only
//! Standard PCIe Resizable BAR control for the GPU's BAR1.
//!
//! The payload raises the hardware aperture limit to 64 GiB; this module
//! selects the 64 GiB entry in the standard ReBAR capability after FLR.
//! It deliberately does not move the BAR address or touch bridge windows:
//! Linux with `pci=realloc` owns the resource assignment.

use log::info;

use crate::pci::PciIo;
use crate::tx::{regs, TxError, TxGpu};

const PCI_EXT_CAP_ID_REBAR: u16 = 0x15;
const PCI_REBAR_CAP: u32 = 0x04;
const PCI_REBAR_CTRL: u32 = 0x08;
const PCI_REBAR_CTRL_BAR_IDX: u32 = 0x7;
const PCI_REBAR_CTRL_NBAR_MASK: u32 = 0xe0;
const PCI_REBAR_CTRL_NBAR_SHIFT: u32 = 5;
const PCI_REBAR_CTRL_BAR_SIZE: u32 = 0x1f00;
const PCI_REBAR_CAP_SIZES: u32 = 0x00ff_fff0;

/// 64 GiB = 2^16 MiB. The size field encodes the power-of-two index with
/// 1 MiB at index 0.
const SIZE_64G_INDEX: u32 = 16;
const SIZE_64G_BIT: u32 = 1 << 20; // bit 20 of the raw CAP dword

struct Entry {
    ctrl_off: u32,
    cap: u32,
    ctrl: u32,
}

fn find_bar1(pci: &PciIo) -> Option<Entry> {
    let cap = pci.find_ext_cap(PCI_EXT_CAP_ID_REBAR)? as u32;
    let first = pci.config_read_u32(cap + PCI_REBAR_CTRL);
    let count = (first & PCI_REBAR_CTRL_NBAR_MASK) >> PCI_REBAR_CTRL_NBAR_SHIFT;
    for i in 0..count {
        let ctrl_off = cap + PCI_REBAR_CTRL + i * 8;
        let ctrl = pci.config_read_u32(ctrl_off);
        if ctrl & PCI_REBAR_CTRL_BAR_IDX != 1 {
            continue;
        }
        let cap_off = cap + PCI_REBAR_CAP + i * 8;
        let cap = pci.config_read_u32(cap_off);
        return Some(Entry { ctrl_off, cap, ctrl });
    }
    None
}

/// Snapshot the BAR1 size control before the payload changes it. The
/// capability is captured too, but the control is what Linux reads as the
/// current BAR1 size.
pub fn capture(pci: &PciIo) -> Option<u32> {
    find_bar1(pci).map(|e| e.ctrl)
}

/// Put the BAR1 size control back to the pre-payload selection. The 64 GiB
/// capability stays in the ReBAR CAP; only the current aperture shrinks back
/// to what firmware/Linux had. This is the bar1 payload without a 64 GiB
/// resource assignment.
pub fn restore(pci: &PciIo, saved: u32) -> Result<(), TxError> {
    let Some(e) = find_bar1(pci) else {
        info!("ReBAR: no BAR1 ReBAR entry to restore");
        return Err(TxError::Unsupported);
    };
    let current = e.ctrl;
    let new = (current & !PCI_REBAR_CTRL_BAR_SIZE) | (saved & PCI_REBAR_CTRL_BAR_SIZE);
    pci.config_write_u32(e.ctrl_off, new);
    let rb = pci.config_read_u32(e.ctrl_off);
    info!(
        "ReBAR: restored BAR1 size control to {:#010x} (readback {rb:#010x}, size_index={})",
        saved,
        (rb & PCI_REBAR_CTRL_BAR_SIZE) >> 8
    );
    if rb & PCI_REBAR_CTRL_BAR_SIZE != saved & PCI_REBAR_CTRL_BAR_SIZE {
        return Err(TxError::Io);
    }
    report(pci, "after-restore");
    Ok(())
}

pub fn report(pci: &PciIo, phase: &str) {
    let Some(e) = find_bar1(pci) else {
        info!("ReBAR {phase}: no ReBAR capability with a BAR1 entry");
        return;
    };
    let possible = (e.cap & PCI_REBAR_CAP_SIZES) >> 4;
    let size = (e.ctrl & PCI_REBAR_CTRL_BAR_SIZE) >> 8;
    let bar1_lo = pci.config_read_u32(0x14) & 0xffff_fff0;
    let bar1_hi = pci.config_read_u32(0x18);
    info!(
        "ReBAR {phase}: CAP={:#010x} possible={possible:#x} 64G_supported={} ctrl={:#010x} size_index={size} BAR1={bar1_hi:#010x}{bar1_lo:08x}",
        e.cap,
        e.cap & SIZE_64G_BIT != 0,
        e.ctrl,
    );
}

/// A card from a previous boot can still be unlocked (the FEAT PLM lives on
/// the AON island). If it carries the bar1 construct, apply the ReBAR size
/// even though the transaction is skipped. If it carries the tensor-only
/// construct, leave ReBAR alone.
pub fn apply_to_existing(gpu: &TxGpu) -> Result<(), TxError> {
    let cya = gpu.read32_raw(regs::XVE_CYA);
    let cfg = gpu.read32_raw(regs::XVE_CFG);
    if (cya & 0x3f) == 0x30 && (cfg & 0x8000_000f) == 0x8000_000a {
        info!("ReBAR: already-unlocked card carries the bar1 construct");
        select_64g(gpu.pci)
    } else {
        info!("ReBAR: already-unlocked card has no bar1 construct; leaving ReBAR alone");
        Ok(())
    }
}

/// Select 64 GiB for BAR1 without moving the BAR. Linux's `pci=realloc`
/// will assign an aligned address and rebuild the bridge windows.
pub fn select_64g(pci: &PciIo) -> Result<(), TxError> {
    let Some(e) = find_bar1(pci) else {
        info!("ReBAR: no BAR1 ReBAR entry");
        return Err(TxError::Unsupported);
    };
    if e.cap & SIZE_64G_BIT == 0 {
        info!("ReBAR: capability does not advertise 64 GiB ({:#010x})", e.cap);
        return Err(TxError::Unsupported);
    }
    let old = e.ctrl;
    let new = (old & !PCI_REBAR_CTRL_BAR_SIZE) | (SIZE_64G_INDEX << 8);
    pci.config_write_u32(e.ctrl_off, new);
    let rb = pci.config_read_u32(e.ctrl_off);
    info!(
        "ReBAR: BAR1 size control {:#010x} -> {:#010x}, readback {rb:#010x} (64G selected={})",
        old,
        new,
        rb & PCI_REBAR_CTRL_BAR_SIZE == SIZE_64G_INDEX << 8
    );
    if rb & PCI_REBAR_CTRL_BAR_SIZE != SIZE_64G_INDEX << 8 {
        return Err(TxError::Io);
    }
    report(pci, "after-select");
    Ok(())
}
