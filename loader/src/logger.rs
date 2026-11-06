// SPDX-License-Identifier: GPL-2.0-only
//! Console + `\ada-unlock.log` logger. One line, one flush: the ESP is a
//! local disk, and a crash mid-transaction must leave the last phase on it.

use core::cell::UnsafeCell;
use core::fmt::Write;

use log::{Level, Log, Metadata, Record};
use uefi::proto::media::file::{File, RegularFile};
use uefi::CStr16;

struct Logger {
    file: UnsafeCell<Option<RegularFile>>,
    lines: UnsafeCell<u64>,
}

unsafe impl Sync for Logger {}
unsafe impl Send for Logger {}

static LOGGER: Logger = Logger {
    file: UnsafeCell::new(None),
    lines: UnsafeCell::new(0),
};

/// Console line for the window before the log file exists. UEFI ConOut is
/// already redirected to SOL on a BMC server; no direct SerialIo access.
pub fn early(text: &str) {
    uefi::println!("{text}");
}

pub fn init(path: &CStr16) {
    let file = crate::bootvol::create_log(path);
    if file.is_none() {
        early("ada-unlock: cannot create log file; console/SOL only");
    }
    unsafe {
        *LOGGER.file.get() = file;
    }
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
}

pub fn finish() {
    unsafe {
        if let Some(file) = (*LOGGER.file.get()).as_mut() {
            let _ = file.flush();
        }
        let _ = (*LOGGER.file.get()).take();
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let n = unsafe {
            let n = *LOGGER.lines.get() + 1;
            *LOGGER.lines.get() = n;
            n
        };
        let mut line = alloc::string::String::new();
        let _ = write!(
            line,
            "[{n:06}] {:<5} {}\r\n",
            record.level(),
            record.args()
        );

        unsafe {
            if let Some(file) = (*LOGGER.file.get()).as_mut() {
                if file.write(line.as_bytes()).is_err() {
                    let _ = (*LOGGER.file.get()).take();
                } else {
                    let _ = file.flush();
                }
            }
        }
        uefi::print!("{}", line);
    }

    fn flush(&self) {}
}
