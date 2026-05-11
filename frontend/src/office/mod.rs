//! Office tab — pixel-art view where each agent is a character at a desk.
//!
//! Layered:
//! - `layout`: room geometry, desk slots, deterministic placement.
//! - `state`: `World`, `Character`, animation state machine.
//! - `render`: canvas draw routines.
//!
//! Wiring lives in `components::office_panel`.

pub mod layout;
pub mod render;
pub mod state;
