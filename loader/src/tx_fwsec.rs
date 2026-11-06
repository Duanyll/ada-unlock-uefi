// SPDX-License-Identifier: GPL-2.0-only
//! FWSEC-FRTS extraction from the card's own VBIOS PROM, the WPR layout
//! calculation, and the GSP ignition that provisions WPR2.

use alloc::vec;
use log::info;

use crate::tx::{eng, falcon_dma_unblock, falcon_reset, inaccessible, TxError, TxGpu};

pub struct FwsecInfo {
    pub image_off: u32,
    pub image_size: u32,
    pub stored_size: u32,
    pub imem_pa: u32,
    pub imem_va: u32,
    pub imem_size: u32,
    pub dmem_pa: u32,
    pub dmem_size: u32,
    pub data_off: u32,
    pub pkc_off: u32,
    pub engine_mask: u32,
    pub ucode_id: u32,
    pub sig_versions: u32,
    pub sig_off: u32,
    pub interface_off: u32,
}

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn u32v(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn find_bit_header(rom: &[u8]) -> Option<usize> {
    let mut a = 0usize;
    while a + 12 <= rom.len() {
        if le16(rom, a) == 0xB8FF && &rom[a + 2..a + 6] == b"BIT\0" {
            let hs = rom[a + 8] as usize;
            if hs >= 12 && a + hs <= rom.len() {
                let sum: u32 = rom[a..a + hs].iter().map(|&b| b as u32).sum();
                if sum & 0xff == 0 {
                    return Some(a);
                }
            }
        }
        a += 2;
    }
    None
}

fn find_expansion_offset(rom: &[u8]) -> Option<usize> {
    let mut curr = 0usize;
    let mut base_size = None;
    for _ in 0..16 {
        let pcir = le16(rom, curr + 0x18) as usize;
        let p = curr + pcir;
        match rom.get(p..p + 4) {
            Some(b"PCIR") | Some(b"NPDS") | Some(b"RGIS") => {}
            _ => return None,
        }
        let slen = le16(rom, p + 0x0a) as usize;
        let imglen = le16(rom, p + 0x10) as usize;
        let code = rom[p + 0x14];
        let mut sub = imglen;
        let npde_at = p + slen + 0xf & !0xf;
        if rom.get(npde_at..npde_at + 4) == Some(b"NPDE".as_slice()) {
            let rev = le16(rom, npde_at + 4);
            if rev == 0x10 || rev == 0x11 {
                sub = le16(rom, npde_at + 8) as usize;
            }
        }
        if code == 0xE0 {
            return curr.checked_sub(base_size?);
        }
        if code == 0x00 && base_size.is_none() {
            base_size = Some(sub * 512);
        }
        if sub == 0 || curr + sub * 512 + 0x20 > rom.len() {
            return None;
        }
        curr += sub * 512;
    }
    None
}

/// Walk the BIT token table to the production FWSEC (app 0x85, V3).
pub fn prom_find_fwsec(rom: &[u8]) -> Option<FwsecInfo> {
    let bit = find_bit_header(rom)?;
    let expansion = find_expansion_offset(rom)?;
    let hdr_size = *rom.get(bit + 8)? as usize;
    let stride = *rom.get(bit + 9)? as usize;
    let tokens = *rom.get(bit + 10)? as usize;
    if !(6..=8).contains(&stride) {
        return None;
    }
    for t in 0..tokens {
        let tok = bit + hdr_size + t * stride;
        if rom.get(tok..tok + 2) != Some(&[0x70, 0x02][..]) {
            continue;
        }
        let data_ptr = if stride >= 8 {
            le32(rom, tok + 4) as usize
        } else {
            le16(rom, tok + 4) as usize
        };
        let table = expansion.checked_add(u32v(rom, data_ptr)? as usize)?;
        if rom.get(table) != Some(&1) {
            continue;
        }
        let esz = *rom.get(table + 2)? as usize;
        let count = *rom.get(table + 3)? as usize;
        for e in 0..count {
            let at = table + 6 + e * esz;
            if *rom.get(at)? != 0x85 {
                continue;
            }
            let desc_off = expansion.checked_add(u32v(rom, at + 2)? as usize)?;
            let vdesc = u32v(rom, desc_off)?;
            let ver = (vdesc >> 8) & 0xff;
            let size = vdesc >> 16;
            if vdesc & 4 != 0 {
                info!("FWSEC: descriptor is encrypted");
                return None;
            }
            if ver != 3 || size < 44 || desc_off + 44 > rom.len() {
                return None;
            }
            let stored = le32(rom, desc_off + 4);
            return Some(FwsecInfo {
                image_off: desc_off as u32 + size,
                image_size: (stored + 255) & !255,
                stored_size: stored,
                imem_pa: le32(rom, desc_off + 16),
                imem_va: le32(rom, desc_off + 24),
                imem_size: le32(rom, desc_off + 20),
                dmem_pa: le32(rom, desc_off + 28),
                dmem_size: le32(rom, desc_off + 32),
                data_off: le32(rom, desc_off + 20),
                pkc_off: le32(rom, desc_off + 8),
                engine_mask: le16(rom, desc_off + 36) as u32,
                ucode_id: rom[desc_off + 38] as u32,
                sig_versions: le16(rom, desc_off + 40) as u32,
                sig_off: desc_off as u32 + 44,
                interface_off: le32(rom, desc_off + 12),
            });
        }
    }
    None
}

pub fn fuse_version(raw: u32) -> u32 {
    let mut value = raw & 0xffff;
    if value == 0 {
        return 0;
    }
    let mut index = 0;
    while value > 1 {
        value >>= 1;
        index += 1;
    }
    index + 1
}

/// Signature slot offset = size * popcount of version bits below the fuse.
pub fn fwsec_sig_offset(versions: u32, fuse_ver: u32, sig_size: u32) -> Result<u32, TxError> {
    if fuse_ver >= 16 || versions > 0xffff || versions & (1 << fuse_ver) == 0 {
        return Err(TxError::Range);
    }
    Ok((versions & ((1 << fuse_ver) - 1)).count_ones() * sig_size)
}

/// Point the FWSEC mapper at our FRTS offset.
pub fn fwsec_patch_frts(image: &mut [u8], fw: &FwsecInfo, frts_offset: u64) -> Result<(), TxError> {
    if frts_offset == 0 || frts_offset & 0xfff != 0 || (frts_offset >> 12) > u32::MAX as u64 {
        return Err(TxError::Invalid);
    }
    let d0 = fw.data_off as usize;
    let io = fw.interface_off as usize;
    if d0 + fw.dmem_size as usize > image.len() || io + 4 > fw.dmem_size as usize {
        return Err(TxError::Invalid);
    }
    let dmem = &mut image[d0..d0 + fw.dmem_size as usize];
    if dmem[io] != 1 || dmem[io + 1] != 4 || dmem[io + 2] != 8 {
        return Err(TxError::Io);
    }
    let entries = dmem[io + 3] as usize;
    let mut off = io + 4;
    if entries < 2 || off + entries * 8 > dmem.len() {
        return Err(TxError::Invalid);
    }
    let mut mapper = None;
    for _ in 0..entries {
        let dest = le32(dmem, off + 4) as usize;
        if le32(dmem, off) == 4 {
            if mapper.is_some() || dest + 64 > dmem.len() {
                return Err(TxError::Invalid);
            }
            if le16(dmem, dest + 4) != 3 || le16(dmem, dest + 6) < 64 {
                return Err(TxError::Io);
            }
            mapper = Some(dest);
        }
        off += 8;
    }
    let mapper = mapper.ok_or(TxError::Io)?;
    let cmd_off = le32(dmem, mapper + 8) as usize;
    let cmd_size = le32(dmem, mapper + 12) as usize;
    if cmd_size < 44 || cmd_off + cmd_size > dmem.len() {
        return Err(TxError::Invalid);
    }
    let mut cmd = [0u8; 44];
    cmd[0..4].copy_from_slice(&1u32.to_le_bytes());
    cmd[4..8].copy_from_slice(&24u32.to_le_bytes());
    cmd[20..24].copy_from_slice(&2u32.to_le_bytes());
    cmd[24..28].copy_from_slice(&1u32.to_le_bytes());
    cmd[28..32].copy_from_slice(&20u32.to_le_bytes());
    cmd[32..36].copy_from_slice(&((frts_offset >> 12) as u32).to_le_bytes());
    cmd[36..40].copy_from_slice(&0x100u32.to_le_bytes());
    cmd[40..44].copy_from_slice(&2u32.to_le_bytes());
    dmem[cmd_off..cmd_off + 44].copy_from_slice(&cmd);
    dmem[mapper + 44..mapper + 48].copy_from_slice(&0x15u32.to_le_bytes());
    info!("FWSEC: mapper patched, FRTS={frts_offset:#x}");
    Ok(())
}

pub struct LayoutOut {
    pub fb_size: u64,
    pub vga_base: u64,
    pub wpr_end: u64,
    pub frts_offset: u64,
    pub boot_offset: u64,
    pub gsp_offset: u64,
    pub heap_offset: u64,
    pub wpr_start: u64,
    pub nonwpr_start: u64,
}

pub fn layout_from_registers(gpu: &TxGpu, image_size: u64, boot_size: u64) -> Result<LayoutOut, TxError> {
    let fb_raw = gpu.read32_raw(0x0011_83a4);
    let vga_raw = gpu.read32_raw(0x0062_5f04);
    let plm_raw = gpu.read32_raw(0x001f_a7c8);
    let lo = gpu.read32(0x001f_a82c)?;
    let hi = gpu.read32(0x001f_a830)?;
    if inaccessible(fb_raw) || inaccessible(plm_raw) || plm_raw & 1 == 0 {
        info!("layout: FB/MMU registers unreadable ({fb_raw:#x}/{plm_raw:#x})");
        return Err(TxError::NoDevice);
    }
    let usable_fb = (fb_raw as u64) << 20;
    info!(
        "layout: fb={usable_fb:#x} vga={vga_raw:#x} mmu_plm={plm_raw:#x} mmu_lo={lo:#x} mmu_hi={hi:#x} image={image_size:#x} boot={boot_size:#x}"
    );
    if usable_fb < 256 << 20 {
        return Err(TxError::Range);
    }

    // The VGA workspace pointer is tolerated as poison: bit 3 alone decides,
    // and a non-console dGPU reads badf here.
    let vga = if inaccessible(vga_raw) { 0 } else { vga_raw };
    let mut vga_base = usable_fb - (1 << 20);
    if vga & (1 << 3) != 0 {
        vga_base = ((vga >> 8) as u64) << 16;
        if vga_base < usable_fb - (1 << 20) {
            vga_base = usable_fb - 0x20000;
        }
    }
    if vga_base >= usable_fb {
        return Err(TxError::Range);
    }

    let mmu_lock_low = ((lo >> 4) as u64) << 12;
    let mmu_hi = ((hi >> 4) as u64) << 12;
    let mmu_top = (mmu_hi.saturating_add(0xffff)) & !0xffffu64;
    let mmu_lock_valid = mmu_hi >= mmu_lock_low && (mmu_top >> 20) == (usable_fb >> 20);
    let reserved = if mmu_lock_valid && mmu_lock_low < vga_base { mmu_lock_low } else { vga_base };

    let wpr_end = reserved & !(128 * 1024 - 1);
    let frts = wpr_end - (1 << 20);
    let boot = (frts - boot_size) & !4095u64;
    let gsp = (boot - image_size) & !(64 * 1024 - 1);
    let heap = (gsp - (88 << 20)) & !(1024 * 1024 - 1);
    let wpr_start = heap - (1 << 20);
    let nonwpr_start = wpr_start - (1 << 20);
    if nonwpr_start >= usable_fb
        || wpr_end > usable_fb
        || wpr_start >= wpr_end
        || gsp >= boot
        || boot >= frts
        || frts >= wpr_end
        || nonwpr_start < usable_fb - (256 << 20)
    {
        info!(
            "layout: rejected nonwpr={nonwpr_start:#x} wpr=[{wpr_start:#x},{wpr_end:#x}] gsp={gsp:#x} boot={boot:#x} frts={frts:#x}"
        );
        return Err(TxError::Range);
    }
    info!(
        "layout: frts={frts:#x} wpr=[{wpr_start:#x},{wpr_end:#x}] heap={heap:#x} gsp={gsp:#x} boot={boot:#x}"
    );
    Ok(LayoutOut {
        fb_size: usable_fb,
        vga_base,
        wpr_end,
        frts_offset: frts,
        boot_offset: boot,
        gsp_offset: gsp,
        heap_offset: heap,
        wpr_start,
        nonwpr_start,
    })
}

/// Read the whole 1 MiB PROM window, parse FWSEC, patch it for our layout,
/// install the fuse-selected signature, and ignite it on the GSP falcon.
pub fn fwsec_ignite(gpu: &TxGpu, layout: &LayoutOut) -> Result<(), TxError> {
    const PROM: u64 = 0x30_0000;
    const PROM_LEN: usize = 1024 * 1024;

    info!("FWSEC: reading 1 MiB VBIOS PROM at {PROM:#x}");
    let mut rom_words = vec![0u32; PROM_LEN / 4];
    let chunk = rom_words.len() / 16;
    for (i, part) in rom_words.chunks_mut(chunk).enumerate() {
        let st = gpu.pci.bar_read_u32_buf(gpu.bar, PROM + (i * chunk * 4) as u64, part);
        if st != uefi::Status::SUCCESS {
            info!("FWSEC: PROM read failed at chunk {i}: {st:?}");
            return Err(TxError::NoDevice);
        }
    }
    let rom: &[u8] = unsafe { core::slice::from_raw_parts(rom_words.as_ptr().cast(), PROM_LEN) };
    let Some(fw) = prom_find_fwsec(rom) else {
        info!("FWSEC: no production V3 FWSEC in the PROM");
        return Err(TxError::NoDevice);
    };
    info!(
        "FWSEC: image={:#x}+{:#x} imem={:#x}/{:#x} dmem={:#x}/{:#x} ucode={} sigs={:#x}",
        fw.image_off, fw.stored_size, fw.imem_pa, fw.imem_size, fw.dmem_pa, fw.dmem_size, fw.ucode_id, fw.sig_versions
    );
    if fw.engine_mask != 0x400 {
        info!("FWSEC: unexpected GSP engine mask {:#x}", fw.engine_mask);
        return Err(TxError::Unsupported);
    }
    if fw.ucode_id == 0 || fw.ucode_id > 16 {
        return Err(TxError::Invalid);
    }
    if fw.image_off as usize + fw.stored_size as usize > rom.len()
        || fw.imem_size > fw.image_size
        || fw.data_off as usize + fw.dmem_size as usize > fw.image_size as usize
    {
        return Err(TxError::Range);
    }

    let dma = gpu.pci.dma_alloc(fw.image_size as usize).map_err(|_| TxError::NoDevice)?;
    unsafe {
        core::ptr::write_bytes(dma.host, 0, fw.image_size as usize);
        core::ptr::copy_nonoverlapping(
            rom.as_ptr().add(fw.image_off as usize),
            dma.host,
            fw.stored_size as usize,
        );
    }
    {
        let image = unsafe { core::slice::from_raw_parts_mut(dma.host, fw.image_size as usize) };
        fwsec_patch_frts(image, &fw, layout.frts_offset)?;
    }

    let fuse_raw = gpu.read32(0x0082_41c0 + (fw.ucode_id - 1) * 4)?;
    let version = fuse_version(fuse_raw);
    let sig_at = fwsec_sig_offset(fw.sig_versions, version, 384)?;
    info!("FWSEC: fuse={fuse_raw:#x} version={version} signature_slot={sig_at:#x}");
    unsafe {
        core::ptr::copy_nonoverlapping(
            rom.as_ptr().add(fw.sig_off as usize + sig_at as usize),
            dma.host.add(fw.data_off as usize + fw.pkc_off as usize),
            384,
        );
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    falcon_reset(gpu, eng::GSP, eng::GSP_FALCON2)?;
    falcon_dma_unblock(gpu, eng::GSP, eng::GSP_FBIF)?;

    let image = unsafe { core::slice::from_raw_parts(dma.host, fw.image_size as usize) };
    let imem = &image[..fw.imem_size as usize];
    let blocks = imem.len().div_ceil(256);
    let mut tag = fw.imem_va >> 8;
    for blk in 0..blocks {
        let off = blk * 256;
        gpu.write32(eng::GSP + 0x180, ((fw.imem_pa + off as u32) & 0x00ff_fffc) | (1 << 28) | (1 << 24));
        gpu.write32(eng::GSP + 0x188, tag & 0xffff);
        tag += 1;
        for w in 0..64 {
            let b = off + w * 4;
            let mut tmp = [0u8; 4];
            for (k, t) in tmp.iter_mut().enumerate() {
                if b + k < imem.len() {
                    *t = imem[b + k];
                }
            }
            gpu.write32(eng::GSP + 0x184, u32::from_le_bytes(tmp));
        }
    }
    let dmem = &image[fw.data_off as usize..(fw.data_off + fw.dmem_size) as usize];
    crate::tx_portal::portal_load_dmem(gpu, eng::GSP, fw.dmem_pa, dmem);
    info!("FWSEC: IMEM {blocks} blocks, DMEM {} bytes loaded", dmem.len());

    gpu.write32(eng::GSP_FALCON2 + 0x210, fw.pkc_off);
    gpu.write32(eng::GSP_FALCON2 + 0x19c, fw.engine_mask);
    gpu.write32(eng::GSP_FALCON2 + 0x198, fw.ucode_id & 0xff);
    gpu.write32(eng::GSP_FALCON2 + 0x180, 1);
    gpu.write32(eng::GSP + 0x104, fw.imem_va);
    let cpuctl = gpu.read32(eng::GSP + 0x100)?;
    if cpuctl & (1 << 6) != 0 {
        gpu.write32(eng::GSP + 0x130, 1 << 1);
    } else {
        gpu.write32(eng::GSP + 0x100, 1 << 1);
    }
    info!("FWSEC: started, waiting for HALT");
    gpu.poll_us(eng::GSP + 0x100, 1 << 4, 1 << 4, 5_000_000)?;

    let scratch = gpu.read32(0x1400 + 0x0e * 4)?;
    let wpr_lo = gpu.read32_raw(crate::tx::regs::WPR2_LO);
    let wpr_hi = gpu.read32_raw(crate::tx::regs::WPR2_HI);
    info!("FWSEC: scratch={scratch:#x} WPR2={wpr_lo:#x}/{wpr_hi:#x}");
    if scratch >> 16 != 0 {
        return Err(TxError::Io);
    }
    if (wpr_hi >> 4) == 0
        || (wpr_hi >> 4) < (wpr_lo >> 4)
        || ((wpr_hi >> 4) as u64) << 12 >= layout.wpr_end
        || (wpr_lo >> 4) as u64 != layout.frts_offset >> 12
    {
        return Err(TxError::Io);
    }
    info!("FWSEC: WPR2 verified");
    Ok(())
}
