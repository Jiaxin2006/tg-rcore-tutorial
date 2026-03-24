//! Tangram "OS" drawing primitives and rasterizer.
//!
//! New model:
//! 1) Build 7 standard tangram bricks automatically from `UNIT`.
//! 2) Place "O/S" by repeatedly calling those bricks with
//!    (anchor vertex position + rotation angle).

#![cfg(target_arch = "riscv64")]

use tg_sbi::console_putchar;

/// RGBA8 packed as `0xAABBGGRR` for little-endian `u32` framebuffer.
type Color = u32;

const WHITE: Color = 0xFFFF_FFFF;
const RED: Color = 0xFF30_30E8;
const YELLOW: Color = 0xFF00_D7FF;
const CYAN: Color = 0xFFE0_B423;
const MAGENTA: Color = 0xFFC9_4ED8;
const PURPLE: Color = 0xFFDE_2A40;
const GREEN: Color = 0xFF40_EB49;
const ORANGE: Color = 0xFF00_8CFF;

/// Base size: leg length of a small right-isosceles triangle.
const UNIT: i32 = 44;
const MEDIUM: i32 = 62;
const FP_SHIFT: i32 = 10;
const FP_ONE: i32 = 1 << FP_SHIFT;
const FP_INV_SQRT2: i32 = 724;

#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy)]
enum BrickShape {
    Tri([Point; 3]),
    Quad([Point; 4]),
}

#[derive(Clone, Copy)]
struct BrickTemplate {
    shape: BrickShape,
    color: Color,
}

/// Standard tangram brick library (7 pieces, standard proportions).
///
/// Area ratios (small triangle area = 1):
/// - large triangles: 4 + 4
/// - medium triangle: 2
/// - small triangles: 1 + 1
/// - square: 2
/// - parallelogram: 2
fn standard_bricks(unit: i32) -> [BrickTemplate; 7] {
    let large = 2 * unit;
    let medium = MEDIUM;
    let small = unit;
    let para_dx = unit;
    let para_dy = unit;

    [
        // 0: Large triangle A
        BrickTemplate {
            shape: BrickShape::Tri([
                Point { x: 0, y: 0 },
                Point { x: large, y: 0 },
                Point { x: 0, y: large },
            ]),
            color: RED,
        },
        // 1: Large triangle B
        BrickTemplate {
            shape: BrickShape::Tri([
                Point { x: 0, y: 0 },
                Point { x: large, y: 0 },
                Point { x: 0, y: large },
            ]),
            color: MAGENTA,
        },
        // 2: Medium triangle
        BrickTemplate {
            shape: BrickShape::Tri([
                Point { x: 0, y: 0 },
                Point { x: medium, y: 0 },
                Point { x: 0, y: medium },
            ]),
            color: CYAN,
        },
        // 3: Small triangle A
        BrickTemplate {
            shape: BrickShape::Tri([
                Point { x: 0, y: 0 },
                Point { x: small, y: 0 },
                Point { x: 0, y: small },
            ]),
            color: PURPLE,
        },
        // 4: Small triangle B
        BrickTemplate {
            shape: BrickShape::Tri([
                Point { x: 0, y: 0 },
                Point { x: small, y: 0 },
                Point { x: 0, y: small },
            ]),
            color: ORANGE,
        },
        // 5: Square
        BrickTemplate {
            shape: BrickShape::Quad([
                Point { x: 0, y: 0 },
                Point { x: unit, y: 0 },
                Point { x: unit, y: unit },
                Point { x: 0, y: unit },
            ]),
            color: GREEN,
        },
        // 6: Parallelogram
        BrickTemplate {
            shape: BrickShape::Quad([
                Point { x: 0, y: 0 },
                Point { x: unit, y: 0 },
                Point {
                    x: unit + para_dx,
                    y: para_dy,
                },
                Point {
                    x: para_dx,
                    y: para_dy,
                },
            ]),
            color: YELLOW,
        },
    ]
}

#[derive(Clone, Copy)]
struct Placement {
    brick_id: usize,
    anchor: Point, // local vertex p0 is moved to this position
    rotate_deg: i32,
}

/// "O" and "S" are built by reusing the same 7 brick templates.
/// You only need to tweak `anchor` and `rotate_deg`.
const O_LAYOUT: [Placement; 6] = [
    Placement {
        brick_id: 0,
        anchor: Point { x: 296, y: 208 },
        rotate_deg: -90,
    },
    Placement {
        brick_id: 1,
        anchor: Point { x: 208, y: 296 },
        rotate_deg: 90,
    },
    // Placement {
    //     brick_id: 2,
    //     anchor: Point { x: 70, y: 500 },
    //     rotate_deg: -90,
    // },
    Placement {
        brick_id: 4,
        anchor: Point { x: 120, y: 164 },
        rotate_deg: -90,
    },
    Placement {
        brick_id: 5,
        anchor: Point { x: 340, y: 296 },
        rotate_deg: 45,
    },
    Placement {
        brick_id: 6,
        anchor: Point { x: 353, y: 164 },
        rotate_deg: 45,
    },
    Placement {
        brick_id: 6,
        anchor: Point { x: 120, y: 164 },
        rotate_deg: 45,
    },
];

const S_LAYOUT: [Placement; 7] = [
    Placement {
        brick_id: 2,
        anchor: Point { x: 640, y: 220 },
        rotate_deg: 0,
    },
    Placement {
        brick_id: 3,
        anchor: Point { x: 770, y: 90 },
        rotate_deg: 0,
    },
    Placement {
        brick_id: 1,
        anchor: Point { x: 1020, y: 65 },
        rotate_deg: 180,
    },
    Placement {
        brick_id: 5,
        anchor: Point { x: 770, y: 345 },
        rotate_deg: 0,
    },
    Placement {
        brick_id: 0,
        anchor: Point { x: 1020, y: 470 },
        rotate_deg: 180,
    },
    Placement {
        brick_id: 4,
        anchor: Point { x: 640, y: 500 },
        rotate_deg: 0,
    },
    Placement {
        brick_id: 6,
        anchor: Point { x: 770, y: 500 },
        rotate_deg: 0,
    },
];

/// Draw complete tangram frame.
pub(crate) fn render_tangram(framebuffer: &mut [u32], width: usize, height: usize) {
    log_str("tangram: render start\n");
    log_num("tangram: width=", width);
    log_num("tangram: height=", height);
    log_num("tangram: fb_len=", framebuffer.len());
    clear(framebuffer, WHITE);
    log_str("tangram: clear done\n");
    let bricks = standard_bricks(UNIT);
    log_str("tangram: bricks ready\n");

    for p in O_LAYOUT {
        draw_placement(framebuffer, width, height, bricks[p.brick_id], p);
    }
    log_str("tangram: O done\n");
    for p in S_LAYOUT {
        draw_placement(framebuffer, width, height, bricks[p.brick_id], p);
    }
    log_str("tangram: S done\n");
}

fn draw_placement(
    framebuffer: &mut [u32],
    width: usize,
    height: usize,
    brick: BrickTemplate,
    placement: Placement,
) {
    match brick.shape {
        BrickShape::Tri(pts) => {
            let p0 = transform_local(pts[0], placement.anchor, placement.rotate_deg);
            let p1 = transform_local(pts[1], placement.anchor, placement.rotate_deg);
            let p2 = transform_local(pts[2], placement.anchor, placement.rotate_deg);
            fill_triangle(framebuffer, width, height, p0, p1, p2, brick.color);
        }
        BrickShape::Quad(pts) => {
            let p0 = transform_local(pts[0], placement.anchor, placement.rotate_deg);
            let p1 = transform_local(pts[1], placement.anchor, placement.rotate_deg);
            let p2 = transform_local(pts[2], placement.anchor, placement.rotate_deg);
            let p3 = transform_local(pts[3], placement.anchor, placement.rotate_deg);
            fill_triangle(framebuffer, width, height, p0, p1, p2, brick.color);
            fill_triangle(framebuffer, width, height, p0, p2, p3, brick.color);
        }
    }
}

#[inline]
fn transform_local(local: Point, anchor: Point, deg: i32) -> Point {
    let (c, s) = rotation_fp(deg);
    Point {
        x: anchor.x + ((local.x * c - local.y * s) >> FP_SHIFT),
        y: anchor.y + ((local.x * s + local.y * c) >> FP_SHIFT),
    }
}

fn clear(framebuffer: &mut [u32], color: Color) {
    for px in framebuffer.iter_mut() {
        *px = color;
    }
}

fn fill_triangle(
    framebuffer: &mut [u32],
    width: usize,
    height: usize,
    p0: Point,
    p1: Point,
    p2: Point,
    color: Color,
) {
    let min_x = p0.x.min(p1.x).min(p2.x).max(0);
    let max_x = p0.x.max(p1.x).max(p2.x).min(width as i32 - 1);
    let min_y = p0.y.min(p1.y).min(p2.y).max(0);
    let max_y = p0.y.max(p1.y).max(p2.y).min(height as i32 - 1);

    if min_x > max_x || min_y > max_y {
        return;
    }

    let area = edge(p0, p1, p2);
    if area == 0 {
        return;
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = Point { x, y };
            let w0 = edge(p1, p2, p);
            let w1 = edge(p2, p0, p);
            let w2 = edge(p0, p1, p);
            if (w0 >= 0 && w1 >= 0 && w2 >= 0) || (w0 <= 0 && w1 <= 0 && w2 <= 0) {
                let idx = y as usize * width + x as usize;
                if idx < framebuffer.len() {
                    framebuffer[idx] = color;
                }
            }
        }
    }
}

#[inline]
fn edge(a: Point, b: Point, c: Point) -> i32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

#[inline]
fn rotation_fp(deg: i32) -> (i32, i32) {
    match deg {
        0 => (FP_ONE, 0),
        45 => (FP_INV_SQRT2, FP_INV_SQRT2),
        90 => (0, FP_ONE),
        180 => (-FP_ONE, 0),
        -90 => (0, -FP_ONE),
        -45 => (FP_INV_SQRT2, -FP_INV_SQRT2),
        135 => (-FP_INV_SQRT2, FP_INV_SQRT2),
        -135 => (-FP_INV_SQRT2, -FP_INV_SQRT2),
        _ => (FP_ONE, 0),
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
