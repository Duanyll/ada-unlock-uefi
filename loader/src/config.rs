// SPDX-License-Identifier: GPL-2.0-only
//! `\ada-unlock.ini` parser. Flat `key = value`, `#`/`;` comments, section
//! headers ignored. The app knows exactly what it is doing; an unknown key
//! or a bad value is an operator error, not something to guess around.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::CString16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    Tensor,
    TensorBar1,
}

#[derive(Clone, Copy, Debug)]
pub struct Bdf {
    pub seg: u16,
    pub bus: u8,
    pub dev: u8,
    pub fun: u8,
}

pub struct Config {
    pub payload: Payload,
    pub bar1_rebar64: bool,
    pub chainload: CString16,
    pub log: CString16,
    pub targets: Option<Vec<Bdf>>,
}

impl Config {
    pub fn matches(&self, seg: usize, bus: usize, dev: usize, fun: usize) -> bool {
        match &self.targets {
            None => true,
            Some(list) => list.iter().any(|b| {
                b.seg as usize == seg && b.bus as usize == bus && b.dev as usize == dev && b.fun as usize == fun
            }),
        }
    }
}

pub fn parse(bytes: &[u8]) -> Result<Config, String> {
    let text = core::str::from_utf8(bytes).map_err(|_| "config is not UTF-8".to_string())?;
    let mut payload = None;
    let mut bar1_rebar64 = false;
    let mut chainload = None;
    let mut log = None;
    let mut targets = None;

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected key = value", lineno + 1));
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "payload" => {
                payload = Some(match value.to_ascii_lowercase().as_str() {
                    "tensor" | "tensor-only" | "four-plm" => Payload::Tensor,
                    "tensor-bar1" | "bar1" | "bar1-64g" => Payload::TensorBar1,
                    other => return Err(format!("line {}: unknown payload '{other}'", lineno + 1)),
                });
            }
            "bar1_rebar64" => {
                bar1_rebar64 = match value.to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => true,
                    "false" | "0" | "no" | "off" => false,
                    other => return Err(format!("line {}: bar1_rebar64 must be true or false, got '{other}'", lineno + 1)),
                };
            }
            "chainload" => {
                let path = if value.starts_with('\\') { value.to_string() } else { format!("\\{value}") };
                chainload = Some(CString16::try_from(path.as_str()).map_err(|_| "chainload path is not representable".to_string())?);
            }
            "log" => {
                let path = if value.starts_with('\\') { value.to_string() } else { format!("\\{value}") };
                log = Some(CString16::try_from(path.as_str()).map_err(|_| "log path is not representable".to_string())?);
            }
            "targets" => {
                if value.eq_ignore_ascii_case("all") {
                    targets = None;
                } else {
                    let mut list = Vec::new();
                    for item in value.split(',') {
                        list.push(parse_bdf(item.trim()).ok_or_else(|| format!("line {}: bad BDF '{item}'", lineno + 1))?);
                    }
                    if list.is_empty() {
                        return Err(format!("line {}: empty targets list", lineno + 1));
                    }
                    targets = Some(list);
                }
            }
            other => return Err(format!("line {}: unknown key '{other}'", lineno + 1)),
        }
    }

    let payload = payload.ok_or_else(|| "missing payload = tensor | tensor-bar1".to_string())?;
    let chainload = chainload.ok_or_else(|| "missing chainload = \\EFI\\...".to_string())?;
    if payload == Payload::Tensor && bar1_rebar64 {
        return Err("bar1_rebar64=true requires payload = tensor-bar1".to_string());
    }
    let log = log.unwrap_or_else(|| CString16::try_from("\\ada-unlock.log").unwrap());
    Ok(Config { payload, bar1_rebar64, chainload, log, targets })
}

fn parse_bdf(s: &str) -> Option<Bdf> {
    let (seg, rest) = match s.matches(':').count() {
        2 => {
            let (a, b) = s.split_once(':')?;
            (u16::from_str_radix(a, 16).ok()?, b)
        }
        1 => (0, s),
        _ => return None,
    };
    let (bus, devfun) = rest.split_once(':')?;
    let (dev, fun) = devfun.split_once('.')?;
    Some(Bdf {
        seg,
        bus: u8::from_str_radix(bus, 16).ok()?,
        dev: u8::from_str_radix(dev, 16).ok()?,
        fun: u8::from_str_radix(fun, 16).ok()?,
    })
}
