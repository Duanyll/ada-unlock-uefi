//! Portal-based IMEM/DMEM loading — Falcon v4 register interface, field
//! encodings from dev_falcon_v4.h (authoritative: kernel_falcon_tu102.c
//! kflcnMaskImemAddr/kflcnMaskDmemAddr = addr & (OFFS|BLK masks)):
//!   IMEMC +0x180: OFFS[7:2] BLK[23:8] AINCW[24] SECURE[28]  (byte addr maps
//!                 verbatim onto bits [23:2])
//!   IMEMD +0x184: data dword
//!   IMEMT +0x188: TAG[15:0] = virtual byte addr >> 8, per 256B block
//!   DMEMC +0x1c0: OFFS[7:2] BLK[23:8] AINCW[24]
//!   DMEMD +0x1c4: data dword
//!
//! History: an earlier revision shifted addresses right by 2 (addr>>2),
//! which silently corrupted every IMEM block past the first (four blocks
//! overlapped per 256B span) and mispointed non-zero DMEM reads by 4x.
//! Zero-address operations masked the bug; caught 2026-10-26 when the
//! post-halt DMEM dump read static image bytes 12 bytes off.
#![allow(dead_code)]

use log::info;

use crate::tx::TxGpu;

const IMEMC: u32 = 0x180;
const IMEMD: u32 = 0x184;
const IMEMT: u32 = 0x188;
const DMEMC: u32 = 0x1c0;
const DMEMD: u32 = 0x1c4;

const AINCW: u32 = 1 << 24;
const SECURE: u32 = 1 << 28;

/// Falcon v4 portal address field: byte address, bits [23:2] significant.
const fn flcn_addr(addr: u32) -> u32 {
    addr & 0x00ff_fffc
}

/// Load one 256-byte-aligned IMEM region via the IMEM portal.
/// `secure` tags the transfer SECURE (HS ucode requires it); `va` is the
/// virtual address the blocks are tagged with (kernel: the ucode's
/// imemNsPa/imemSecPa, tag incremented per block).
pub fn portal_load_imem(gpu: &TxGpu, base: u32, dest: u32, va: u32, data: &[u8], secure: bool) {
    let sec = if secure { SECURE } else { 0 };
    let blocks = data.len().div_ceil(256);
    let mut tag = va >> 8;
    for blk in 0..blocks {
        let off = blk * 256;
        gpu.write32(base + IMEMC, flcn_addr(dest + off as u32) | AINCW | sec);
        gpu.write32(base + IMEMT, tag & 0xffff);
        tag += 1;
        for w in 0..64 {
            let b = off + w * 4;
            let v = if b + 4 <= data.len() {
                u32::from_le_bytes([data[b], data[b + 1], data[b + 2], data[b + 3]])
            } else {
                let mut tmp = [0u8; 4];
                for (k, t) in tmp.iter_mut().enumerate() {
                    if b + k < data.len() {
                        *t = data[b + k];
                    }
                }
                u32::from_le_bytes(tmp)
            };
            gpu.write32(base + IMEMD, v);
        }
    }
    info!(
        "PORTAL: IMEM loaded - {} blocks, {} bytes, dest={dest:#x} va={va:#x} secure={secure}",
        blocks, data.len(),
    );
}

/// Load a DMEM region via the DATA portal (port 0).
pub fn portal_load_dmem(gpu: &TxGpu, base: u32, dest: u32, data: &[u8]) {
    gpu.write32(base + DMEMC, flcn_addr(dest) | AINCW);
    for chunk in data.chunks(4) {
        let mut tmp = [0u8; 4];
        tmp[..chunk.len()].copy_from_slice(chunk);
        gpu.write32(base + DMEMD, u32::from_le_bytes(tmp));
    }
    info!("PORTAL: DMEM loaded - {} bytes at dest={dest:#x}", data.len());
}

/// Set the DMEM read pointer to `dest` (auto-increment reads via DMEMD).
fn portal_seek_dmem(gpu: &TxGpu, base: u32, dest: u32) {
    gpu.write32(base + DMEMC, flcn_addr(dest) | AINCW);
}

/// Read `n` dwords of DMEM starting at `dest` through the portal.
/// AINCW auto-increments WRITES only - reads need an explicit seek per
/// dword (observed 2026-10-26: one seek + 64 reads returned the same word).
pub fn portal_read_dmem(gpu: &TxGpu, base: u32, dest: u32, out: &mut [u32]) {
    for (i, slot) in out.iter_mut().enumerate() {
        portal_seek_dmem(gpu, base, dest + (i * 4) as u32);
        *slot = gpu.pci.bar_read_u32(gpu.bar, (base + DMEMD) as u64);
    }
}

/// Verify by read-back where readable: DMEM portal reads work; IMEM is
/// write-only through this portal.
pub fn portal_verify_dmem(gpu: &TxGpu, base: u32, dest: u32, expect: &[u8]) -> bool {
    let mut words = [0u32; 1];
    for (i, chunk) in expect.chunks(4).enumerate() {
        portal_seek_dmem(gpu, base, dest + (i * 4) as u32);
        let _ = gpu.pci.bar_read_u32_buf(gpu.bar, (base + DMEMD) as u64, &mut words);
        let got = words[0].to_le_bytes();
        if &got[..chunk.len()] != chunk {
            let v = words[0];
            info!("PORTAL verify: DMEM mismatch at word {i}: got {v:#010x}");
            return false;
        }
    }
    true
}

/// Full portal-based load of an HS ucode image ([IMEM | DMEM] layout).
pub fn portal_load_ucode(
    gpu: &TxGpu,
    base: u32,
    image: &[u8],
    imem_size: u32,
    data_off: u32,
    dmem_size: u32,
    imem_va: u32,
    pkc_off: u32,
) {
    let imem = &image[..imem_size as usize];
    let d0 = data_off as usize;
    let dmem = &image[d0..(d0 + dmem_size as usize).min(image.len())];
    // HS ucode goes in tagged secure IMEM.
    portal_load_imem(gpu, base, 0, imem_va, imem, true);
    portal_load_dmem(gpu, base, 0, dmem);
    let _ = pkc_off;
}
