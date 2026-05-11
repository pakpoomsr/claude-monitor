//! Canvas rendering for the Office tab.
//!
//! Pure draw functions — they read from `World` + an image atlas and
//! issue Canvas2D draw calls. No game-state mutation here.

use std::collections::HashMap;

use web_sys::{CanvasRenderingContext2d, HtmlImageElement};

use crate::types::AgentStatus;

use super::layout::{desk_grid, Facing, ROOM_H, ROOM_W, TILE};
use super::state::{AnimState, Character, World};

/// Map from sprite name (e.g. "01_monitor_bot") to a loaded `<img>`.
pub type SpriteAtlas = HashMap<String, HtmlImageElement>;

/// Theme tokens. Pulled from CSS custom properties on demand so the
/// canvas tracks the active theme without us hard-coding two palettes.
pub struct Palette {
    pub floor: String,
    pub floor_alt: String,
    pub wall: String,
    pub wall_top: String,
    pub desk: String,
    pub desk_edge: String,
    pub screen: String,
    pub working: String,
    pub waiting: String,
    pub error: String,
    pub muted: String,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            floor: "#1a2240".into(),
            floor_alt: "#15193a".into(),
            wall: "#0d1428".into(),
            wall_top: "#222a4c".into(),
            desk: "#3a3530".into(),
            desk_edge: "#231f1c".into(),
            screen: "#5cd5ff".into(),
            working: "#4ade80".into(),
            waiting: "#facc15".into(),
            error: "#ff6b6b".into(),
            muted: "#7d87a8".into(),
        }
    }

    pub fn light() -> Self {
        Self {
            floor: "#e8ecf6".into(),
            floor_alt: "#dde2ef".into(),
            wall: "#f3f5fb".into(),
            wall_top: "#c9d0e2".into(),
            desk: "#b9886a".into(),
            desk_edge: "#7a5a44".into(),
            screen: "#1296c7".into(),
            working: "#15803d".into(),
            waiting: "#a16207".into(),
            error: "#b91c1c".into(),
            muted: "#6b7280".into(),
        }
    }

    pub fn from_theme(theme: &str) -> Self {
        if theme == "light" { Self::light() } else { Self::dark() }
    }
}

/// Draw the whole world. Caller is responsible for clearing the canvas
/// and applying the integer scale transform before calling this.
pub fn draw_world(
    ctx: &CanvasRenderingContext2d,
    world: &World,
    atlas: &SpriteAtlas,
    palette: &Palette,
) {
    draw_floor(ctx, palette);
    draw_walls(ctx, palette);
    draw_desks(ctx, palette);

    // Sort characters by y so closer ones render on top.
    let mut sorted: Vec<&Character> = world.characters.values().collect();
    sorted.sort_by(|a, b| a.pos.1.partial_cmp(&b.pos.1).unwrap_or(std::cmp::Ordering::Equal));
    for ch in sorted {
        draw_character(ctx, ch, world.elapsed, atlas, palette);
    }
}

fn fill(ctx: &CanvasRenderingContext2d, color: &str) {
    ctx.set_fill_style_str(color);
}

fn rect(ctx: &CanvasRenderingContext2d, x: i32, y: i32, w: i32, h: i32) {
    ctx.fill_rect(x as f64, y as f64, w as f64, h as f64);
}

fn draw_floor(ctx: &CanvasRenderingContext2d, p: &Palette) {
    fill(ctx, &p.floor);
    rect(ctx, 0, 0, ROOM_W, ROOM_H);

    // Subtle checkerboard so the room reads as having depth.
    fill(ctx, &p.floor_alt);
    let tile = TILE;
    let mut y = 0;
    while y < ROOM_H {
        let mut x = 0;
        while x < ROOM_W {
            if ((x / tile) + (y / tile)) % 2 == 0 {
                rect(ctx, x, y, tile, tile);
            }
            x += tile;
        }
        y += tile;
    }
}

fn draw_walls(ctx: &CanvasRenderingContext2d, p: &Palette) {
    // Top wall band.
    fill(ctx, &p.wall);
    rect(ctx, 0, 0, ROOM_W, TILE + 8);
    fill(ctx, &p.wall_top);
    rect(ctx, 0, TILE + 8, ROOM_W, 2);

    // Door cutout at left edge (lighter slot in the wall).
    fill(ctx, &p.floor_alt);
    rect(ctx, 6, 6, TILE - 2, TILE + 4);
}

fn draw_desks(ctx: &CanvasRenderingContext2d, p: &Palette) {
    let desks = desk_grid();
    for d in desks.iter() {
        // Desk surface sits two tiles wide, above the character standing
        // position. Character feet at (d.x, d.y); desk top at y - 14.
        let dx = d.x - 4;
        let dy = d.y - 14;
        fill(ctx, &p.desk_edge);
        rect(ctx, dx - 1, dy - 1, TILE * 2 + 2, TILE / 2 + 2);
        fill(ctx, &p.desk);
        rect(ctx, dx, dy, TILE * 2, TILE / 2);

        // Tiny monitor.
        fill(ctx, &p.desk_edge);
        rect(ctx, dx + TILE + 2, dy - 10, 10, 10);
        fill(ctx, &p.screen);
        rect(ctx, dx + TILE + 3, dy - 9, 8, 8);
    }
}

/// Per-frame transform offsets, all in integer pixels so the result stays
/// pixel-perfect. Returned values are (dx, dy) applied to the sprite's
/// position before drawing.
fn frame_offset(anim: AnimState, frame: u32) -> (i32, i32) {
    match anim {
        AnimState::Idle => match frame % 2 {
            0 => (0, 0),
            _ => (0, -1),
        },
        AnimState::Walk => match frame % 4 {
            0 => (0, 0),
            1 => (0, -1),
            2 => (0, 0),
            _ => (0, 1),
        },
        AnimState::Typing => match frame % 4 {
            0 => (-1, 0),
            1 => (0, 0),
            2 => (1, 0),
            _ => (0, 0),
        },
        AnimState::Reading => match frame % 2 {
            0 => (-1, 0),
            _ => (1, 0),
        },
        AnimState::Running => match frame % 3 {
            0 => (0, 0),
            1 => (1, -1),
            _ => (-1, 1),
        },
        AnimState::Waiting => match frame % 2 {
            0 => (0, 0),
            _ => (0, -1),
        },
        AnimState::Error => (0, 0),
    }
}

fn draw_character(
    ctx: &CanvasRenderingContext2d,
    ch: &Character,
    elapsed: f32,
    atlas: &SpriteAtlas,
    palette: &Palette,
) {
    let Some(img) = atlas.get(&ch.sprite_name) else {
        // Fallback: a small placeholder dot so the agent doesn't render invisible.
        fill(ctx, &palette.muted);
        rect(ctx, ch.pos.0 as i32, ch.pos.1 as i32, 12, 16);
        return;
    };
    if img.complete() == false || img.natural_width() == 0 {
        return;
    }

    let (fx, fy) = frame_offset(ch.anim, ch.frame);
    let sprite_w = 16f64;
    let sprite_h = 16f64;
    let dx = (ch.pos.0 as i32 + fx - (sprite_w as i32 / 2)) as f64;
    let dy = (ch.pos.1 as i32 + fy - sprite_h as i32) as f64;

    ctx.set_global_alpha(ch.alpha.clamp(0.0, 1.0) as f64);

    // Mirror horizontally when facing Left.
    if matches!(ch.facing, Facing::Left) {
        let _ = ctx.translate(dx + sprite_w, dy);
        let _ = ctx.scale(-1.0, 1.0);
        let _ = ctx.draw_image_with_html_image_element_and_dw_and_dh(
            img, 0.0, 0.0, sprite_w, sprite_h,
        );
        let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    } else {
        let _ = ctx.draw_image_with_html_image_element_and_dw_and_dh(
            img, dx, dy, sprite_w, sprite_h,
        );
    }
    ctx.set_global_alpha(1.0);

    // Chat bubble (the only status indicator now — colour matches the
    // status, so the old above-head pixel dot is redundant).
    if let Some(text) = ch.bubble_text(elapsed) {
        let bubble_bg = match ch.status {
            AgentStatus::Working => &palette.working,
            AgentStatus::Waiting => &palette.waiting,
            AgentStatus::Error => &palette.error,
            AgentStatus::Idle => &palette.muted,
        };
        draw_chat_bubble(ctx, ch.pos.0 as i32, ch.pos.1 as i32 - 18, &text, bubble_bg, palette);
    }
}

/// Width of a fixed string in our bitmap font, including the 1-pixel
/// inter-character gap (but not a trailing gap).
fn bitmap_text_width(text: &str) -> i32 {
    let n = text.chars().count() as i32;
    if n == 0 { 0 } else { n * (FONT_W + 1) - 1 }
}

const FONT_W: i32 = 3;
const FONT_H: i32 = 5;

/// 3x5 bitmap font. Each row is the low 3 bits of a u8 — bit 2 = left,
/// bit 0 = right. Glyphs cover both cases of A-Z, 0-9, and a few
/// punctuation marks. Lowercase letters share the 5-row box (no real
/// descenders below baseline) but use distinct shapes from their
/// uppercase counterparts so camelCase tool names like `TodoWrite` read
/// correctly. Unknown chars render as blank space.
fn glyph(c: char) -> [u8; 5] {
    match c {
        // ---- uppercase ----
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b110, 0b100, 0b110, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b110, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b011],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        // ---- lowercase ----
        // Letters that "fit" without a real descender drop the top row
        // so the glyph reads as shorter than its uppercase form. Letters
        // that need a descender (g j p q y) hang their tail in row 4.
        'a' => [0b000, 0b011, 0b101, 0b101, 0b011],
        'b' => [0b100, 0b100, 0b110, 0b101, 0b110],
        'c' => [0b000, 0b011, 0b100, 0b100, 0b011],
        'd' => [0b001, 0b001, 0b011, 0b101, 0b011],
        'e' => [0b000, 0b010, 0b111, 0b100, 0b011],
        'f' => [0b011, 0b010, 0b111, 0b010, 0b010],
        'g' => [0b000, 0b011, 0b101, 0b011, 0b110],
        'h' => [0b100, 0b100, 0b110, 0b101, 0b101],
        'i' => [0b010, 0b000, 0b010, 0b010, 0b010],
        'j' => [0b001, 0b000, 0b001, 0b001, 0b110],
        'k' => [0b100, 0b100, 0b101, 0b110, 0b101],
        'l' => [0b110, 0b010, 0b010, 0b010, 0b111],
        'm' => [0b000, 0b110, 0b111, 0b101, 0b101],
        'n' => [0b000, 0b110, 0b101, 0b101, 0b101],
        'o' => [0b000, 0b010, 0b101, 0b101, 0b010],
        'p' => [0b000, 0b110, 0b101, 0b110, 0b100],
        'q' => [0b000, 0b011, 0b101, 0b011, 0b001],
        'r' => [0b000, 0b110, 0b101, 0b100, 0b100],
        's' => [0b000, 0b011, 0b110, 0b011, 0b110],
        't' => [0b010, 0b111, 0b010, 0b010, 0b011],
        'u' => [0b000, 0b101, 0b101, 0b101, 0b011],
        'v' => [0b000, 0b101, 0b101, 0b101, 0b010],
        'w' => [0b000, 0b101, 0b101, 0b111, 0b101],
        'x' => [0b000, 0b101, 0b010, 0b010, 0b101],
        'y' => [0b000, 0b101, 0b101, 0b011, 0b110],
        'z' => [0b000, 0b111, 0b010, 0b100, 0b111],
        // ---- digits + punctuation ----
        '0' => [0b010, 0b101, 0b101, 0b101, 0b010],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b110, 0b101, 0b010],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b010, 0b101, 0b010, 0b101, 0b010],
        '9' => [0b010, 0b101, 0b011, 0b001, 0b110],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '?' => [0b110, 0b001, 0b010, 0b000, 0b010],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        _ => [0; 5],
    }
}

/// Draw a bitmap-font string at (x, y). The current fill color is used
/// for the glyph pixels.
fn draw_bitmap_text(ctx: &CanvasRenderingContext2d, text: &str, x: i32, y: i32) {
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        let cx = x + i as i32 * (FONT_W + 1);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..FONT_W {
                if bits & (1 << (FONT_W - 1 - col)) != 0 {
                    rect(ctx, cx + col, y + row as i32, 1, 1);
                }
            }
        }
    }
}

/// Draws a 1-px-bordered chat bubble centered on `anchor_x`, with its
/// bottom edge at `anchor_y` (i.e. just above the character's head).
///
/// `bg` is the bubble fill color; the border and the glyph pixels use
/// the dark wall color so the bubble pops on every theme. The caller is
/// responsible for keeping `text` at a constant width frame-to-frame
/// (e.g. by padding the trailing dots with spaces) so the bubble
/// doesn't pulse with the animation.
fn draw_chat_bubble(
    ctx: &CanvasRenderingContext2d,
    anchor_x: i32,
    anchor_y: i32,
    text: &str,
    bg: &str,
    palette: &Palette,
) {
    let pad_x = 2;
    let pad_y = 2;
    let bubble_w = bitmap_text_width(text) + pad_x * 2;
    let bubble_h = FONT_H + pad_y * 2;

    // Center on anchor_x, clamp inside the room so the bubble stays
    // visible when the character is up against a wall.
    let mut bx = anchor_x - bubble_w / 2;
    if bx < 1 {
        bx = 1;
    }
    if bx + bubble_w > ROOM_W - 1 {
        bx = ROOM_W - 1 - bubble_w;
    }
    let by = anchor_y - bubble_h - 2;

    // Border (1 px, dark).
    fill(ctx, &palette.wall);
    rect(ctx, bx - 1, by - 1, bubble_w + 2, bubble_h + 2);
    // Body.
    fill(ctx, bg);
    rect(ctx, bx, by, bubble_w, bubble_h);
    // Pointer (a 2x2 nub centered below the bubble, clamped to the body).
    let nub_x = (anchor_x - 1).clamp(bx + 1, bx + bubble_w - 3);
    fill(ctx, &palette.wall);
    rect(ctx, nub_x, by + bubble_h, 2, 2);
    fill(ctx, bg);
    rect(ctx, nub_x, by + bubble_h, 2, 1);

    // Text — left-aligned inside the padding box.
    fill(ctx, &palette.wall);
    draw_bitmap_text(ctx, text, bx + pad_x, by + pad_y);
}
