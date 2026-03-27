//! VirtIO-GPU：内核侧帧缓冲，供 `fb_get_info` / `fb_present` 系统调用使用。
//!
//! 将 doomgeneric 固定分辨率（640×400）的 RGBA 缓冲区按 1:1 居中贴到 QEMU 扫描缓冲并 `flush`。
//!
//! 仅在 `target_arch = "riscv64"` 下由 `main.rs` 声明本模块。

use crate::{build_flags, Sv39, KERNEL_SPACE};
use alloc::alloc::{alloc_zeroed, dealloc};
use core::{alloc::Layout, ptr::NonNull, slice};
use spin::Mutex;
use tg_console::log;
use tg_kernel_vm::page_table::{MmuMeta, VAddr, VmFlags};
use virtio_drivers::{Hal, MmioTransport, VirtIOGpu, VirtIOHeader};

const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STEP: usize = 0x1000;
const VIRTIO_MMIO_SLOTS: usize = 8;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_ID_GPU: u32 = 16;

/// 与 `doomgeneric.h` 默认一致（软件渲染分辨率）。
pub const DOOM_FB_W: usize = 640;
pub const DOOM_FB_H: usize = 400;
pub const DOOM_FB_BYTES: usize = DOOM_FB_W * DOOM_FB_H * 4;

struct GpuState {
    gpu: VirtIOGpu<'static, VirtioHal, MmioTransport>,
    fb_ptr: *mut u8,
    fb_len: usize,
    width: u32,
    height: u32,
    stride: u32,
    present_count: usize,
}

unsafe impl Send for GpuState {}

static GPU: Mutex<Option<GpuState>> = Mutex::new(None);

struct VirtioHal;

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> usize {
        unsafe {
            alloc_zeroed(Layout::from_size_align_unchecked(
                pages << Sv39::PAGE_BITS,
                1 << Sv39::PAGE_BITS,
            )) as _
        }
    }

    fn dma_dealloc(paddr: usize, pages: usize) -> i32 {
        unsafe {
            dealloc(
                paddr as _,
                Layout::from_size_align_unchecked(pages << Sv39::PAGE_BITS, 1 << Sv39::PAGE_BITS),
            )
        }
        0
    }

    fn phys_to_virt(paddr: usize) -> usize {
        paddr
    }

    fn virt_to_phys(vaddr: usize) -> usize {
        const VALID: VmFlags<Sv39> = build_flags("__V");
        let ptr: NonNull<u8> = unsafe {
            KERNEL_SPACE
                .assume_init_ref()
                .translate(VAddr::new(vaddr), VALID)
                .unwrap()
        };
        ptr.as_ptr() as usize
    }
}

fn find_gpu_mmio_base() -> Option<usize> {
    for i in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + i * VIRTIO_MMIO_STEP;
        let magic = unsafe { core::ptr::read_volatile(base as *const u32) };
        if magic != VIRTIO_MAGIC {
            continue;
        }
        let device_id = unsafe { core::ptr::read_volatile((base + 0x008) as *const u32) };
        if device_id == VIRTIO_DEVICE_ID_GPU {
            return Some(base);
        }
    }
    None
}

/// 探测并初始化 VirtIO-GPU，建立扫描帧缓冲。
pub fn init() -> Result<(), ()> {
    let base = find_gpu_mmio_base().ok_or(())?;
    let transport = unsafe {
        MmioTransport::new(NonNull::new(base as *mut VirtIOHeader).unwrap())
    }
    .map_err(|_| ())?;
    let mut gpu = VirtIOGpu::new(transport).map_err(|_| ())?;
    let (w, h) = gpu.resolution().map_err(|_| ())?;
    let fb = gpu.setup_framebuffer().map_err(|_| ())?;
    let fb_len = fb.len();
    let fb_ptr = fb.as_mut_ptr();
    let h_us = h as usize;
    let stride = if h_us > 0 && fb_len % h_us == 0 {
        fb_len / h_us
    } else {
        (w as usize).saturating_mul(4)
    };
    if stride * h_us > fb_len {
        return Err(());
    }
    let state = GpuState {
        gpu,
        fb_ptr,
        fb_len,
        width: w,
        height: h,
        stride: stride as u32,
        present_count: 0,
    };
    log::info!(
        "virtio-gpu: resolution={}x{} stride={} fb_len={}",
        w,
        h,
        stride,
        fb_len
    );
    *GPU.lock() = Some(state);
    Ok(())
}

/// 当前帧缓冲的宽度、高度、stride（字节/行）。
pub fn dimensions() -> Option<(u32, u32, u32)> {
    let g = GPU.lock();
    g.as_ref().map(|s| (s.width, s.height, s.stride))
}

/// 将用户缓冲区（至少 [`DOOM_FB_BYTES`]）按 1:1 居中贴到扫描输出并刷新显示。
pub fn present(src: &[u8]) -> Result<(), ()> {
    if src.len() < DOOM_FB_BYTES {
        return Err(());
    }
    let mut g = GPU.lock();
    let s = g.as_mut().ok_or(())?;
    let w = s.width as usize;
    let h = s.height as usize;
    let stride = s.stride as usize;
    let fb = unsafe { slice::from_raw_parts_mut(s.fb_ptr, s.fb_len) };
    fb.fill(0);
    s.present_count += 1;

    let copy_w = DOOM_FB_W.min(w);
    let copy_h = DOOM_FB_H.min(h);
    let dst_x_off = (w.saturating_sub(copy_w)) / 2;
    let dst_y_off = (h.saturating_sub(copy_h)) / 2;
    let src_x_off = (DOOM_FB_W.saturating_sub(copy_w)) / 2;
    let src_y_off = (DOOM_FB_H.saturating_sub(copy_h)) / 2;
    let row_bytes = copy_w * 4;

    if s.present_count == 1 {
        let opaque = src
            .chunks_exact(4)
            .take(1024)
            .filter(|px| px[3] != 0)
            .count();
        log::info!(
            "virtio-gpu: first present src0={:02x?} opaque_sample={}/1024 copy={}x{} dst_offset=({}, {})",
            &src[..4],
            opaque,
            copy_w,
            copy_h,
            dst_x_off,
            dst_y_off
        );
    }

    for row in 0..copy_h {
        let src_row = src_y_off + row;
        let dst_row = dst_y_off + row;
        let src_start = (src_row * DOOM_FB_W + src_x_off) * 4;
        let dst_start = dst_row * stride + dst_x_off * 4;
        if src_start + row_bytes <= src.len() && dst_start + row_bytes <= fb.len() {
            fb[dst_start..dst_start + row_bytes]
                .copy_from_slice(&src[src_start..src_start + row_bytes]);
        }
    }
    s.gpu.flush().map_err(|_| ())?;
    Ok(())
}
