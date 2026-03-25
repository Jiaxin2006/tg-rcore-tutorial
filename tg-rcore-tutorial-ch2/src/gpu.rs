//! VirtIO-GPU wrapper for ch2 progressive tangram rendering.
//!
//! Provides a persistent framebuffer that survives across syscalls,
//! so each user app can trigger one more piece to appear.

#![cfg(target_arch = "riscv64")]

use core::{ptr::NonNull, slice};
use virtio_drivers::{Hal, MmioTransport, VirtIOGpu, VirtIOHeader};

const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STEP: usize = 0x1000;
const VIRTIO_MMIO_SLOTS: usize = 8;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_DEVICE_ID_GPU: u32 = 16;
const PAGE_SIZE: usize = 4096;
const DMA_POOL_SIZE: usize = 16 * 1024 * 1024;

#[repr(align(4096))]
struct DmaPool([u8; DMA_POOL_SIZE]);

static mut DMA_POOL: DmaPool = DmaPool([0; DMA_POOL_SIZE]);
static mut DMA_OFFSET: usize = 0;

static mut GPU_INNER: Option<VirtIOGpu<'static, VirtioHal, MmioTransport>> = None;
static mut FB_PTR: *mut u32 = core::ptr::null_mut();
static mut FB_WIDTH: usize = 0;
static mut FB_HEIGHT: usize = 0;
static mut FB_PX_LEN: usize = 0;

/// Probe VirtIO-MMIO, create GPU, setup framebuffer.
/// Must be called once before any draw/flush.
pub(crate) fn init() {
    let base = find_gpu_mmio_base().expect("gpu: no device found");
    let transport =
        unsafe { MmioTransport::new(NonNull::new(base as *mut VirtIOHeader).unwrap()) }
            .expect("gpu: transport failed");
    let mut gpu = VirtIOGpu::new(transport).expect("gpu: device init failed");
    let (w, h) = gpu.resolution().expect("gpu: resolution failed");
    let fb = gpu.setup_framebuffer().expect("gpu: framebuffer failed");
    let px_len = fb.len() / 4;
    let fb_ptr = fb.as_mut_ptr() as *mut u32;

    unsafe {
        core::ptr::addr_of_mut!(FB_PTR).write(fb_ptr);
        core::ptr::addr_of_mut!(FB_WIDTH).write(w as usize);
        core::ptr::addr_of_mut!(FB_HEIGHT).write(h as usize);
        core::ptr::addr_of_mut!(FB_PX_LEN).write(px_len);
        core::ptr::addr_of_mut!(GPU_INNER).write(Some(gpu));
    }
}

/// Run a closure with mutable access to the framebuffer.
pub(crate) fn with_framebuffer<F: FnOnce(&mut [u32], usize, usize)>(f: F) {
    unsafe {
        let ptr = core::ptr::addr_of!(FB_PTR).read();
        let len = core::ptr::addr_of!(FB_PX_LEN).read();
        let w = core::ptr::addr_of!(FB_WIDTH).read();
        let h = core::ptr::addr_of!(FB_HEIGHT).read();
        assert!(!ptr.is_null(), "gpu: not initialized");
        let fb = slice::from_raw_parts_mut(ptr, len);
        f(fb, w, h);
    }
}

/// Flush the current framebuffer content to the display.
pub(crate) fn flush() {
    unsafe {
        let gpu = &mut *core::ptr::addr_of_mut!(GPU_INNER);
        gpu.as_mut()
            .expect("gpu: not initialized")
            .flush()
            .expect("gpu: flush failed");
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

struct VirtioHal;

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> usize {
        let bytes = pages * PAGE_SIZE;
        unsafe {
            let off_ptr = core::ptr::addr_of_mut!(DMA_OFFSET);
            let off = off_ptr.read();
            let start = (off + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            if start + bytes > DMA_POOL_SIZE {
                return 0;
            }
            off_ptr.write(start + bytes);
            core::ptr::addr_of_mut!(DMA_POOL.0).cast::<u8>().add(start) as usize
        }
    }

    fn dma_dealloc(_paddr: usize, _pages: usize) -> i32 {
        0
    }

    fn phys_to_virt(paddr: usize) -> usize {
        paddr
    }

    fn virt_to_phys(vaddr: usize) -> usize {
        vaddr
    }
}
