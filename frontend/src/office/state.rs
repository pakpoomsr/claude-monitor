//! Game-state model for the Office tab.
//!
//! A `World` owns the set of `Character`s (one per agent session) and the
//! per-character animation state. Two entry points mutate it:
//!
//! - `sync_from_snapshots` — reconcile with the agent list (add/remove/
//!   update status). Called from a Leptos `Effect` so it runs on every
//!   change to the agents signal, not on every animation frame.
//! - `tick(dt)` — advance positions and frame counters. Called from the
//!   requestAnimationFrame loop.

use std::collections::HashMap;

use crate::types::{AgentSnapshot, AgentStatus};

use super::layout::{
    assign_desk, child_offset_from_parent, desk_grid, Facing, DOOR, ROOM_W,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimState {
    Idle,
    Walk,
    Typing,
    Reading,
    Running,
    Waiting,
    Error,
}

impl AnimState {
    /// Seconds-per-frame for stepped pixel-art animation. Lower = snappier.
    pub fn frame_duration(self) -> f32 {
        match self {
            AnimState::Idle => 0.55,
            AnimState::Walk => 0.12,
            AnimState::Typing => 0.10,
            AnimState::Reading => 0.50,
            AnimState::Running => 0.14,
            AnimState::Waiting => 0.45,
            AnimState::Error => 1.0,
        }
    }

    /// How many frames to cycle through. The exact visuals are produced
    /// at draw time from the single-frame sprite — the frame index just
    /// indexes into a transform table.
    pub fn frame_count(self) -> u32 {
        match self {
            AnimState::Idle => 2,
            AnimState::Walk => 4,
            AnimState::Typing => 4,
            AnimState::Reading => 2,
            AnimState::Running => 3,
            AnimState::Waiting => 2,
            AnimState::Error => 1,
        }
    }
}

fn anim_for(status: AgentStatus, tool: Option<&str>) -> AnimState {
    match status {
        AgentStatus::Idle => AnimState::Idle,
        AgentStatus::Waiting => AnimState::Waiting,
        AgentStatus::Error => AnimState::Error,
        AgentStatus::Working => match tool {
            Some(t) => {
                let t = t.to_ascii_lowercase();
                if t.contains("edit") || t.contains("write") || t.contains("notebook") {
                    AnimState::Typing
                } else if t.contains("read") || t.contains("grep") || t.contains("glob") {
                    AnimState::Reading
                } else if t.contains("bash") || t.contains("task") {
                    AnimState::Running
                } else {
                    AnimState::Typing
                }
            }
            None => AnimState::Typing,
        },
    }
}

#[derive(Debug, Clone)]
pub struct Character {
    pub session_id: String,
    pub sprite_name: String,
    pub pos: (f32, f32),
    pub target: (f32, f32),
    pub facing: Facing,
    pub anim: AnimState,
    pub frame: u32,
    pub frame_timer: f32,
    /// Status snapshot used to recompute `anim` when the character finishes
    /// walking.
    pub status: AgentStatus,
    pub current_tool: Option<String>,
    /// While true the character ignores status-driven anim and stays in Walk.
    pub walking_in: bool,
    /// True once the agent vanished from the snapshot list — character
    /// walks back to the door, then fades and is removed.
    pub leaving: bool,
    pub alpha: f32,
    /// Stable assignment so we don't reshuffle desks every render.
    pub desk_index: Option<usize>,
}

impl Character {
    /// Text to show in the speech bubble above the character's head, or
    /// `None` for no bubble.
    ///
    /// - Hidden while spawning (walking in), leaving (walking out), or in
    ///   the Idle / transient Walk state.
    /// - "Waiting..." with the trailing dots animated from `elapsed`.
    /// - During Working states the tool name is shown verbatim
    ///   ("TodoWrite...", "MultiEdit...") — Claude Code's tool names are
    ///   camelCase and that casing is recognizable at a glance. Falls back
    ///   to "Working..." if no tool is set.
    pub fn bubble_text(&self, elapsed: f32) -> Option<String> {
        if self.walking_in || self.leaving {
            return None;
        }
        let base: String = match self.anim {
            AnimState::Waiting => "Waiting".into(),
            AnimState::Typing | AnimState::Reading | AnimState::Running => {
                let raw = self
                    .current_tool
                    .as_deref()
                    .unwrap_or("Working");
                truncate_tool_label(raw)
            }
            AnimState::Error => return Some("Error".into()),
            AnimState::Idle | AnimState::Walk => return None,
        };
        Some(format!("{}{}", base, dots_for(elapsed)))
    }
}

/// Cap the tool label at 12 chars so the bubble never overflows the
/// room. Casing is preserved — Claude Code's tool names are camelCase
/// (`TodoWrite`, `MultiEdit`) and uppercasing them lost information.
/// Caller is responsible for the dots suffix.
fn truncate_tool_label(raw: &str) -> String {
    const MAX: usize = 12;
    if raw.chars().count() <= MAX {
        raw.to_string()
    } else {
        let mut out: String = raw.chars().take(MAX - 1).collect();
        out.push('.');
        out
    }
}

/// Always three trailing characters — visible dots up front, spaces in
/// the rest. Keeping the suffix a fixed width keeps the chat bubble from
/// jittering as the animation cycles.
fn dots_for(elapsed: f32) -> &'static str {
    let phase = ((elapsed / 0.4) as i32).rem_euclid(4);
    match phase {
        0 => "   ",
        1 => ".  ",
        2 => ".. ",
        _ => "...",
    }
}

pub struct World {
    pub characters: HashMap<String, Character>,
    /// Monotonically increasing seconds since the world was created.
    /// Drives time-based animations that aren't per-character (e.g. the
    /// dots in chat bubbles).
    pub elapsed: f32,
}

impl World {
    pub fn new() -> Self {
        Self { characters: HashMap::new(), elapsed: 0.0 }
    }

    /// Bring the world in line with the latest snapshot list. Spawns new
    /// characters, marks dropped (or now-Idle) sessions as leaving, and
    /// updates status / tool on existing characters.
    ///
    /// Idle agents are treated as absent: the office is meant to show
    /// only Working/Waiting/Error agents. When an agent transitions to
    /// Idle the character walks to the door and fades out.
    pub fn sync_from_snapshots(&mut self, snaps: &[AgentSnapshot]) {
        // 1. The "live" set is only the snapshots we want to display.
        //    Idle agents are intentionally filtered out — when an agent
        //    times out, its character leaves the office.
        let visible: Vec<&AgentSnapshot> = snaps
            .iter()
            .filter(|s| !matches!(s.status, AgentStatus::Idle))
            .collect();
        let live: std::collections::HashSet<&str> =
            visible.iter().map(|s| s.session_id.as_str()).collect();

        // 2. Mark dropped sessions as leaving (they'll walk to the door,
        //    fade, then get removed in `tick`).
        for (id, ch) in self.characters.iter_mut() {
            if !live.contains(id.as_str()) && !ch.leaving {
                ch.leaving = true;
                ch.walking_in = false;
                ch.target = (DOOR.0 as f32, DOOR.1 as f32);
                ch.anim = AnimState::Walk;
                ch.frame = 0;
                ch.frame_timer = 0.0;
            }
        }

        // 3. Decide sibling indices upfront: among all visible snaps
        //    sharing the same parent_id, order is stable by session_id
        //    so repeat syncs don't reshuffle.
        let mut sibling_by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for s in &visible {
            if let Some(pid) = &s.parent_id {
                sibling_by_parent
                    .entry(pid.clone())
                    .or_default()
                    .push(s.session_id.clone());
            }
        }
        for v in sibling_by_parent.values_mut() {
            v.sort();
        }

        // 4. Upsert each visible snapshot.
        let taken: HashMap<String, usize> = self
            .characters
            .iter()
            .filter(|(_, c)| !c.leaving)
            .filter_map(|(id, c)| c.desk_index.map(|d| (id.clone(), d)))
            .collect();
        let mut taken_mut = taken;

        for s in &visible {
            let sibling_index = s
                .parent_id
                .as_deref()
                .and_then(|pid| sibling_by_parent.get(pid))
                .and_then(|v| v.iter().position(|x| x == &s.session_id))
                .unwrap_or(0);

            match self.characters.get_mut(&s.session_id) {
                Some(ch) => {
                    ch.status = s.status;
                    ch.current_tool = s.current_tool.clone();
                    ch.leaving = false;
                    // If standing at target, switch animation to match status.
                    if !ch.walking_in && distance(ch.pos, ch.target) < 0.5 {
                        let next = anim_for(s.status, s.current_tool.as_deref());
                        if next != ch.anim {
                            ch.anim = next;
                            ch.frame = 0;
                            ch.frame_timer = 0.0;
                        }
                    }
                }
                None => {
                    // New character — assign a desk and spawn at the door.
                    let (target, facing, desk_index) = if let Some(pid) = &s.parent_id {
                        // Sub-agent: anchor to parent's desk.
                        if let Some(parent_ch) = self.characters.get(pid)
                            && let Some(pdesk) = parent_ch.desk_index
                        {
                            let desks = desk_grid();
                            let pd = desks[pdesk];
                            let (dx, dy) = child_offset_from_parent(pdesk, sibling_index);
                            (((pd.x + dx) as f32, (pd.y + dy) as f32), pd.facing, None)
                        } else {
                            // Parent not yet placed — fall back to a fresh desk.
                            fallback_desk(&taken_mut, &s.session_id)
                        }
                    } else {
                        fallback_desk(&taken_mut, &s.session_id)
                    };

                    if let Some(d) = desk_index {
                        taken_mut.insert(s.session_id.clone(), d);
                    }

                    let _ = sibling_index; // used only for offset above
                    self.characters.insert(
                        s.session_id.clone(),
                        Character {
                            session_id: s.session_id.clone(),
                            sprite_name: crate::types::avatar_for(&s.session_id).to_string(),
                            pos: (DOOR.0 as f32, DOOR.1 as f32),
                            target,
                            facing,
                            anim: AnimState::Walk,
                            frame: 0,
                            frame_timer: 0.0,
                            status: s.status,
                            current_tool: s.current_tool.clone(),
                            walking_in: true,
                            leaving: false,
                            alpha: 1.0,
                            desk_index,
                        },
                    );
                }
            }
        }
    }

    /// Advance positions and frame timers. `dt` is seconds since the last
    /// tick. Returns true if anything moved (caller can skip a redraw when
    /// the world is fully static, but doing so prematurely loses the bob
    /// animation — easier to always redraw at modest cost).
    pub fn tick(&mut self, dt: f32) {
        const WALK_SPEED: f32 = 60.0; // logical px / sec

        self.elapsed += dt;

        // Resolve walks (in-place to keep borrow checker simple).
        let mut to_remove: Vec<String> = Vec::new();
        for ch in self.characters.values_mut() {
            // Movement.
            let dx = ch.target.0 - ch.pos.0;
            let dy = ch.target.1 - ch.pos.1;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > 0.5 {
                let step = WALK_SPEED * dt;
                if step >= dist {
                    ch.pos = ch.target;
                } else {
                    ch.pos.0 += dx / dist * step;
                    ch.pos.1 += dy / dist * step;
                }
                ch.facing = if dx >= 0.0 { Facing::Right } else { Facing::Left };
                if ch.anim != AnimState::Walk {
                    ch.anim = AnimState::Walk;
                    ch.frame = 0;
                    ch.frame_timer = 0.0;
                }
            } else if ch.walking_in {
                // Arrived at desk — switch to status-driven anim.
                ch.walking_in = false;
                let next = anim_for(ch.status, ch.current_tool.as_deref());
                if next != ch.anim {
                    ch.anim = next;
                    ch.frame = 0;
                    ch.frame_timer = 0.0;
                }
            } else if ch.leaving {
                // Reached the door — fade out, then remove.
                ch.alpha = (ch.alpha - dt * 1.5).max(0.0);
                if ch.alpha <= 0.0 {
                    to_remove.push(ch.session_id.clone());
                }
            }

            // Frame timer.
            ch.frame_timer += dt;
            let dur = ch.anim.frame_duration();
            if ch.frame_timer >= dur {
                ch.frame_timer -= dur;
                let count = ch.anim.frame_count().max(1);
                ch.frame = (ch.frame + 1) % count;
            }
        }

        for id in to_remove {
            self.characters.remove(&id);
        }

        // Clamp positions inside the room (defensive — desks should keep
        // them in range already).
        for ch in self.characters.values_mut() {
            ch.pos.0 = ch.pos.0.clamp(0.0, ROOM_W as f32 - 16.0);
            ch.pos.1 = ch.pos.1.clamp(0.0, super::layout::ROOM_H as f32 - 16.0);
        }
    }
}

fn fallback_desk(
    taken: &HashMap<String, usize>,
    session_id: &str,
) -> ((f32, f32), Facing, Option<usize>) {
    let desks = desk_grid();
    match assign_desk(taken, session_id) {
        Some(i) => {
            let d = desks[i];
            ((d.x as f32, d.y as f32), d.facing, Some(i))
        }
        None => {
            // Office full — park at the door (no desk).
            ((DOOR.0 as f32 + 12.0, DOOR.1 as f32), Facing::Right, None)
        }
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, status: AgentStatus, tool: Option<&str>) -> AgentSnapshot {
        AgentSnapshot {
            session_id: id.into(),
            project: String::new(),
            status,
            current_message: String::new(),
            current_tool: tool.map(String::from),
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            cache_read_tokens: 0,
            cache_tokens: 0,
            cost_usd: 0.0,
            last_activity: String::new(),
            started_at: String::new(),
            parent_id: None,
        }
    }

    fn child_snap(id: &str, parent: &str, status: AgentStatus) -> AgentSnapshot {
        let mut s = snap(id, status, None);
        s.parent_id = Some(parent.into());
        s
    }

    // ---- anim_for routing ----

    #[test]
    fn anim_for_idle() {
        assert_eq!(anim_for(AgentStatus::Idle, None), AnimState::Idle);
        assert_eq!(anim_for(AgentStatus::Idle, Some("Edit")), AnimState::Idle);
    }

    #[test]
    fn anim_for_waiting_beats_tool() {
        // Waiting overrides any tool — Claude is paused for the user, not
        // actively working.
        assert_eq!(anim_for(AgentStatus::Waiting, Some("Bash")), AnimState::Waiting);
        assert_eq!(anim_for(AgentStatus::Waiting, None), AnimState::Waiting);
    }

    #[test]
    fn anim_for_error() {
        assert_eq!(anim_for(AgentStatus::Error, None), AnimState::Error);
    }

    #[test]
    fn anim_for_working_tool_buckets() {
        let cases = [
            (Some("Edit"), AnimState::Typing),
            (Some("Write"), AnimState::Typing),
            (Some("MultiEdit"), AnimState::Typing),
            (Some("NotebookEdit"), AnimState::Typing),
            (Some("Read"), AnimState::Reading),
            (Some("Grep"), AnimState::Reading),
            (Some("Glob"), AnimState::Reading),
            (Some("Bash"), AnimState::Running),
            (Some("Task"), AnimState::Running),
        ];
        for (tool, expected) in cases {
            assert_eq!(
                anim_for(AgentStatus::Working, tool),
                expected,
                "tool {tool:?} should map to {expected:?}",
            );
        }
    }

    #[test]
    fn anim_for_working_unknown_tool_defaults_typing() {
        assert_eq!(
            anim_for(AgentStatus::Working, Some("Mystery")),
            AnimState::Typing,
        );
        assert_eq!(anim_for(AgentStatus::Working, None), AnimState::Typing);
    }

    // ---- AnimState frame metadata ----

    #[test]
    fn every_anim_state_has_at_least_one_frame() {
        for s in [
            AnimState::Idle,
            AnimState::Walk,
            AnimState::Typing,
            AnimState::Reading,
            AnimState::Running,
            AnimState::Waiting,
            AnimState::Error,
        ] {
            assert!(s.frame_count() >= 1, "{s:?} must have >=1 frame");
            assert!(s.frame_duration() > 0.0, "{s:?} must have positive duration");
        }
    }

    // ---- World lifecycle ----

    #[test]
    fn new_world_is_empty() {
        let w = World::new();
        assert!(w.characters.is_empty());
    }

    #[test]
    fn sync_spawns_character_at_door_walking_in() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);

        assert_eq!(w.characters.len(), 1);
        let c = w.characters.get("a").unwrap();
        assert!(c.walking_in, "newly-spawned character should be walking in");
        assert_eq!(c.anim, AnimState::Walk);
        assert!(!c.leaving);
        assert_eq!(c.pos, (DOOR.0 as f32, DOOR.1 as f32));
        assert!(c.target != c.pos, "target should differ from door position");
    }

    #[test]
    fn sync_is_idempotent_on_repeated_snapshots() {
        let mut w = World::new();
        let snaps = vec![snap("a", AgentStatus::Working, Some("Edit"))];
        w.sync_from_snapshots(&snaps);
        w.sync_from_snapshots(&snaps);
        w.sync_from_snapshots(&snaps);
        assert_eq!(w.characters.len(), 1);
    }

    #[test]
    fn sync_marks_dropped_session_as_leaving() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        // Now sync with an empty list — character should head for the door.
        w.sync_from_snapshots(&[]);
        let c = w.characters.get("a").unwrap();
        assert!(c.leaving);
        assert!(!c.walking_in);
        assert_eq!(c.target, (DOOR.0 as f32, DOOR.1 as f32));
    }

    // ---- Tick / movement ----

    #[test]
    fn tick_moves_walking_character_toward_target() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        let before = w.characters.get("a").unwrap().pos;
        w.tick(0.5);
        let after = w.characters.get("a").unwrap().pos;
        let target = w.characters.get("a").unwrap().target;

        let dist_before = ((before.0 - target.0).powi(2) + (before.1 - target.1).powi(2)).sqrt();
        let dist_after = ((after.0 - target.0).powi(2) + (after.1 - target.1).powi(2)).sqrt();
        assert!(
            dist_after < dist_before,
            "character should be closer to target after tick (before={dist_before}, after={dist_after})",
        );
    }

    #[test]
    fn arrived_character_switches_to_status_anim() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Read"))]);
        // Tick enough times to definitely arrive (WALK_SPEED=60 px/s, room=240px).
        for _ in 0..30 {
            w.tick(0.2);
        }
        let c = w.characters.get("a").unwrap();
        assert!(!c.walking_in, "should have arrived");
        assert_eq!(c.anim, AnimState::Reading, "Read tool maps to Reading");
    }

    #[test]
    fn status_change_after_arrival_updates_anim() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Read"))]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        // Switch tool — should switch to Typing.
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        assert_eq!(w.characters.get("a").unwrap().anim, AnimState::Typing);
    }

    #[test]
    fn leaving_character_fades_and_is_removed() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        // Drop it.
        w.sync_from_snapshots(&[]);
        // Tick long enough for it to reach the door (the spawn position is
        // the door, so it's effectively there already) and fade.
        for _ in 0..30 {
            w.tick(0.1);
        }
        assert!(
            w.characters.get("a").is_none(),
            "fully-faded character should be removed",
        );
    }

    #[test]
    fn frame_timer_advances() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        // After arrival, accumulate enough time for at least one frame
        // transition (Typing frame duration is 0.10s).
        let frame_before = w.characters.get("a").unwrap().frame;
        for _ in 0..5 {
            w.tick(0.1);
        }
        let frame_after = w.characters.get("a").unwrap().frame;
        assert!(
            frame_after != frame_before || AnimState::Typing.frame_count() == 1,
            "frame index should cycle while animation is active",
        );
    }

    // ---- Sub-agent placement ----

    #[test]
    fn child_spawns_next_to_parent_when_parent_already_present() {
        let mut w = World::new();
        // Parent first.
        w.sync_from_snapshots(&[snap("parent", AgentStatus::Working, Some("Task"))]);
        // Tick to arrival.
        for _ in 0..30 {
            w.tick(0.2);
        }
        let parent_pos = w.characters.get("parent").unwrap().pos;

        // Now add a child.
        w.sync_from_snapshots(&[
            snap("parent", AgentStatus::Working, Some("Task")),
            child_snap("child", "parent", AgentStatus::Working),
        ]);
        let child = w.characters.get("child").unwrap();
        // Child's target should be near its parent (within ~3 tiles).
        let dx = (child.target.0 - parent_pos.0).abs();
        let dy = (child.target.1 - parent_pos.1).abs();
        assert!(dx <= 48.0, "child should target near parent x (dx={dx})");
        assert!(dy <= 48.0, "child should target near parent y (dy={dy})");
    }

    #[test]
    fn child_without_present_parent_falls_back_to_fresh_desk() {
        let mut w = World::new();
        // Child arrives before parent is in the snapshot list.
        w.sync_from_snapshots(&[child_snap("orphan", "missing_parent", AgentStatus::Working)]);
        let c = w.characters.get("orphan").unwrap();
        assert!(c.desk_index.is_some(), "orphan child should still get a desk");
    }

    // ---- Idle is hidden / leave-on-idle ----

    #[test]
    fn idle_agent_never_spawns_in_office() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Idle, None)]);
        assert!(
            w.characters.get("a").is_none(),
            "office should ignore Idle agents",
        );
    }

    #[test]
    fn working_to_idle_marks_character_leaving() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        // Now the agent goes idle (e.g. hit the idle timeout).
        w.sync_from_snapshots(&[snap("a", AgentStatus::Idle, None)]);
        let c = w.characters.get("a").unwrap();
        assert!(c.leaving, "idle agent's character should be heading for the door");
        assert_eq!(c.target, (DOOR.0 as f32, DOOR.1 as f32));
    }

    #[test]
    fn idle_then_working_lets_character_walk_back_in() {
        // Edge case: an agent flips Idle → Working between syncs. The
        // simplest behaviour is to spawn it fresh — it'll walk in from
        // the door again.
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        // Idle → character marked leaving.
        w.sync_from_snapshots(&[snap("a", AgentStatus::Idle, None)]);
        // Fully fade out.
        for _ in 0..40 {
            w.tick(0.1);
        }
        // Now it comes back working — should spawn a fresh character.
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        let c = w.characters.get("a").unwrap();
        assert!(c.walking_in);
        assert!(!c.leaving);
    }

    // ---- Bubble text ----

    #[test]
    fn bubble_text_hidden_while_walking_in() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        let c = w.characters.get("a").unwrap();
        assert!(c.walking_in);
        assert_eq!(c.bubble_text(0.0), None);
    }

    #[test]
    fn bubble_text_hidden_while_leaving() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("Edit"))]);
        w.sync_from_snapshots(&[]);
        let c = w.characters.get("a").unwrap();
        assert!(c.leaving);
        assert_eq!(c.bubble_text(0.0), None);
    }

    #[test]
    fn bubble_text_waiting_shows_waiting_with_dots_suffix() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Waiting, None)]);
        // Arrive at desk.
        for _ in 0..30 {
            w.tick(0.2);
        }
        let c = w.characters.get("a").unwrap();
        // Pad-suffix is always 3 chars wide; check that the base label is present.
        let text = c.bubble_text(0.0).expect("waiting agent should have a bubble");
        assert!(text.starts_with("Waiting"), "got: {text:?}");
        assert_eq!(text.chars().count(), "Waiting".len() + 3, "trailing 3 chars always reserved: {text:?}");
    }

    #[test]
    fn bubble_text_working_preserves_tool_casing() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("TodoWrite"))]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        let text = w
            .characters
            .get("a")
            .unwrap()
            .bubble_text(0.0)
            .expect("working agent should have a bubble");
        assert!(text.starts_with("TodoWrite"), "got: {text:?}");
    }

    #[test]
    fn bubble_text_preserves_camelcase_multi_edit() {
        // Regression guard: re-introducing `to_ascii_uppercase()` anywhere
        // in the bubble pipeline would collapse this to "MULTIEDIT".
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, Some("MultiEdit"))]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        let text = w.characters.get("a").unwrap().bubble_text(0.0).unwrap();
        assert!(text.starts_with("MultiEdit"), "got: {text:?}");
    }

    #[test]
    fn bubble_text_working_without_tool_falls_back_to_working_label() {
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Working, None)]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        let text = w.characters.get("a").unwrap().bubble_text(0.0).unwrap();
        assert!(text.starts_with("Working"), "got: {text:?}");
    }

    #[test]
    fn bubble_text_truncates_long_tool_names() {
        let mut w = World::new();
        w.sync_from_snapshots(&[
            snap("a", AgentStatus::Working, Some("VeryLongHypotheticalToolName")),
        ]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        let text = w.characters.get("a").unwrap().bubble_text(0.0).unwrap();
        // Tool label capped at 12 chars; total = 12 + 3 dots-suffix.
        assert_eq!(text.chars().count(), 12 + 3, "got: {text:?}");
    }

    #[test]
    fn bubble_text_dots_cycle_with_elapsed_time() {
        // The dots suffix should cycle through "   " / ".  " / ".. " / "..."
        // as the world clock advances.
        let mut w = World::new();
        w.sync_from_snapshots(&[snap("a", AgentStatus::Waiting, None)]);
        for _ in 0..30 {
            w.tick(0.2);
        }
        let c = w.characters.get("a").unwrap();

        let t0 = c.bubble_text(0.0).unwrap();    // phase 0 → "   "
        let t1 = c.bubble_text(0.4).unwrap();    // phase 1 → ".  "
        let t2 = c.bubble_text(0.8).unwrap();    // phase 2 → ".. "
        let t3 = c.bubble_text(1.2).unwrap();    // phase 3 → "..."

        // All same total length.
        assert_eq!(t0.chars().count(), t3.chars().count());

        // Strip the prefix to inspect just the suffix.
        let suffix = |s: &str| s.strip_prefix("Waiting").unwrap().to_string();
        assert_eq!(suffix(&t0), "   ");
        assert_eq!(suffix(&t1), ".  ");
        assert_eq!(suffix(&t2), ".. ");
        assert_eq!(suffix(&t3), "...");
    }

    #[test]
    fn world_elapsed_advances_with_ticks() {
        let mut w = World::new();
        assert_eq!(w.elapsed, 0.0);
        w.tick(0.5);
        w.tick(0.25);
        assert!((w.elapsed - 0.75).abs() < 1e-5, "elapsed = {}", w.elapsed);
    }
}
