// SPDX-License-Identifier: GPL-2.0-only
//! Falcon v4 register-portal IMEM/DMEM loading. The portal is used instead
//! of the engine's DMA path because the booter's DMA queue does not drain
//! reliably in the firmware window.
//!
//! IMEMC +0x180: OFFS[7:2] BLK[23:8] AINCW[24] SECURE[28]
//! IMEMD +0x184: data
//! IMEMT +0x188: TAG[15:0] = virtual byte address >> 8, one tag per 256 B
//! DMEMC +0x1c0: OFFS[7:2] BLK[23:8] AINCW[24]
//! DMEMD +0x1c4: data

use log::info;

use crate::tx::TxGpu;

const IMEMC: u32 = 0x180;
const IMEMD: u32 = 0x184;
const IMEMT: u32 = 0x188;
const DMEMC: u32 = 0x1c0;
const DMEMD: u32 = 0x1c4;
const AINCW: u32 = 1 << 24;
const SECURE: u32 = 1 << 28;

fn flcn_addr(addr: u32) -> u32 {
    addr & 0x00ff_fffc
}

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
            let mut tmp = [0u8; 4];
            for (k, t) in tmp.iter_mut().enumerate() {
                if b + k < data.len() {
                    *t = data[b + k];
                }
            }
            gpu.write32(base + IMEMD, u32::from_le_bytes(tmp));
        }
    }
    info!("portal: IMEM {blocks} blocks, {} bytes, dest={dest:#x} va={va:#x} secure={secure}", data.len());
}

pub fn portal_load_dmem(gpu: &TxGpu, base: u32, dest: u32, data: &[u8]) {
    gpu.write32(base + DMEMC, flcn_addr(dest) | AINCW);
    for chunk in data.chunks(4) {
        let mut tmp = [0u8; 4];
        tmp[..chunk.len()].copy_from_slice(chunk);
        gpu.write32(base + DMEMD, u32::from_le_bytes(tmp));
    }
    info!("portal: DMEM {} bytes at dest={dest:#x}", data.len());
}
