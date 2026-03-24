//! Minimal VirtIO-GPU wrapper for ch1.
//!
//! This module keeps the GPU usage simple:
//! - init device from MMIO
//! - expose framebuffer as `&mut [u32]`
//! - flush to scanout

#![cfg(target_arch = "riscv64")]

use core::{ptr::NonNull, slice};
use tg_sbi::console_putchar;
use virtio_drivers::{Hal, MmioTransport, VirtIOGpu, VirtIOHeader};

const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STEP: usize = 0x1000;
const VIRTIO_MMIO_SLOTS: usize = 8;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_ID_GPU: u32 = 16;
const PAGE_SIZE: usize = 4096;
// Keep enough contiguous DMA memory for high-resolution framebuffers.
const DMA_POOL_SIZE: usize = 16 * 1024 * 1024;

#[repr(align(4096))]
struct DmaPool([u8; DMA_POOL_SIZE]);

static mut DMA_POOL: DmaPool = DmaPool([0; DMA_POOL_SIZE]);
static mut DMA_OFFSET: usize = 0;

/// Simple error type for display initialization/rendering.
#[derive(Debug, Clone, Copy)]
pub(crate) enum GpuError {
    /// Failed to create MMIO transport.
    Transport,
    /// No GPU device found on virtio-mmio bus.
    NotFound,
    /// Failed to create GPU device.
    Device,
    /// Failed to setup framebuffer.
    Framebuffer,
    /// Failed to flush rendered result.
    Flush,
}

/// Thin GPU wrapper used by `main.rs`.
pub(crate) struct Gpu {
    inner: VirtIOGpu<'static, VirtioHal, MmioTransport>,
}

impl Gpu {
    /// Create and initialize VirtIO-GPU.
    pub(crate) fn new() -> Result<Self, GpuError> {
        log_str("gpu: probing virtio-mmio slots\n");
        let base = find_gpu_mmio_base().ok_or(GpuError::NotFound)?;
        log_str("gpu: found device base\n");
        let transport =
            unsafe { MmioTransport::new(NonNull::new(base as *mut VirtIOHeader).unwrap()) }
                .map_err(|_| GpuError::Transport)?;
        log_str("gpu: transport ready\n");
        let inner = VirtIOGpu::new(transport).map_err(|_| GpuError::Device)?;
        log_str("gpu: device initialized\n");
        Ok(Self { inner })
    }

    /// Get framebuffer, render with callback, then flush.
    pub(crate) fn render_once(
        &mut self,
        f: impl FnOnce(&mut [u32], usize, usize),
    ) -> Result<(), GpuError> {
        log_str("gpu: querying resolution\n");
        let (width, height) = self.inner.resolution().map_err(|_| GpuError::Framebuffer)?;
        log_str("gpu: resolution ok\n");
        log_num("gpu: width=", width as usize);
        log_num("gpu: height=", height as usize);
        log_str("gpu: setting up framebuffer\n");
        let framebuffer = self
            .inner
            .setup_framebuffer()
            .map_err(|_| GpuError::Framebuffer)?;
        log_str("gpu: framebuffer ready\n");
        let px_len = framebuffer.len() / 4;
        log_num("gpu: fb pixels=", px_len);
        let fb_u32 =
            unsafe { slice::from_raw_parts_mut(framebuffer.as_mut_ptr() as *mut u32, px_len) };

        log_str("gpu: entering draw callback\n");
        f(fb_u32, width as usize, height as usize);
        log_str("gpu: draw callback returned\n");
        log_str("gpu: flushing scanout\n");
        self.inner.flush().map_err(|_| GpuError::Flush)?;
        log_str("gpu: flush complete\n");
        Ok(())
    }
}

struct VirtioHal;

fn find_gpu_mmio_base() -> Option<usize> {
    for i in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + i * VIRTIO_MMIO_STEP;
        let magic = mmio_read(base + 0x000);
        if magic != VIRTIO_MAGIC {
            continue;
        }
        let device_id = mmio_read(base + 0x008);
        if device_id == VIRTIO_DEVICE_ID_GPU {
            return Some(base);
        }
    }
    None
}

#[inline]
fn mmio_read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> usize {
        let bytes = pages * PAGE_SIZE;
        unsafe {
            let start = (DMA_OFFSET + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            if start + bytes > DMA_POOL_SIZE {
                return 0;
            }
            DMA_OFFSET = start + bytes;
            core::ptr::addr_of_mut!(DMA_POOL.0).cast::<u8>().add(start) as usize
        }
    }

    fn dma_dealloc(_paddr: usize, _pages: usize) -> i32 {
        // no-op bump allocator
        0
    }

    fn phys_to_virt(paddr: usize) -> usize {
        paddr
    }

    fn virt_to_phys(vaddr: usize) -> usize {
        vaddr
    }
}

fn log_str(msg: &str) {
    for c in msg.bytes() {
        console_putchar(c);
    }
}

fn log_num(prefix: &str, value: usize) {
    log_str(prefix);
    log_usize(value);
    console_putchar(b'\n');
}

fn log_usize(mut value: usize) {
    if value == 0 {
        console_putchar(b'0');
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;
    while value > 0 {
        buf[i] = b'0' + (value % 10) as u8;
        value /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        console_putchar(buf[i]);
    }
}
