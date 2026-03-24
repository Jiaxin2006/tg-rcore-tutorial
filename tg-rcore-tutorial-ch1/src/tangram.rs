//! Tangram "OS" drawing primitives and rasterizer.
//!
//! Adjustable parameters:
//! - Colors: modify the color constants below
//! - Positions: modify coordinate values in `O_PIECES` / `S_PIECES`
//!
//! For ch2 extension (animate pieces one-by-one), use:
//! - `o_pieces()` / `s_pieces()` to get ordered piece slices
//! - `render_piece()` to draw a single piece
//! - `render_pieces()` to draw a prefix of pieces
//! - `render_progressive()` to show first N pieces directly
//! - `clear()` to wipe the framebuffer

#![cfg(target_arch = "riscv64")]

use tg_sbi::console_putchar;

/// RGBA8 packed as `0xAABBGGRR` for little-endian `u32` framebuffer.
type Color = u32;

pub(crate) const WHITE: Color = 0xFFFF_FFFF;
pub(crate) const RED: Color = 0xFF30_30E8;
pub(crate) const YELLOW: Color = 0xFF00_D7FF;
pub(crate) const CYAN: Color = 0xFFE0_B423;
pub(crate) const MAGENTA: Color = 0xFFC9_4ED8;
pub(crate) const PURPLE: Color = 0xFFDE_2A40;
pub(crate) const GREEN: Color = 0xFF40_EB49;
pub(crate) const ORANGE: Color = 0xFF00_8CFF;

#[derive(Clone, Copy)]
pub(crate) struct Point {
    pub x: i32,
    pub y: i32,
}

/// A tangram piece: either a triangle or a quadrilateral (parallelogram / square).
#[derive(Clone, Copy)]
pub(crate) enum Piece {
    /// Triangle defined by three vertices.
    Tri {
        p0: Point,
        p1: Point,
        p2: Point,
        color: Color,
    },
    /// Quadrilateral defined by four vertices (rendered as two triangles).
    Quad {
        p0: Point,
        p1: Point,
        p2: Point,
        p3: Point,
        color: Color,
    },
}

// ── Letter "O" pieces ────────────────────────────────────────────

pub(crate) const O_PIECES: [Piece; 6] = [
    // Top-left triangle — RED
    Piece::Tri {
        p0: Point { x: 80, y: 67 },
        p1: Point { x: 240, y: 67 },
        p2: Point { x: 80, y: 200 },
        color: RED,
    },
    // Left slanted piece — YELLOW
    Piece::Quad {
        p0: Point { x: 80, y: 200 },
        p1: Point { x: 240, y: 67 },
        p2: Point { x: 240, y: 467 },
        p3: Point { x: 80, y: 600 },
        color: YELLOW,
    },
    // Top-right large triangle — MAGENTA
    Piece::Tri {
        p0: Point { x: 240, y: 67 },
        p1: Point { x: 560, y: 67 },
        p2: Point { x: 560, y: 333 },
        color: MAGENTA,
    },
    // Right vertical piece — PURPLE
    Piece::Quad {
        p0: Point { x: 400, y: 200 },
        p1: Point { x: 560, y: 333 },
        p2: Point { x: 560, y: 600 },
        p3: Point { x: 400, y: 467 },
        color: PURPLE,
    },
    // Bottom diamond — GREEN
    Piece::Quad {
        p0: Point { x: 400, y: 467 },
        p1: Point { x: 560, y: 600 },
        p2: Point { x: 400, y: 733 },
        p3: Point { x: 240, y: 600 },
        color: GREEN,
    },
    // Bottom-left large triangle — CYAN
    Piece::Tri {
        p0: Point { x: 80, y: 467 },
        p1: Point { x: 400, y: 733 },
        p2: Point { x: 80, y: 733 },
        color: CYAN,
    },
];

// ── Letter "S" pieces ────────────────────────────────────────────

pub(crate) const S_PIECES: [Piece; 7] = [
    // Top-left triangle — CYAN
    Piece::Tri {
        p0: Point { x: 800, y: 200 },
        p1: Point { x: 960, y: 67 },
        p2: Point { x: 960, y: 333 },
        color: CYAN,
    },
    // Top notch — PURPLE
    Piece::Tri {
        p0: Point { x: 960, y: 67 },
        p1: Point { x: 1120, y: 67 },
        p2: Point { x: 1120, y: 200 },
        color: PURPLE,
    },
    // Top-right piece — MAGENTA
    Piece::Quad {
        p0: Point { x: 1120, y: 27 },
        p1: Point { x: 1279, y: 27 },
        p2: Point { x: 1279, y: 200 },
        p3: Point { x: 1120, y: 267 },
        color: MAGENTA,
    },
    // Center square — GREEN
    Piece::Quad {
        p0: Point { x: 960, y: 333 },
        p1: Point { x: 1120, y: 333 },
        p2: Point { x: 1120, y: 467 },
        p3: Point { x: 960, y: 467 },
        color: GREEN,
    },
    // Right middle triangle — MAGENTA
    Piece::Tri {
        p0: Point { x: 1120, y: 333 },
        p1: Point { x: 1279, y: 467 },
        p2: Point { x: 1120, y: 600 },
        color: MAGENTA,
    },
    // Bottom-left piece — ORANGE
    Piece::Quad {
        p0: Point { x: 800, y: 600 },
        p1: Point { x: 960, y: 600 },
        p2: Point { x: 1040, y: 733 },
        p3: Point { x: 880, y: 733 },
        color: ORANGE,
    },
    // Bottom-right triangle — PURPLE
    Piece::Tri {
        p0: Point { x: 960, y: 600 },
        p1: Point { x: 1120, y: 600 },
        p2: Point { x: 1040, y: 733 },
        color: PURPLE,
    },
];

// ── Public rendering API ─────────────────────────────────────────

/// Return the ordered pieces that form the letter `O`.
#[allow(dead_code)]
pub(crate) fn o_pieces() -> &'static [Piece] {
    &O_PIECES
}

/// Return the ordered pieces that form the letter `S`.
#[allow(dead_code)]
pub(crate) fn s_pieces() -> &'static [Piece] {
    &S_PIECES
}

/// Clear the entire framebuffer to the given color.
pub(crate) fn clear(framebuffer: &mut [u32], color: Color) {
    for px in framebuffer.iter_mut() {
        *px = color;
    }
}

/// Render a single piece onto the framebuffer.
pub(crate) fn render_piece(framebuffer: &mut [u32], width: usize, height: usize, piece: &Piece) {
    match *piece {
        Piece::Tri {
            p0,
            p1,
            p2,
            color,
        } => {
            fill_triangle(framebuffer, width, height, p0, p1, p2, color);
        }
        Piece::Quad {
            p0,
            p1,
            p2,
            p3,
            color,
        } => {
            fill_triangle(framebuffer, width, height, p0, p1, p2, color);
            fill_triangle(framebuffer, width, height, p0, p2, p3, color);
        }
    }
}

/// Render a slice of pieces in order.
pub(crate) fn render_pieces(
    framebuffer: &mut [u32],
    width: usize,
    height: usize,
    pieces: &[Piece],
) {
    for piece in pieces {
        render_piece(framebuffer, width, height, piece);
    }
}

/// Clear framebuffer to white, then render the first `o_count` and `s_count`
/// pieces. This is useful for ch2 piece-by-piece display.
#[allow(dead_code)]
pub(crate) fn render_progressive(
    framebuffer: &mut [u32],
    width: usize,
    height: usize,
    o_count: usize,
    s_count: usize,
) {
    clear(framebuffer, WHITE);
    render_pieces(framebuffer, width, height, &O_PIECES[..o_count.min(O_PIECES.len())]);
    render_pieces(framebuffer, width, height, &S_PIECES[..s_count.min(S_PIECES.len())]);
}

/// Clear framebuffer to white, then render the complete "OS" tangram.
pub(crate) fn render_tangram(framebuffer: &mut [u32], width: usize, height: usize) {
    log_str("tangram: render start\n");
    clear(framebuffer, WHITE);
    render_pieces(framebuffer, width, height, &O_PIECES);
    log_str("tangram: O done\n");
    render_pieces(framebuffer, width, height, &S_PIECES);
    log_str("tangram: S done\n");
}

// ── Rasterizer ───────────────────────────────────────────────────

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

// ── Logging helpers ──────────────────────────────────────────────

fn log_str(msg: &str) {
    for c in msg.bytes() {
        console_putchar(c);
    }
}
