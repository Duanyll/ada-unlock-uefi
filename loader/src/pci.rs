// SPDX-License-Identifier: GPL-2.0-only
//! Minimal EFI_PCI_IO_PROTOCOL wrapper: config space, BAR MMIO, DMA and
//! bridge bus-master control. Layout follows UEFI 2.x; the protocol struct
//! is hand-written because uefi-rs 0.41 does not expose PciIo.

use alloc::vec::Vec;
use core::ffi::c_void;
use log::info;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::unsafe_protocol;
use uefi::{Identify, Status};

pub const NVIDIA_VENDOR_ID: u16 = 0x10de;

#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum PciWidth {
    Uint8 = 0,
    Uint16,
    Uint32,
}

type AccessMemFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    PciWidth,
    bar_index: u8,
    offset: u64,
    count: usize,
    buffer: *mut c_void,
) -> Status;
type AccessCfgFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    PciWidth,
    offset: u32,
    count: usize,
    buffer: *mut c_void,
) -> Status;

#[repr(C)]
struct MemAccess {
    read: AccessMemFn,
    write: AccessMemFn,
}

#[repr(C)]
struct CfgAccess {
    read: AccessCfgFn,
    write: AccessCfgFn,
}

pub type MapFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    operation: u32,
    host: *mut c_void,
    bytes: *mut usize,
    device: *mut u64,
    mapping: *mut *mut c_void,
) -> Status;
pub type UnmapFn = unsafe extern "efiapi" fn(*mut PciIoRaw, mapping: *mut c_void) -> Status;
pub type AllocBufFn = unsafe extern "efiapi" fn(
    *mut PciIoRaw,
    alloc_type: u32,
    memory_type: u32,
    pages: usize,
    host: *mut *mut c_void,
    attributes: u64,
) -> Status;
pub type FreeBufFn = unsafe extern "efiapi" fn(*mut PciIoRaw, pages: usize, host: *mut c_void) -> Status;

#[repr(C)]
pub struct PciIoRaw {
    _poll_mem: *const c_void,
    _poll_io: *const c_void,
    mem: MemAccess,
    _io: MemAccess,
    pci: CfgAccess,
    _copy_mem: *const c_void,
    map: MapFn,
    unmap: UnmapFn,
    allocate_buffer: AllocBufFn,
    free_buffer: FreeBufFn,
    _flush: *const c_void,
    get_location: unsafe extern "efiapi" fn(*mut PciIoRaw, *mut usize, *mut usize, *mut usize, *mut usize) -> Status,
    attributes: unsafe extern "efiapi" fn(*mut PciIoRaw, u32, u64, u64, *mut Status) -> Status,
    _get_bar_attributes: *const c_void,
    _set_bar_attributes: *const c_void,
    rom_size: u64,
    rom_image: *const c_void,
}

const _: () = assert!(core::mem::offset_of!(PciIoRaw, pci) == 6 * 8);
const _: () = assert!(core::mem::offset_of!(PciIoRaw, get_location) == 14 * 8);
const _: () = assert!(core::mem::offset_of!(PciIoRaw, attributes) == 15 * 8);
const _: () = assert!(core::mem::offset_of!(PciIoRaw, rom_size) == 18 * 8);
const _: () = assert!(core::mem::size_of::<PciIoRaw>() == 20 * 8);

#[repr(transparent)]
#[unsafe_protocol("4cf5b200-68b8-4ca5-9eec-b23e3f50029a")]
pub struct PciIo(PciIoRaw);

impl PciIo {
    fn config_raw(&self, width: PciWidth, offset: u32, count: usize, buf: *mut c_void) -> Status {
        unsafe { (self.0.pci.read)(&self.0 as *const _ as *mut _, width, offset, count, buf) }
    }

    fn config_write_raw(&self, width: PciWidth, offset: u32, count: usize, buf: *const c_void) {
        unsafe {
            let _ = (self.0.pci.write)(&self.0 as *const _ as *mut _, width, offset, count, buf as *mut c_void);
        }
    }

    pub fn config_read_u8(&self, offset: u32) -> u8 {
        let mut v = 0u8;
        let _ = self.config_raw(PciWidth::Uint8, offset, 1, &mut v as *mut _ as *mut c_void);
        v
    }

    pub fn config_read_u16(&self, offset: u32) -> u16 {
        let mut v = 0u16;
        let _ = self.config_raw(PciWidth::Uint16, offset, 1, &mut v as *mut _ as *mut c_void);
        v
    }

    pub fn config_read_u32(&self, offset: u32) -> u32 {
        let mut v = 0u32;
        let _ = self.config_raw(PciWidth::Uint32, offset, 1, &mut v as *mut _ as *mut c_void);
        v
    }

    pub fn config_write_u16(&self, offset: u32, v: u16) {
        self.config_write_raw(PciWidth::Uint16, offset, 1, &v as *const _ as *const c_void);
    }

    pub fn config_write_u32(&self, offset: u32, v: u32) {
        self.config_write_raw(PciWidth::Uint32, offset, 1, &v as *const _ as *const c_void);
    }

    fn mem_raw(&self, width: PciWidth, bar: u8, offset: u64, count: usize, buf: *mut c_void) -> Status {
        unsafe { (self.0.mem.read)(&self.0 as *const _ as *mut _, width, bar, offset, count, buf) }
    }

    fn mem_write_raw(&self, width: PciWidth, bar: u8, offset: u64, count: usize, buf: *mut c_void) -> Status {
        unsafe { (self.0.mem.write)(&self.0 as *const _ as *mut _, width, bar, offset, count, buf) }
    }

    pub fn bar_read_u32(&self, bar: u8, offset: u64) -> u32 {
        let mut v = 0u32;
        let _ = self.mem_raw(PciWidth::Uint32, bar, offset, 1, &mut v as *mut _ as *mut c_void);
        v
    }

    pub fn bar_read_u32_buf(&self, bar: u8, offset: u64, buf: &mut [u32]) -> Status {
        self.mem_raw(PciWidth::Uint32, bar, offset, buf.len(), buf.as_mut_ptr() as *mut c_void)
    }

    pub fn bar_write_u32(&self, bar: u8, offset: u64, v: u32) {
        let _ = self.mem_write_raw(PciWidth::Uint32, bar, offset, 1, &v as *const _ as *mut c_void);
    }

    pub fn get_location(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut seg, mut bus, mut dev, mut fun) = (0usize, 0usize, 0usize, 0usize);
        let st = unsafe {
            (self.0.get_location)(&self.0 as *const _ as *mut _, &mut seg, &mut bus, &mut dev, &mut fun)
        };
        if st != Status::SUCCESS {
            info!("PCI: get_location failed: {st:?}");
            return None;
        }
        Some((seg, bus, dev, fun))
    }

    /// Walk the PCI Express capability list. Returns the capability offset.
    pub fn pcie_cap(&self) -> Option<u8> {
        if self.config_read_u16(0x06) & 0x10 == 0 {
            return None;
        }
        let mut ptr = self.config_read_u8(0x34) & 0xfc;
        for _ in 0..48 {
            if ptr == 0 {
                break;
            }
            if self.config_read_u8(ptr.into()) == 0x10 {
                return Some(ptr);
            }
            ptr = self.config_read_u8(ptr as u32 + 1) & 0xfc;
        }
        None
    }

    pub fn pm_cap(&self) -> Option<u8> {
        if self.config_read_u16(0x06) & 0x10 == 0 {
            return None;
        }
        let mut ptr = self.config_read_u8(0x34) & 0xfc;
        for _ in 0..48 {
            if ptr == 0 {
                break;
            }
            if self.config_read_u8(ptr.into()) == 0x01 {
                return Some(ptr);
            }
            ptr = self.config_read_u8(ptr as u32 + 1) & 0xfc;
        }
        None
    }

    /// Find a PCIe extended capability by ID (config space at 0x100+).
    pub fn find_ext_cap(&self, id: u16) -> Option<u16> {
        let mut off = 0x100u16;
        for _ in 0..64 {
            let header = self.config_read_u32(off as u32);
            if header == 0 || header == 0xffff_ffff {
                return None;
            }
            if (header & 0xffff) as u16 == id {
                return Some(off);
            }
            let next = ((header >> 20) & 0xffc) as u16;
            if next == 0 || next == off {
                return None;
            }
            off = next;
        }
        None
    }

    /// Enable MMIO decode and bus master on the GPU. UEFI usually leaves MEM
    /// on but BME off for a device no driver owns; the transaction's DMA
    /// needs both.
    pub fn enable_decode_and_master(&self) -> Result<(), Status> {
        let cmd = self.config_read_u16(0x04);
        let want = cmd | 0x2 | 0x4;
        if want != cmd {
            self.config_write_u16(0x04, want);
        }
        let rb = self.config_read_u16(0x04);
        if rb & 0x6 != 0x6 {
            info!("PCI: command readback {rb:#06x} - MEMORY/BUS_MASTER did not stick");
            return Err(Status::DEVICE_ERROR);
        }
        Ok(())
    }

    pub fn dma_alloc(&self, len: usize) -> Result<DmaBuffer, Status> {
        let pages = len.div_ceil(4096);
        for (mt, name) in [(4u32, "BootServicesData"), (6u32, "RuntimeServicesData")] {
            let mut host: *mut c_void = core::ptr::null_mut();
            let st = unsafe {
                (self.0.allocate_buffer)(
                    &self.0 as *const _ as *mut _,
                    0,
                    mt,
                    pages,
                    &mut host,
                    0,
                )
            };
            if st != Status::SUCCESS || host.is_null() {
                continue;
            }
            let mut bytes = pages * 4096;
            let mut device = 0u64;
            let mut mapping: *mut c_void = core::ptr::null_mut();
            let st = unsafe {
                (self.0.map)(
                    &self.0 as *const _ as *mut _,
                    2, // EfiPciIoOperationBusMasterCommonBuffer
                    host,
                    &mut bytes,
                    &mut device,
                    &mut mapping,
                )
            };
            if st == Status::SUCCESS {
                info!("DMA: {pages} pages {name} dev={device:#x}");
                return Ok(DmaBuffer {
                    host: host as *mut u8,
                    dev: device,
                    mapping,
                    pages,
                    released: false,
                    owner: &self.0 as *const PciIoRaw,
                });
            }
            let _ = unsafe { (self.0.free_buffer)(&self.0 as *const _ as *mut _, pages, host) };
        }
        Err(Status::OUT_OF_RESOURCES)
    }
}

pub struct DmaBuffer {
    pub host: *mut u8,
    pub dev: u64,
    mapping: *mut c_void,
    pages: usize,
    released: bool,
    owner: *const PciIoRaw,
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if self.owner.is_null() {
            return;
        }
        unsafe {
            let _ = ((*self.owner).unmap)(self.owner as *mut _, self.mapping);
            let _ = ((*self.owner).free_buffer)(self.owner as *mut _, self.pages, self.host as *mut c_void);
        }
    }
}

#[derive(Clone, Copy)]
pub struct FoundGpu {
    pub handle: uefi::Handle,
    pub device_id: u16,
    pub seg: usize,
    pub bus: usize,
    pub dev: usize,
    pub fun: usize,
}

pub fn enumerate() -> Vec<FoundGpu> {
    let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) {
        Ok(h) => h,
        Err(e) => {
            info!("PCI: enumerate failed: {:?}", e.status());
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for handle in handles.iter() {
        let pci = match unsafe {
            boot::open_protocol::<PciIo>(
                OpenProtocolParams { handle: *handle, agent: boot::image_handle(), controller: None },
                OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(p) => p,
            Err(_) => continue,
        };
        let Some((seg, bus, dev, fun)) = pci.get_location() else { continue };
        let vendor = pci.config_read_u16(0x00);
        let device = pci.config_read_u16(0x02);
        if vendor != NVIDIA_VENDOR_ID || fun != 0 {
            continue;
        }
        let class = pci.config_read_u32(0x08) >> 16;
        if class >> 8 != 0x03 {
            continue;
        }
        info!("PCI: GPU 10de:{device:04x} at {seg:04x}:{bus:02x}:{dev:02x}.{fun}");
        out.push(FoundGpu { handle: *handle, device_id: device, seg, bus, dev, fun });
    }
    out
}

pub fn open_gpu(handle: uefi::Handle) -> Option<boot::ScopedProtocol<PciIo>> {
    unsafe {
        boot::open_protocol::<PciIo>(
            OpenProtocolParams { handle, agent: boot::image_handle(), controller: None },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()
}

/// Enable bus master on every PCI-to-PCI bridge between the root complex and
/// the GPU, and clear it again on drop. Some firmware leaves upstream bridge
/// BME off; the GPU's DMA then completes with Master Aborts.
pub struct BridgeMaster {
    enabled: Vec<uefi::Handle>,
}

impl BridgeMaster {
    pub fn enable(gpu: &PciIo) -> BridgeMaster {
        let mut enabled = Vec::new();
        let Some((gpu_seg, gpu_bus, _, _)) = gpu.get_location() else {
            return BridgeMaster { enabled };
        };
        let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) else {
            return BridgeMaster { enabled };
        };
        for handle in handles.iter() {
            let Ok(pci) = (unsafe {
                boot::open_protocol::<PciIo>(
                    OpenProtocolParams { handle: *handle, agent: boot::image_handle(), controller: None },
                    OpenProtocolAttributes::GetProtocol,
                )
            }) else { continue };
            if pci.config_read_u8(0x0e) & 0x7f != 0x01 {
                continue;
            }
            // Same segment, and the bridge's secondary bus range contains
            // the GPU's bus.
            let Some((seg, _, _, _)) = pci.get_location() else { continue };
            if seg != gpu_seg {
                continue;
            }
            let buses = pci.config_read_u32(0x18);
            let sec = (buses >> 8 & 0xff) as usize;
            let sub = (buses >> 16 & 0xff) as usize;
            if gpu_bus < sec || gpu_bus > sub {
                continue;
            }
            let before = pci.config_read_u16(0x04);
            if before & 0x4 != 0 {
                continue;
            }
            pci.config_write_u16(0x04, before | 0x4);
            let after = pci.config_read_u16(0x04);
            info!("BRIDGE: {seg:04x}:{sec:02x} BME {before:#06x} -> {after:#06x}");
            if after & 0x4 != 0 {
                enabled.push(*handle);
            }
        }
        BridgeMaster { enabled }
    }
}

impl Drop for BridgeMaster {
    fn drop(&mut self) {
        for handle in self.enabled.drain(..) {
            let Ok(pci) = (unsafe {
                boot::open_protocol::<PciIo>(
                    OpenProtocolParams { handle, agent: boot::image_handle(), controller: None },
                    OpenProtocolAttributes::GetProtocol,
                )
            }) else { continue };
            let now = pci.config_read_u16(0x04);
            pci.config_write_u16(0x04, now & !0x4);
        }
    }
}
