// SPDX-License-Identifier: GPL-2.0-only
//! Start the configured Debian bootloader from the same ESP.

use alloc::vec::Vec;

use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::DevicePath;
use uefi::{CStr16, Status};

const NODE_TYPE_MEDIA: u8 = 0x04;
const NODE_SUBTYPE_FILE: u8 = 0x04;
const END_ENTIRE: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];

fn splice_file_node(base: &DevicePath, path: &CStr16) -> Option<Vec<u8>> {
    let data = base.as_bytes();
    if data.len() < 4 || data[data.len() - 4..] != END_ENTIRE {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() + 8 + 128);
    out.extend_from_slice(&data[..data.len() - 4]);

    let mut name: Vec<u16> = path.as_slice().iter().map(|c| u16::from(*c)).collect();
    name.push(0);
    let node_len = (4 + 2 * name.len()) as u16;
    out.push(NODE_TYPE_MEDIA);
    out.push(NODE_SUBTYPE_FILE);
    out.extend_from_slice(&node_len.to_le_bytes());
    for unit in name {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&END_ENTIRE);
    Some(out)
}

/// LoadImage + StartImage the configured EFI application from our volume.
/// On success this does not return (control passes to the bootloader).
pub fn start(path: &CStr16) -> Result<(), Status> {
    let device = crate::bootvol::image_device_handle().ok_or(Status::NOT_FOUND)?;
    let dp = unsafe {
        boot::open_protocol::<DevicePath>(
            OpenProtocolParams {
                handle: device,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .map_err(|e| e.status())?;

    let full = splice_file_node(&dp, path).ok_or(Status::UNSUPPORTED)?;
    let full_dp = unsafe { DevicePath::from_ffi_ptr(full.as_ptr().cast()) };

    let loaded = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath {
            device_path: full_dp,
            boot_policy: BootPolicy::ExactMatch,
        },
    )
    .map_err(|e| e.status())?;
    log::info!("chainload: loaded, starting");
    crate::logger::finish();
    boot::start_image(loaded).map_err(|e| e.status())
}
