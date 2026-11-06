// SPDX-License-Identifier: GPL-2.0-only
//! Access to the volume this image was loaded from (the ESP).

use alloc::vec::Vec;

use uefi::boot;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::{CStr16, Status};

pub fn read_file(path: &CStr16) -> Result<Vec<u8>, Status> {
    let mut fs = boot::get_image_file_system(boot::image_handle()).map_err(|e| e.status())?;
    let mut dir = fs.open_volume().map_err(|e| e.status())?;
    let file = dir.open(path, FileMode::Read, FileAttribute::empty()).map_err(|e| e.status())?;
    let mut file = file.into_regular_file().ok_or(Status::INVALID_PARAMETER)?;
    let size = file
        .get_boxed_info::<FileInfo>()
        .map_err(|e| e.status())?
        .file_size() as usize;
    let mut out = Vec::with_capacity(size);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(e.status()),
        }
    }
    Ok(out)
}

/// Create (or truncate) the log file and return the open handle.
pub fn create_log(path: &CStr16) -> Option<RegularFile> {
    let mut fs = boot::get_image_file_system(boot::image_handle()).ok()?;
    let mut dir = fs.open_volume().ok()?;
    if let Ok(existing) = dir.open(path, FileMode::Read, FileAttribute::empty()) {
        let _ = existing.delete();
    }
    dir.open(path, FileMode::CreateReadWrite, FileAttribute::empty())
        .ok()?
        .into_regular_file()
}

pub fn image_device_handle() -> Option<uefi::Handle> {
    use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
    use uefi::proto::loaded_image::LoadedImage;
    let li = unsafe {
        boot::open_protocol::<LoadedImage>(
            OpenProtocolParams {
                handle: boot::image_handle(),
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()?;
    li.device()
}
