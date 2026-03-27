#![no_std]
#![no_main]

//! 帧缓冲演示：640×400 RGBA 软件绘制后通过 `fb_present` 送到 VirtIO-GPU。
//! 在 shell 中执行 `fb_demo`；无 GPU 时打印提示并正常退出。

extern crate user_lib;

use user_lib::{fb_get_info, fb_present, println, FbInfo};

const W: usize = 640;
const H: usize = 400;
const BYTES: usize = W * H * 4;

static mut FRAME: [u8; BYTES] = [0u8; BYTES];

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    let mut info = FbInfo::default();
    if fb_get_info(&mut info as *mut _) < 0 {
        println!("fb_demo: fb_get_info failed (no virtio-gpu?)");
        return 0;
    }
    println!(
        "fb_demo: display {}x{} stride={}",
        info.width, info.height, info.stride
    );

    let buf = unsafe { core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(FRAME).cast(), BYTES) };
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            buf[i] = ((x * 4) & 0xff) as u8;
            buf[i + 1] = ((y * 4) & 0xff) as u8;
            buf[i + 2] = 0x60;
            buf[i + 3] = 0xff;
        }
    }

    if fb_present(buf.as_ptr(), buf.len()) < 0 {
        println!("fb_demo: fb_present failed");
        return 0;
    }
    println!("fb_demo: frame presented");
    0
}
