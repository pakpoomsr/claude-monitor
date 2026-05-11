//! Deterministic desk-slot assignment for the Office tab.
//!
//! Eight desks arranged in a 4x2 grid, plus a "door" tile at the room's
//! left edge. Sub-agents claim a slot adjacent to their parent's desk.
//!
//! The world is rendered in logical pixels (later scaled by an integer
//! factor). All coordinates here are in those logical pixels.

use std::collections::HashMap;

/// Logical room dimensions, in pixels. Picked so the canvas tiles cleanly
/// at common integer scales (2x = 480x320; 3x = 720x480).
pub const ROOM_W: i32 = 240;
pub const ROOM_H: i32 = 160;

/// Tile pixel size. Used for movement step quantization and for placing
/// furniture on the grid.
pub const TILE: i32 = 16;

/// Where new characters walk in from. Sits just inside the top wall's
/// door cutout drawn in `render::draw_walls`.
pub const DOOR: (i32, i32) = (12, 26);

/// One desk position. Coordinates point at where the *character* stands
/// (i.e. the chair tile); the desk sprite is drawn slightly above.
#[derive(Debug, Clone, Copy)]
pub struct Desk {
    pub x: i32,
    pub y: i32,
    /// Direction the character faces while sitting at this desk.
    pub facing: Facing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Left,
    Right,
}

/// Eight desks in a 4-column, 2-row grid.
///
/// Coordinates point at the *character standing position* (their feet).
/// `render::draw_desks` adds the desk surface above that point and a
/// monitor to the right; the union footprint is 36 px wide × 16 px tall,
/// so columns are spaced 50 px apart to leave a clear gap between desks
/// while still fitting four columns inside the 240 px room.
pub fn desk_grid() -> [Desk; 8] {
    let cols = 4;
    let col_x0 = 30;
    let col_step = 50;
    let row_top = 60;
    let row_bot = 120;

    let mut desks = [Desk { x: 0, y: 0, facing: Facing::Right }; 8];
    for col in 0..cols {
        let x = col_x0 + col as i32 * col_step;
        desks[col * 2]     = Desk { x, y: row_top, facing: Facing::Right };
        desks[col * 2 + 1] = Desk { x, y: row_bot, facing: Facing::Right };
    }
    desks
}

/// First-fit desk picker. Returns the chosen desk index (0..8) or `None`
/// when all eight are taken — in which case the caller should park the
/// character at the door instead.
pub fn assign_desk(taken: &HashMap<String, usize>, _session_id: &str) -> Option<usize> {
    let desks = desk_grid();
    let used: std::collections::HashSet<usize> = taken.values().copied().collect();
    (0..desks.len()).find(|i| !used.contains(i))
}

/// Where a sub-agent should stand relative to its parent's desk. We use
/// the tile immediately to the right (or left, if the parent already
/// occupies the rightmost slot) so the family relationship reads visually.
pub fn child_offset_from_parent(parent_desk_idx: usize, child_index_in_parent: usize) -> (i32, i32) {
    // Children stack horizontally beside the parent's desk.
    let dx = TILE + (child_index_in_parent as i32) * (TILE - 2);
    // Mirror to the left for the rightmost column to keep them on-screen.
    let row_col = parent_desk_idx / 2;
    if row_col >= 3 {
        (-dx, 0)
    } else {
        (dx, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desk_grid_has_eight_in_room() {
        let desks = desk_grid();
        assert_eq!(desks.len(), 8);
        for d in desks.iter() {
            assert!(d.x >= 0 && d.x < ROOM_W, "desk x={} out of room", d.x);
            assert!(d.y >= 0 && d.y < ROOM_H, "desk y={} out of room", d.y);
        }
    }

    #[test]
    fn desk_positions_are_unique() {
        let desks = desk_grid();
        for (i, a) in desks.iter().enumerate() {
            for (j, b) in desks.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    a.x != b.x || a.y != b.y,
                    "desks {i} and {j} share coords",
                );
            }
        }
    }

    #[test]
    fn assign_desk_picks_first_free_slot() {
        let mut taken = HashMap::new();
        // No slots taken — should pick 0.
        assert_eq!(assign_desk(&taken, "a"), Some(0));
        taken.insert("a".into(), 0);
        // 0 taken — should pick 1.
        assert_eq!(assign_desk(&taken, "b"), Some(1));
    }

    #[test]
    fn assign_desk_skips_taken_slots() {
        let mut taken = HashMap::new();
        taken.insert("x".into(), 0);
        taken.insert("y".into(), 2);
        // Should skip 0 and 2, pick 1.
        assert_eq!(assign_desk(&taken, "z"), Some(1));
    }

    #[test]
    fn assign_desk_returns_none_when_full() {
        let mut taken = HashMap::new();
        for i in 0..8 {
            taken.insert(format!("{i}"), i);
        }
        assert_eq!(assign_desk(&taken, "extra"), None);
    }

    #[test]
    fn child_offset_pushes_right_for_left_parents() {
        // Parent at column 0 (desk_idx 0 or 1) → child on the right (dx > 0).
        let (dx, _) = child_offset_from_parent(0, 0);
        assert!(dx > 0, "left-column parent should put child to the right");
    }

    #[test]
    fn child_offset_pushes_left_for_rightmost_parents() {
        // Parent at column 3 (desk_idx 6 or 7) → child on the left (dx < 0).
        let (dx, _) = child_offset_from_parent(6, 0);
        assert!(dx < 0, "rightmost-column parent should put child to the left");
        let (dx2, _) = child_offset_from_parent(7, 0);
        assert!(dx2 < 0);
    }

    #[test]
    fn sibling_index_increases_offset_magnitude() {
        let (dx0, _) = child_offset_from_parent(0, 0);
        let (dx1, _) = child_offset_from_parent(0, 1);
        assert!(dx1.abs() > dx0.abs(), "later siblings should fan out further");
    }
}
