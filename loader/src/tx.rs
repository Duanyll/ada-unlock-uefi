// SPDX-License-Identifier: GPL-2.0-only
//! AD102 transaction: FWSEC-FRTS, Booter with the V67 payload, selector
//! writes, one FLR, and the optional 64 GiB BAR1 construct.
//!
//! Ported from JustAResearcher/gpu-unlock-ada's `unlock_610.c` and
//! physicsdolphin/nv-uefi-unlocker's UEFI transaction. The sequence is kept;
//! the marker-file experiments, diagnostics-only probes and Windows display
//! handoff are not.

use core::time::Duration;

use log::info;
use uefi::boot::stall;

use crate::pci::{BridgeMaster, PciIo};

pub mod eng {
    pub const GSP: u32 = 0x0011_0000;
    pub const SEC2: u32 = 0x0084_0000;
    pub const SEC2_FBIF: u32 = 0x0084_0600;
    pub const SEC2_FALCON2: u32 = 0x0084_1000;
    pub const GSP_FBIF: u32 = 0x0011_0600;
    pub const GSP_FALCON2: u32 = 0x0011_1000;
}

pub mod regs {
    pub const GFW_BOOT: u32 = 0x0011_8234;
    pub const GFW_CPUCTL: u32 = 0x0011_0100;
    pub const WPR2_LO: u32 = 0x001f_a824;
    pub const WPR2_HI: u32 = 0x001f_a828;
    pub const FEAT_PLM: u32 = 0x0082_3804;
    pub const FEAT_SS0: u32 = 0x0082_381c;
    pub const FEAT_SS1: u32 = 0x0082_3820;
    pub const FEAT_READOUT: u32 = 0x0082_3818;
    pub const WPR_PLM: u32 = 0x001f_a7c4;
    pub const WPRCFG_PLM: u32 = 0x001f_a7cc;
    pub const FBPA_PLM: u32 = 0x009a_0148;
    pub const XVE_CYA: u32 = 0x0008_8724;
    pub const XVE_CFG: u32 = 0x0008_8dcc;
    pub const XVE_PLM: u32 = 0x0008_8ff4;
    pub const SEC2_MAILBOX0: u32 = 0x0084_0040;
}

pub const SS0_UNLOCK: u32 = 0x8888_8888;
pub const SS1_UNLOCK: u32 = 0x8;
pub const TENSOR_MARKER: u32 = 0x67b0_0001;
pub const BAR1_MARKER: u32 = 0x67b0_0004;
pub const V67_SIZE: usize = 0xFA00;

#[derive(Debug, Clone, Copy)]
pub enum TxError {
    Io,
    NoDevice,
    Timeout,
    Invalid,
    Range,
    Busy,
    Already,
    Unsupported,
}

pub fn inaccessible(value: u32) -> bool {
    value == !0u32 || (value & 0xffff_0000) == 0xbadf_0000
}

pub struct TxGpu<'a> {
    pub pci: &'a PciIo,
    pub bar: u8,
}

impl TxGpu<'_> {
    pub fn read32(&self, off: u32) -> Result<u32, TxError> {
        let v = self.pci.bar_read_u32(self.bar, off as u64);
        if inaccessible(v) {
            info!("BAR0 read {off:#x} returned {v:#010x}");
            return Err(TxError::Io);
        }
        Ok(v)
    }

    pub fn read32_raw(&self, off: u32) -> u32 {
        self.pci.bar_read_u32(self.bar, off as u64)
    }

    pub fn write32(&self, off: u32, v: u32) {
        self.pci.bar_write_u32(self.bar, off as u64, v);
    }

    pub fn poll_us(
        &self,
        off: u32,
        mask: u32,
        expect: u32,
        budget_us: u32,
    ) -> Result<(), TxError> {
        let step_us = 75u32;
        for _ in 0..(budget_us / step_us + 1) {
            let value = self.pci.bar_read_u32(self.bar, off as u64);
            if inaccessible(value) {
                info!("poll {off:#x}: inaccessible value {value:#010x}");
                return Err(TxError::Io);
            }
            if value & mask == expect {
                return Ok(());
            }
            stall(Duration::from_micros(step_us as u64));
        }
        info!("poll {off:#x}: timeout after {budget_us} us (last value did not match {expect:#x}/{mask:#x})");
        Err(TxError::Timeout)
    }
}

/// Falcon engine reset. `falcon2` is the Falcon v4 register block; on
/// RISC-V-capable engines the BCR is forced to the Falcon core because the
/// Falcon-compat DMA queue stalls behind a RISC-V selection.
pub fn falcon_reset(gpu: &TxGpu, base: u32, falcon2: u32) -> Result<(), TxError> {
    info!("falcon reset: base={base:#x}");
    // HW bug 3419321: the pre-reset idle wait may time out and is tolerated.
    let _ = gpu.poll_us(base + 0xf4, 1 << 31, 1 << 31, 1000);
    let engine = gpu.read32(base + 0x3c0)?;
    gpu.write32(base + 0x3c0, engine | 1);
    for _ in 0..10 {
        let _ = gpu.pci.bar_read_u32(gpu.bar, (base + 0x3c0) as u64);
    }
    gpu.write32(base + 0x3c0, engine & !1);
    for _ in 0..10 {
        let _ = gpu.pci.bar_read_u32(gpu.bar, (base + 0x3c0) as u64);
    }
    gpu.poll_us(base + 0xf4, 1 << 12, 0, 1_000_000)?;
    let hwcfg = gpu.read32(base + 0xf4)?;
    if hwcfg & (1 << 10) != 0 {
        let bcr = gpu.read32(falcon2 + 0x668)?;
        let falcon_selected = (bcr & 1) != 0 && (bcr >> 4 & 1) == 0;
        if !falcon_selected {
            gpu.write32(falcon2 + 0x668, 0);
            gpu.poll_us(falcon2 + 0x668, 1, 1, 1_000_000)?;
            info!("falcon reset: BCR {bcr:#010x} -> {:#010x}", gpu.read32(falcon2 + 0x668)?);
        }
    }
    let chip = gpu.read32(0)?;
    gpu.write32(base + 0x84, chip);
    info!("falcon reset: done hwcfg={hwcfg:#010x} chip={chip:#010x}");
    Ok(())
}

/// Open the engine's physical, context-less DMA path. Without this the
/// ucode's own DMA requests queue but are never serviced.
pub fn falcon_dma_unblock(gpu: &TxGpu, base: u32, fbif: u32) -> Result<(), TxError> {
    let mut ctl = gpu.read32(fbif + 0x24)?;
    ctl |= 1 << 7;
    gpu.write32(fbif + 0x24, ctl);
    gpu.write32(base + 0x10c, 0);
    let mut trans = gpu.read32(fbif)?;
    trans = (trans & !7) | 5;
    gpu.write32(fbif, trans);
    let ctl_rb = gpu.read32(fbif + 0x24)?;
    let trans_rb = gpu.read32(fbif)?;
    let dmactl_rb = gpu.read32(base + 0x10c)?;
    info!("falcon DMA: FBIF_CTL={ctl_rb:#010x} TRANSCFG={trans_rb:#010x} DMACTL={dmactl_rb:#010x}");
    if ctl_rb & (1 << 7) == 0 || trans_rb & 7 != 5 || dmactl_rb != 0 {
        return Err(TxError::Io);
    }
    Ok(())
}

/// Cold-state checks. `EAlready` means the card is already unlocked and is
/// skipped; `EBusy` means a driver has used it since power-on, so the
/// firmware window is gone.
pub fn claim_guards(gpu: &TxGpu) -> Result<(), TxError> {
    let chip = gpu.read32_raw(0);
    if chip == !0u32 {
        info!("claim: BAR0 reads all ones - the GPU is powered off or not decoded");
        return Err(TxError::NoDevice);
    }
    let plm = gpu.read32_raw(regs::FEAT_PLM);
    if plm == !0u32 {
        // All-ones is also what a dead FEAT block returns. SS0 is in the
        // same block; if it is all-ones too, this is not an open PLM.
        let ss0 = gpu.read32_raw(regs::FEAT_SS0);
        if ss0 == !0u32 {
            info!("claim: FEAT block reads all ones (PLM and SS0); not treating this as unlocked");
            return Err(TxError::NoDevice);
        }
        info!("claim: FEAT_PLM is open - already unlocked");
        return Err(TxError::Already);
    }
    let wpr_hi = gpu.read32(regs::WPR2_HI)?;
    if wpr_hi >> 4 != 0 {
        info!("claim: WPR2 is enabled (hi={wpr_hi:#x}) - a driver has initialized this card since power-on; power off first");
        return Err(TxError::Busy);
    }
    info!("claim: cold card, FEAT_PLM={plm:#010x}, WPR2_HI={wpr_hi:#x}");
    Ok(())
}

pub struct TxAssets {
    pub booter: alloc::vec::Vec<u8>,
    pub boot_desc: alloc::vec::Vec<u8>,
    pub gsp_boot: alloc::vec::Vec<u8>,
    pub gsp_image: alloc::vec::Vec<u8>,
    pub stock_sig: alloc::vec::Vec<u8>,
    pub v67: alloc::vec::Vec<u8>,
    pub bar1_64g: bool,
}

pub fn validate_asset_sizes(assets: &TxAssets) -> Result<(), TxError> {
    if assets.v67.len() != V67_SIZE {
        info!(
            "asset: payload is {} bytes, want {V67_SIZE} (0x{V67_SIZE:x})",
            assets.v67.len()
        );
        return Err(TxError::Invalid);
    }
    if assets.stock_sig.len() != 4096 {
        info!("asset: stock signature is {} bytes, want 4096", assets.stock_sig.len());
        return Err(TxError::Invalid);
    }
    if assets.gsp_image.is_empty() || assets.gsp_image.len() > 128 << 20 {
        info!("asset: GSP image is {} bytes", assets.gsp_image.len());
        return Err(TxError::Range);
    }
    Ok(())
}

/// DMA buffers held across the whole transaction so a failure can FLR the
/// card before they are unmapped. A live SEC2 that still DMA-reads a freed
/// buffer is a host-memory corruption, not just a failed unlock.
struct DmaBuffers {
    booter: crate::pci::DmaBuffer,
    sig: crate::pci::DmaBuffer,
    meta: crate::pci::DmaBuffer,
    stock: crate::pci::DmaBuffer,
    boot: crate::pci::DmaBuffer,
    radix: Option<crate::tx_booter::RadixBlock>,
}

/// One card, full transaction. On any error after DMA allocation the card
/// is FLR'd before the buffers drop; on success the transaction's own FLR
/// has already run.
pub fn run_transaction(gpu: &TxGpu, assets: &TxAssets, rebar64: bool) -> Result<(), TxError> {
    info!("TX: begin");
    if let Some(cap) = gpu.pci.pm_cap() {
        let v = gpu.pci.config_read_u16(cap as u32 + 4);
        if v & 3 != 0 {
            info!("PM: forcing D0 (PMCSR={v:#06x})");
            gpu.pci.config_write_u16(cap as u32 + 4, v & !3);
            stall(Duration::from_millis(20));
        }
    }
    gpu.pci.enable_decode_and_master().map_err(|_| TxError::NoDevice)?;
    claim_guards(gpu)?;
    if assets.bar1_64g {
        let cya = gpu.read32_raw(regs::XVE_CYA);
        let cfg = gpu.read32_raw(regs::XVE_CFG);
        info!("TX-bar1: pre-check CYA={cya:#x} CFG={cfg:#010x}");
        if cya & !0x3f != 0 || cfg & !0x8000_000f != 0 {
            info!("TX-bar1: reserved bits set in CYA/CFG - this is not the known AD102 layout");
            return Err(TxError::Unsupported);
        }
    }

    let mut dma = DmaBuffers {
        booter: gpu.pci.dma_alloc(assets.booter.len()).map_err(|_| TxError::NoDevice)?,
        sig: gpu.pci.dma_alloc(V67_SIZE).map_err(|_| TxError::NoDevice)?,
        meta: gpu.pci.dma_alloc(crate::tx_booter::WPR_META_SIZE).map_err(|_| TxError::NoDevice)?,
        stock: gpu.pci.dma_alloc(4096).map_err(|_| TxError::NoDevice)?,
        boot: gpu.pci.dma_alloc(assets.gsp_boot.len()).map_err(|_| TxError::NoDevice)?,
        radix: None,
    };
    let result = run_with_dma(gpu, assets, rebar64, &mut dma);
    if result.is_err() {
        info!("TX: failed; FLR before releasing the DMA buffers");
        let _ = flr_reset(gpu.pci);
    }
    result
}

fn run_with_dma(
    gpu: &TxGpu,
    assets: &TxAssets,
    rebar64: bool,
    dma: &mut DmaBuffers,
) -> Result<(), TxError> {
    use crate::tx_booter as tb;
    use crate::tx_fwsec;

    let layout = tx_fwsec::layout_from_registers(
        gpu,
        assets.gsp_image.len() as u64,
        assets.gsp_boot.len() as u64,
    )?;
    tx_fwsec::fwsec_ignite(gpu, &layout)?;

    let desc = tb::boot_desc_parse(&assets.boot_desc, assets.gsp_boot.len() as u32)?;
    let (hdr, booter_image, booter_sigs) = tb::load_booter(&assets.booter)?;

    unsafe {
        core::ptr::copy_nonoverlapping(booter_image.as_ptr(), dma.booter.host, booter_image.len());
        core::ptr::copy_nonoverlapping(assets.v67.as_ptr(), dma.sig.host, V67_SIZE);
        core::ptr::copy_nonoverlapping(assets.stock_sig.as_ptr(), dma.stock.host, assets.stock_sig.len());
        core::ptr::copy_nonoverlapping(assets.gsp_boot.as_ptr(), dma.boot.host, assets.gsp_boot.len());
    }

    let fuse_ver = if hdr.num_sigs == 1 {
        0
    } else {
        let raw = gpu.read32(0x0082_41c0 + (hdr.ucode_id - 1) * 4)?;
        tx_fwsec::fuse_version(raw)
    };
    info!("TX: booter signature fuse version {fuse_ver}");
    {
        let image = unsafe { core::slice::from_raw_parts_mut(dma.booter.host, booter_image.len()) };
        tb::booter_patch_stock(image, &hdr, booter_sigs, fuse_ver)?;
    }

    let radix = tb::radix_build(gpu, &assets.gsp_image)?;
    let level0 = radix.level0;
    dma.radix = Some(radix);

    {
        let meta = unsafe { core::slice::from_raw_parts_mut(dma.meta.host, tb::WPR_META_SIZE) };
        tb::wpr_meta_pack(
            meta,
            &layout,
            1 << 20,
            dma.sig.dev,
            V67_SIZE as u64,
            level0,
            assets.gsp_image.len() as u64,
            dma.boot.dev,
            &desc,
        )?;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    info!("TX: WprMeta published");

    let _bridge = BridgeMaster::enable(gpu.pci);
    {
        let booter_view = unsafe { core::slice::from_raw_parts(dma.booter.host, booter_image.len()) };
        tb::booter_fire(gpu, &hdr, booter_view, dma.meta.dev)?;
    }
    let meta_view = unsafe { core::slice::from_raw_parts(dma.meta.host, tb::WPR_META_SIZE) };
    tb::verify_execution(gpu, assets.bar1_64g, meta_view)?;

    tb::write_selector(gpu, "SS0", regs::FEAT_SS0, SS0_UNLOCK)?;
    tb::write_selector(gpu, "SS1", regs::FEAT_SS1, SS1_UNLOCK)?;
    info!("TX: selectors applied");

    {
        let meta = unsafe { core::slice::from_raw_parts_mut(dma.meta.host, tb::WPR_META_SIZE) };
        tb::signature_restore(meta, dma.stock.dev, assets.stock_sig.len() as u64);
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    info!("TX: stock signature restored");

    tb::flr_phase(gpu, assets.bar1_64g, rebar64)?;
    handoff_log(gpu, assets.bar1_64g);
    info!("TX: card complete");
    Ok(())
}

pub fn handoff_log(gpu: &TxGpu, bar1_64g: bool) {
    let ss0 = gpu.read32_raw(regs::FEAT_SS0);
    let ss1 = gpu.read32_raw(regs::FEAT_SS1);
    let plm = gpu.read32_raw(regs::FEAT_PLM);
    let readout = gpu.read32_raw(regs::FEAT_READOUT);
    let wpr_lo = gpu.read32_raw(regs::WPR2_LO);
    let wpr_hi = gpu.read32_raw(regs::WPR2_HI);
    info!(
        "HANDOFF: SS0={ss0:#010x} SS1={ss1:#x} FEAT_PLM={plm:#010x} READOUT={readout:#010x} WPR2={wpr_lo:#x}/{wpr_hi:#x}"
    );
    if bar1_64g {
        info!(
            "HANDOFF-bar1: CYA={:#x} CFG={:#010x} XVE_PLM={:#010x}",
            gpu.read32_raw(regs::XVE_CYA),
            gpu.read32_raw(regs::XVE_CFG),
            gpu.read32_raw(regs::XVE_PLM),
        );
    }
    let cmd = gpu.pci.config_read_u16(0x04);
    info!("HANDOFF: PCI command={cmd:#06x} (MEMORY={} BUS_MASTER={})", cmd & 2 != 0, cmd & 4 != 0);
}

/// One FLR with the config state restored afterwards. Bars are restored with
/// decode off; bus master is deliberately left off so Linux owns it.
pub fn flr_reset(pci: &PciIo) -> Result<(), TxError> {
    let pm = pci.pm_cap();
    let pmcsr = pm.map(|cap| pci.config_read_u16(cap as u32 + 4));
    if let (Some(cap), Some(v)) = (pm, pmcsr) {
        if v & 3 != 0 {
            pci.config_write_u16(cap as u32 + 4, v & !3);
            stall(Duration::from_millis(20));
        }
    }

    let mut saved = [0u32; 10];
    for (i, slot) in saved.iter_mut().enumerate() {
        *slot = pci.config_read_u32(0x04 + i as u32 * 4);
    }

    let pcie = pci.pcie_cap().ok_or(TxError::Unsupported)?;
    let devcap = pci.config_read_u32(pcie as u32 + 0x04);
    if devcap & (1 << 28) == 0 {
        info!("FLR: device does not advertise FLR support");
        return Err(TxError::Unsupported);
    }
    let devctl = pci.config_read_u16(pcie as u32 + 0x08);
    for _ in 0..1000 {
        let devsta = pci.config_read_u16(pcie as u32 + 0x0a);
        if devsta == 0xffff || devsta & 0x20 == 0 {
            break;
        }
        stall(Duration::from_millis(1));
    }

    pci.config_write_u16(0x04, 0x0003); // decode on, BME off
    let cmd = pci.config_read_u16(0x04);
    if cmd & 0x0004 != 0 {
        info!("FLR: BUS_MASTER did not clear (command={cmd:#06x})");
        return Err(TxError::Io);
    }
    pci.config_write_u16(pcie as u32 + 0x08, devctl | 0x8000);
    info!("FLR: triggered");
    stall(Duration::from_millis(100));
    pci.config_write_u16(pcie as u32 + 0x08, devctl);

    for _ in 0..1000 {
        let id = pci.config_read_u32(0x00);
        if id != !0u32 && id != 0 && id & 0xffff == 0x10de {
            break;
        }
        stall(Duration::from_millis(1));
    }

    pci.config_write_u16(0x04, 0x0000);
    for i in (2..10).rev() {
        pci.config_write_u32(0x04 + i as u32 * 4, saved[i]);
    }
    pci.config_write_u16(0x04, (saved[0] as u16) & !0x0004);
    info!("FLR: config restored, command={:#06x}", pci.config_read_u16(0x04));

    if let (Some(cap), Some(v)) = (pm, pmcsr) {
        pci.config_write_u16(cap as u32 + 4, v & !3);
    }
    Ok(())
}

/// Last-resort cleanup after a failed transaction. The card is forced
/// through the same FLR so Linux receives a cold-boot-equivalent function.
pub fn emergency_flr(pci: &PciIo) {
    info!("EMERGENCY: resetting the card after a failed transaction");
    if let Err(e) = flr_reset(pci) {
        info!("EMERGENCY: FLR failed: {e:?}");
    }
}
