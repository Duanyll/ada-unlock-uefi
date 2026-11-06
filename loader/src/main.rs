// SPDX-License-Identifier: GPL-2.0-only
//! ada-unlock: a headless UEFI boot entry that unlocks every applicable
//! RTX 4090 on the machine, then chainloads the normal Debian bootloader.

#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use log::{error, info};
use uefi::prelude::*;
use uefi::{cstr16, CString16};

mod bar1;
mod bootvol;
mod chainload;
mod config;
mod logger;
mod pci;
mod tx;
mod tx_booter;
mod tx_fwsec;
mod tx_portal;

use config::{Config, Payload};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const CONFIG_PATH: &uefi::CStr16 = cstr16!(r"\ada-unlock.ini");

fn read_asset(name: &str) -> Result<Vec<u8>, String> {
    let path = CString16::try_from(name).map_err(|_| format!("bad asset path {name}"))?;
    bootvol::read_file(&path).map_err(|e| format!("read {name}: {e:?}"))
}

fn load_assets(payload: Payload) -> Result<tx::TxAssets, String> {
    let booter = read_asset(r"\booter_load_ad102.bin")?;
    let boot_desc = read_asset(r"\gsp_boot_desc_ad102.bin")?;
    let gsp_boot = read_asset(r"\gsp_boot_ad102.bin")?;
    let gsp_image = read_asset(r"\fwimage_ad102.bin")?;
    let stock_sig = read_asset(r"\fwsignature_ad102.bin")?;
    let v67 = match payload {
        Payload::Tensor => read_asset(r"\v67-ad102-four-plm.bin")?,
        Payload::TensorBar1 => read_asset(r"\v67-ad102-bar1-64g.bin")?,
    };
    info!(
        "assets: booter={} desc={} boot={} image={} signature={} payload={}",
        booter.len(), boot_desc.len(), gsp_boot.len(), gsp_image.len(), stock_sig.len(), v67.len()
    );
    let assets = tx::TxAssets {
        booter,
        boot_desc,
        gsp_boot,
        gsp_image,
        stock_sig,
        v67,
        bar1_64g: payload == Payload::TensorBar1,
    };
    tx::validate_asset_sizes(&assets).map_err(|e| format!("asset validation: {e:?}"))?;
    Ok(assets)
}

fn path_text(c: &uefi::CStr16) -> String {
    let mut s = String::new();
    for unit in c.as_slice() {
        let v: u16 = (*unit).into();
        if v == 0 {
            break;
        }
        s.push(char::from_u32(v as u32).unwrap_or('?'));
    }
    s
}

fn gpu_name(device: u16, payload: Payload) -> Option<&'static str> {
    match device {
        0x2684 => Some("RTX 4090"),
        0x2685 if payload == Payload::Tensor => Some("RTX 4090 D"),
        0x26b5 if payload == Payload::Tensor => Some("L40"),
        _ => None,
    }
}

#[entry]
fn main() -> Status {
    let _ = uefi::boot::set_watchdog_timer(0, 0, None);
    logger::early(&format!("ada-unlock {VERSION}"));

    let cfg_bytes = match bootvol::read_file(CONFIG_PATH) {
        Ok(v) => v,
        Err(e) => {
            logger::early(&format!("ada-unlock: cannot read \\ada-unlock.ini: {e:?}"));
            return Status::INVALID_PARAMETER;
        }
    };
    let cfg: Config = match config::parse(&cfg_bytes) {
        Ok(c) => c,
        Err(e) => {
            logger::early(&format!("ada-unlock: config error: {e}"));
            return Status::INVALID_PARAMETER;
        }
    };

    logger::init(&cfg.log);
    info!("ada-unlock {VERSION}");
    info!(
        "config: payload={:?} bar1_rebar64={} targets={} chainload={}",
        cfg.payload,
        cfg.bar1_rebar64,
        if cfg.targets.is_none() { "all".into() } else { "list" },
        path_text(&cfg.chainload)
    );

    let gpus = pci::enumerate();
    let mut targets = Vec::new();
    for gpu in gpus {
        if !cfg.matches(gpu.seg, gpu.bus, gpu.dev, gpu.fun) {
            continue;
        }
        match gpu_name(gpu.device_id, cfg.payload) {
            Some(name) => {
                info!(
                    "target: {name} 10de:{:04x} at {:04x}:{:02x}:{:02x}.{}",
                    gpu.device_id, gpu.seg, gpu.bus, gpu.dev, gpu.fun
                );
                targets.push(gpu);
            }
            None => info!("skip: 10de:{:04x} is not applicable to payload {:?}", gpu.device_id, cfg.payload),
        }
    }

    if targets.is_empty() {
        info!("no applicable GPU; nothing to unlock");
    } else {
        let assets = match load_assets(cfg.payload) {
            Ok(a) => a,
            Err(e) => {
                error!("asset setup failed: {e}");
                logger::finish();
                return Status::INVALID_PARAMETER;
            }
        };

        let mut ok = 0usize;
        let mut already = 0usize;
        let mut failed = 0usize;
        for (i, gpu) in targets.iter().enumerate() {
            info!("===== card {}/{}: {:04x}:{:02x}:{:02x}.{} =====", i + 1, targets.len(), gpu.seg, gpu.bus, gpu.dev, gpu.fun);
            let Some(pci) = pci::open_gpu(gpu.handle) else {
                error!("cannot open PciIo");
                failed += 1;
                continue;
            };
            let tx_gpu = tx::TxGpu { pci: &pci, bar: 0 };
            match tx::run_transaction(&tx_gpu, &assets, cfg.bar1_rebar64) {
                Ok(()) => {
                    info!("card {}: unlocked", i + 1);
                    ok += 1;
                }
                Err(tx::TxError::Already) => {
                    info!("card {}: already unlocked, skipped", i + 1);
                    if cfg.payload == Payload::TensorBar1 && cfg.bar1_rebar64 {
                        if let Err(e) = bar1::apply_to_existing(&tx_gpu) {
                            error!("card {}: could not apply 64G ReBAR to the already-unlocked card: {e:?}", i + 1);
                        }
                    }
                    already += 1;
                }
                Err(e) => {
                    error!("card {}: transaction failed: {e:?}", i + 1);
                    tx::emergency_flr(&pci);
                    failed += 1;
                }
            }
        }
        info!("summary: {ok} unlocked, {already} already, {failed} failed, {} present", targets.len());
    }

    info!("chainload: {}", path_text(&cfg.chainload));
    let result = chainload::start(&cfg.chainload);
    if let Err(e) = result {
        error!("chainload failed: {e:?}");
        logger::finish();
        return Status::DEVICE_ERROR;
    }
    logger::finish();
    Status::SUCCESS
}
