// SPDX-License-Identifier: GPL-2.0-only
//! Booter bundle handling, WprMeta publication, the SEC2 fire, selector
//! writes, execution verification, and the FLR that hands the card to Linux.

use core::time::Duration;

use log::info;
use uefi::boot::stall;

use crate::bar1;
use crate::pci::DmaBuffer;
use crate::tx::{eng, regs, TxError, TxGpu, BAR1_MARKER, SS0_UNLOCK, SS1_UNLOCK, TENSOR_MARKER};
use crate::tx_fwsec::LayoutOut;

pub const WPR_META_SIZE: usize = 256;
pub const WPR_META_MAGIC: u64 = 0xdc3a_ae21_371a_60b3;

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn wr64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

pub struct BootDesc {
    pub code_offset: u32,
    pub data_offset: u32,
    pub manifest_offset: u32,
    pub image_size: u32,
}

/// Parse the 84-byte GSP boot descriptor used by the AD10x BL image.
pub fn boot_desc_parse(desc: &[u8], image_size: u32) -> Result<BootDesc, TxError> {
    let f = |i: usize| le32(desc, i * 4);
    if desc.len() != 84
        || f(0) != 5
        || f(14) != 1
        || f(19) != image_size
        || f(20) != 0
    {
        return Err(TxError::Invalid);
    }
    for &i in &[1usize, 3, 8, 10, 12] {
        let (off, len) = (f(i), f(i + 1));
        if len == 0 || off > image_size || len > image_size - off {
            return Err(TxError::Range);
        }
    }
    if f(5) != 0 || f(6) != 0 || f(7) != 0 || f(15) != 0 || f(16) != 0 || f(17) != 0 || f(18) != 0 {
        return Err(TxError::Unsupported);
    }
    Ok(BootDesc {
        manifest_offset: f(8),
        data_offset: f(10),
        code_offset: f(12),
        image_size,
    })
}

pub struct BooterHdr {
    pub imem_ns_off: u32,
    pub imem_ns_size: u32,
    pub data_off: u32,
    pub data_size: u32,
    pub app_code_off: u32,
    pub app_code_size: u32,
    pub patch_loc: u32,
    pub engine_id: u32,
    pub ucode_id: u32,
    pub num_sigs: u32,
    pub sig_bytes: u32,
}

const BTL1_MAGIC: u32 = 0x314c_5442;

/// Parse the pinned AD102 BooterLoad bundle:
/// 72-byte BTL1 header + 56832-byte image + two 384-byte signatures.
pub fn load_booter(bundle: &[u8]) -> Result<(BooterHdr, &[u8], &[u8]), TxError> {
    if bundle.len() < 72 || le32(bundle, 0) != BTL1_MAGIC || le32(bundle, 4) != 1 {
        return Err(TxError::Invalid);
    }
    let h = |i: usize| le32(bundle, 8 + i * 4);
    let image_size = le32(bundle, 8 + 14 * 4);
    let sig_total = le32(bundle, 8 + 15 * 4);
    let num_sigs = h(12);
    if num_sigs == 0 || sig_total == 0 || sig_total % num_sigs != 0 || h(13) != sig_total {
        return Err(TxError::Invalid);
    }
    let sig_bytes = sig_total / num_sigs;
    let hdr = BooterHdr {
        imem_ns_off: h(0),
        imem_ns_size: h(1),
        data_off: h(2),
        data_size: h(3),
        app_code_off: h(5),
        app_code_size: h(6),
        patch_loc: h(9),
        ucode_id: h(10),
        engine_id: h(11),
        num_sigs,
        sig_bytes,
    };
    if image_size != 56832
        || hdr.patch_loc != 32272
        || hdr.ucode_id != 3
        || hdr.engine_id != 1
        || hdr.num_sigs != 2
        || hdr.sig_bytes != 384
        || bundle.len() != 72 + image_size as usize + sig_total as usize
    {
        info!(
            "booter: geometry mismatch image={image_size} patch={:#x} ucode={:#x} sigs={} total={sig_total}",
            hdr.patch_loc, hdr.ucode_id, hdr.num_sigs
        );
        return Err(TxError::Invalid);
    }
    for (off, size) in [
        (hdr.imem_ns_off, hdr.imem_ns_size),
        (hdr.data_off, hdr.data_size),
        (hdr.app_code_off, hdr.app_code_size),
    ] {
        if off as u64 + size as u64 > image_size as u64 || off % 256 != 0 || size % 256 != 0 {
            return Err(TxError::Invalid);
        }
    }
    Ok((
        hdr,
        &bundle[72..72 + image_size as usize],
        &bundle[72 + image_size as usize..],
    ))
}

/// Install the fuse-selected stock signature so the BROM accepts the booter.
pub fn booter_patch_stock(
    image: &mut [u8],
    hdr: &BooterHdr,
    signatures: &[u8],
    fuse_ver: u32,
) -> Result<(), TxError> {
    if hdr.sig_bytes == 0 || fuse_ver >= hdr.num_sigs {
        return Err(TxError::Range);
    }
    let index = (hdr.num_sigs - 1 - fuse_ver) as usize;
    let from = index * hdr.sig_bytes as usize;
    let end = from + hdr.sig_bytes as usize;
    let at = hdr.patch_loc as usize;
    if end > signatures.len() || at + hdr.sig_bytes as usize > image.len() {
        return Err(TxError::Invalid);
    }
    image[at..at + hdr.sig_bytes as usize]
        .copy_from_slice(&signatures[from..end]);
    info!("booter: stock signature slot {index} installed");
    Ok(())
}

pub struct RadixBlock {
    _dma: DmaBuffer,
    pub level0: u64,
}

/// Three-level radix over the GSP image, carved from one contiguous DMA
/// block. The booter walks this to copy the 84 MiB image into the WPR.
pub fn radix_build(gpu: &TxGpu, image: &[u8]) -> Result<RadixBlock, TxError> {
    const PAGE: usize = 4096;
    let leaves = image.len().div_ceil(PAGE);
    let l2 = leaves.div_ceil(512);
    let l1 = l2.div_ceil(512).max(1);
    let tables = 1 + l1 + l2;
    let total = tables + leaves;
    let dma = gpu.pci.dma_alloc(total * PAGE).map_err(|_| TxError::NoDevice)?;
    let mem = unsafe { core::slice::from_raw_parts_mut(dma.host, total * PAGE) };
    mem.fill(0);
    let dev = dma.dev;
    let page_addr = |i: usize| dev + (i as u64) * PAGE as u64;
    for i in 0..leaves {
        let off = i * PAGE;
        let n = (image.len() - off).min(PAGE);
        mem[tables * PAGE + off..tables * PAGE + off + n].copy_from_slice(&image[off..off + n]);
    }
    let mut first = 0usize;
    for level in 0..3 {
        let counts = [1usize, l1, l2, leaves];
        let child = first + counts[level];
        for i in 0..counts[level] {
            let at = i * (PAGE / 8);
            let n = (counts[level + 1] - at).min(PAGE / 8);
            for e in 0..n {
                let addr = page_addr(child + at + e);
                wr64(mem, (first + i) * PAGE + e * 8, addr);
            }
        }
        first = child;
    }
    info!("radix: leaves={leaves} tables={tables} level0={dev:#x}");
    Ok(RadixBlock { _dma: dma, level0: dev })
}

/// Publish the WprMeta page the booter consumes. Only the AD102 geometry is
/// used here; the Turing heap variant is not part of this app.
pub fn wpr_meta_pack(
    meta: &mut [u8],
    layout: &LayoutOut,
    frts_size: u64,
    signature_dma: u64,
    signature_size: u64,
    radix_dma: u64,
    image_size: u64,
    boot_dma: u64,
    boot: &BootDesc,
) -> Result<(), TxError> {
    if signature_size == 0
        || image_size == 0
        || boot.image_size == 0
        || layout.vga_base >= layout.fb_size
        || radix_dma & 4095 != 0
        || boot_dma & 4095 != 0
        || layout.frts_offset >= layout.wpr_end
        || frts_size != layout.wpr_end - layout.frts_offset
        || layout.wpr_end > layout.vga_base
        || image_size > layout.boot_offset - layout.gsp_offset
        || boot.image_size as u64 > layout.frts_offset - layout.boot_offset
    {
        return Err(TxError::Invalid);
    }
    meta.fill(0);
    wr64(meta, 0, WPR_META_MAGIC);
    wr64(meta, 8, 1);
    wr64(meta, 16, radix_dma);
    wr64(meta, 24, image_size);
    wr64(meta, 32, boot_dma);
    wr64(meta, 40, boot.image_size as u64);
    wr64(meta, 48, boot.code_offset as u64);
    wr64(meta, 56, boot.data_offset as u64);
    wr64(meta, 64, boot.manifest_offset as u64);
    wr64(meta, 72, signature_dma);
    wr64(meta, 80, signature_size);
    wr64(meta, 88, layout.nonwpr_start);
    wr64(meta, 96, layout.nonwpr_start);
    wr64(meta, 104, layout.wpr_start - layout.nonwpr_start);
    wr64(meta, 112, layout.wpr_start);
    wr64(meta, 120, layout.heap_offset);
    wr64(meta, 128, layout.gsp_offset - layout.heap_offset);
    wr64(meta, 136, layout.gsp_offset);
    wr64(meta, 144, layout.boot_offset);
    wr64(meta, 152, layout.frts_offset);
    wr64(meta, 160, frts_size);
    wr64(meta, 168, layout.wpr_end);
    wr64(meta, 176, layout.fb_size);
    wr64(meta, 184, layout.vga_base);
    wr64(meta, 192, layout.fb_size - layout.vga_base);
    meta[241] = 0;
    Ok(())
}

pub fn signature_restore(meta: &mut [u8], stock_dma: u64, stock_size: u64) {
    wr64(meta, 72, stock_dma);
    wr64(meta, 80, stock_size);
}

pub fn write_selector(gpu: &TxGpu, name: &str, offset: u32, unlock: u32) -> Result<(), TxError> {
    let value = gpu.read32(offset)?;
    info!("selector {name}: read {value:#x}, want {unlock:#x}");
    if value == unlock {
        return Ok(());
    }
    gpu.write32(offset, unlock);
    let value = gpu.read32(offset)?;
    info!("selector {name}: wrote {unlock:#x}, readback {value:#x}");
    if value != unlock {
        return Err(TxError::Io);
    }
    Ok(())
}

/// Load the secure application segment and DMEM into SEC2 through the
/// register portal, program the BROM parameters, start the core, and wait
/// for HALT. This is the UEFI-proven fire shape.
pub fn booter_fire(gpu: &TxGpu, hdr: &BooterHdr, image: &[u8], meta_addr: u64) -> Result<(), TxError> {
    const SEC2: u32 = eng::SEC2;
    let sig_dmem = hdr.patch_loc - hdr.data_off;

    crate::tx::falcon_reset(gpu, SEC2, eng::SEC2_FALCON2)?;
    crate::tx::falcon_dma_unblock(gpu, SEC2, eng::SEC2_FBIF)?;

    let sec = &image[hdr.app_code_off as usize..(hdr.app_code_off + hdr.app_code_size) as usize];
    crate::tx_portal::portal_load_imem(gpu, SEC2, 0, hdr.app_code_off, sec, true);
    let dmem = &image[hdr.data_off as usize..(hdr.data_off + hdr.data_size) as usize];
    crate::tx_portal::portal_load_dmem(gpu, SEC2, 0, dmem);

    gpu.write32(eng::SEC2_FALCON2 + 0x210, sig_dmem);
    gpu.write32(eng::SEC2_FALCON2 + 0x19c, hdr.engine_id);
    gpu.write32(eng::SEC2_FALCON2 + 0x198, hdr.ucode_id & 0xff);
    gpu.write32(eng::SEC2_FALCON2 + 0x180, 1);
    let para = gpu.read32(eng::SEC2_FALCON2 + 0x210)?;
    let engid = gpu.read32(eng::SEC2_FALCON2 + 0x19c)?;
    let ucode = gpu.read32(eng::SEC2_FALCON2 + 0x198)?;
    info!("booter: PARAADDR={para:#x} ENGIDMASK={engid:#x} UCODE_ID={ucode:#x}");
    if para != sig_dmem || engid != hdr.engine_id || ucode != hdr.ucode_id & 0xff {
        return Err(TxError::Io);
    }

    gpu.write32(SEC2 + 0x104, hdr.app_code_off);
    gpu.write32(SEC2 + 0x040, meta_addr as u32);
    gpu.write32(SEC2 + 0x044, (meta_addr >> 32) as u32);
    gpu.write32(SEC2 + 0x0d0, 0);

    let cpuctl = gpu.read32(SEC2 + 0x100)?;
    if cpuctl & (1 << 6) != 0 {
        gpu.write32(SEC2 + 0x130, 1 << 1);
    } else {
        gpu.write32(SEC2 + 0x100, 1 << 1);
    }
    let imm = gpu.pci.bar_read_u32(gpu.bar, (SEC2 + 0x100) as u64);
    stall(Duration::from_millis(5));
    if imm == cpuctl && cpuctl & (1 << 6) == 0 {
        info!("booter: START appeared dropped, retrying through the alias register");
        gpu.write32(SEC2 + 0x130, 1 << 1);
    }

    info!("booter: started, waiting for HALT");
    gpu.poll_us(SEC2 + 0x100, 1 << 4, 1 << 4, 5_000_000)?;
    let cpuctl = gpu.read32(SEC2 + 0x100)?;
    let mbox0 = gpu.read32_raw(SEC2 + 0x040);
    let exci = gpu.read32_raw(SEC2 + 0x0d0);
    info!("booter: HALT cpuctl={cpuctl:#010x} mailbox0={mbox0:#010x} exci={exci:#010x}");
    Ok(())
}

/// Verify the payload's writes before the selectors are touched.
pub fn verify_execution(gpu: &TxGpu, bar1_64g: bool, _meta: &[u8]) -> Result<(), TxError> {
    let plm = gpu.read32_raw(regs::FEAT_PLM);
    let ss0 = gpu.read32_raw(regs::FEAT_SS0);
    let ss1 = gpu.read32_raw(regs::FEAT_SS1);
    let mbox0 = gpu.read32_raw(regs::SEC2_MAILBOX0);
    info!("post-boot: FEAT_PLM={plm:#010x} SS0={ss0:#x} SS1={ss1:#x} mailbox0={mbox0:#010x}");

    if bar1_64g {
        let cya = gpu.read32_raw(regs::XVE_CYA);
        let cfg = gpu.read32_raw(regs::XVE_CFG);
        let xve = gpu.read32_raw(regs::XVE_PLM);
        info!("post-boot bar1: CYA={cya:#x} CFG={cfg:#010x} XVE_PLM={xve:#010x}");
        if (cya & 0x3f) != 0x30
            || (cfg & 0x8000_000f) != 0x8000_000a
            || xve != !0
            || plm != !0
            || mbox0 != BAR1_MARKER
        {
            return Err(TxError::Io);
        }
        return Ok(());
    }

    if plm != !0 {
        info!("post-boot: FEAT_PLM did not open");
        return Err(TxError::Io);
    }
    let wprcfg = gpu.read32_raw(regs::WPRCFG_PLM);
    let fbpa = gpu.read32_raw(regs::FBPA_PLM);
    let wpr = gpu.read32_raw(regs::WPR_PLM);
    info!("post-boot: WPRCFG={wprcfg:#x} FBPA={fbpa:#x} WPR={wpr:#x}");
    if wprcfg != 0xffff_f0ff || wpr != !0 {
        return Err(TxError::Io);
    }
    if fbpa != 0xffff_ff0f && fbpa != !0 {
        return Err(TxError::Io);
    }
    if mbox0 != TENSOR_MARKER {
        info!("post-boot: unexpected mailbox marker {mbox0:#x}");
        return Err(TxError::Io);
    }
    Ok(())
}

fn pre_flr_selectors(gpu: &TxGpu) -> Result<(u32, u32), TxError> {
    let ss0 = gpu.read32(regs::FEAT_SS0)?;
    let ss1 = gpu.read32(regs::FEAT_SS1)?;
    info!("pre-FLR: SS0={ss0:#010x} SS1={ss1:#x}");
    if ss0 != SS0_UNLOCK || ss1 != SS1_UNLOCK {
        return Err(TxError::Io);
    }
    Ok((ss0, ss1))
}

/// Pre-FLR checks, one FLR, post-FLR selector/WPR2 checks, the bar1
/// construct check, and the optional 64 GiB ReBAR selection.
pub fn flr_phase(
    gpu: &TxGpu,
    bar1_64g: bool,
    rebar64: bool,
    bar1_ctrl: Option<u32>,
) -> Result<(), TxError> {
    pre_flr_selectors(gpu)?;
    crate::tx::flr_reset(gpu.pci)?;

    // The chip reboots itself; GFW cycles and the cores settle into HALT.
    let _ = gpu.poll_us(regs::GFW_BOOT, 0xff, 0xff, 5_000_000);
    let _ = gpu.poll_us(regs::GFW_CPUCTL, 0x10, 0x10, 10_000_000);

    let lo = gpu.read32_raw(regs::WPR2_LO);
    let hi = gpu.read32_raw(regs::WPR2_HI);
    info!("post-FLR: WPR2={lo:#x}/{hi:#x}");
    if hi >> 4 != 0 {
        return Err(TxError::Io);
    }

    let ss0 = gpu.read32(regs::FEAT_SS0)?;
    let ss1 = gpu.read32(regs::FEAT_SS1)?;
    info!("post-FLR: SS0={ss0:#010x} SS1={ss1:#x}");
    if ss0 != SS0_UNLOCK || ss1 != SS1_UNLOCK {
        return Err(TxError::Io);
    }

    if bar1_64g {
        let cya = gpu.read32_raw(regs::XVE_CYA);
        let cfg = gpu.read32_raw(regs::XVE_CFG);
        let xve = gpu.read32_raw(regs::XVE_PLM);
        let plm = gpu.read32_raw(regs::FEAT_PLM);
        info!("post-FLR bar1: CYA={cya:#x} CFG={cfg:#010x} XVE_PLM={xve:#010x} FEAT_PLM={plm:#010x}");
        // The 64 GiB construct is CYA/CFG. The XVE PLM is a write gate that
        // may or may not survive the FLR; the selectors above are the
        // compute-unlock proof, and the ReBAR capability carries the 64 GiB
        // support. Do not fail on a closed XVE gate here.
        if (cya & 0x3f) != 0x30 || (cfg & 0x8000_000f) != 0x8000_000a {
            info!("post-FLR bar1: the 64 GiB construct did not survive the FLR");
            return Err(TxError::Io);
        }
        bar1::report(gpu.pci, "post-FLR");
        if rebar64 {
            // A ReBAR write failure must not trigger the error path's
            // emergency FLR: the card is already unlocked and a second FLR
            // would wipe that. Linux can still resize later.
            if let Err(e) = bar1::select_64g(gpu.pci) {
                info!("post-FLR bar1: 64G ReBAR selection failed ({e:?}); the unlock is intact, Linux can resize later");
            }
        } else if let Some(saved) = bar1_ctrl {
            // The payload itself leaves the current size control at 64 GiB.
            // Without pci=realloc/Linux resource assignment that makes the
            // NVIDIA driver fail with "BAR1 is 0M". Put the pre-payload size
            // back; the 64 GiB capability stays in the CAP.
            if let Err(e) = bar1::restore(gpu.pci, saved) {
                info!("post-FLR bar1: restoring the pre-payload BAR1 size failed ({e:?}); current size control may still be 64 GiB");
            }
        } else {
            info!("post-FLR bar1: no saved BAR1 control; leaving ReBAR alone");
        }
    }
    Ok(())
}
