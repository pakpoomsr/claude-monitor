//! Office tab — pixel-art canvas that mirrors the agent grid as
//! characters seated at desks. See `crate::office` for state and render.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement};

use crate::office::layout::{ROOM_H, ROOM_W};
use crate::office::render::{draw_world, Palette, SpriteAtlas};
use crate::office::state::World;
use crate::types::AgentSnapshot;

/// All 8 character sprites we want to preload. Names match `AVATARS`
/// in `types.rs`; PNGs are served by trunk from `frontend/avatars/`.
const SPRITE_NAMES: [&str; 8] = [
    "01_monitor_bot",
    "02_teardrop_bot",
    "03_turtle_bot",
    "04_round_bot",
    "05_cat_bot",
    "06_fox_bot",
    "07_red_probe_bot",
    "08_owl_bot",
];

#[component]
pub fn OfficePanel(agents: ReadSignal<Vec<AgentSnapshot>>) -> impl IntoView {
    // Refs into the DOM.
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Shared mutable state — `World` and the sprite atlas live across
    // animation frames, so they need Rc<RefCell<...>> to be captured by
    // the rAF closure and the Effect at the same time.
    let world = Rc::new(RefCell::new(World::new()));
    let atlas: Rc<RefCell<SpriteAtlas>> = Rc::new(RefCell::new(SpriteAtlas::new()));

    // Sync world from snapshots whenever the agents signal changes. Runs
    // once on mount, then on every push from the `agent-status` listener.
    {
        let world = world.clone();
        Effect::new(move |_| {
            let snaps = agents.get();
            world.borrow_mut().sync_from_snapshots(&snaps);
        });
    }

    // On first mount: preload sprites and start the rAF loop.
    {
        let world = world.clone();
        let atlas = atlas.clone();
        Effect::new(move |prev: Option<()>| {
            if prev.is_some() {
                return;
            }
            // Preload sprites into the atlas.
            for name in SPRITE_NAMES.iter() {
                let url = format!("/avatars/{name}/{name}_32.png");
                let img = HtmlImageElement::new().unwrap();
                img.set_src(&url);
                // Use `image-rendering: pixelated` on the canvas; the
                // HtmlImageElement itself doesn't have that CSS, but
                // canvas with smoothing disabled handles it.
                atlas.borrow_mut().insert((*name).to_string(), img);
            }

            // Set up the rAF loop.
            let Some(canvas_el) = canvas_ref.get() else { return };
            let canvas: HtmlCanvasElement = canvas_el.unchecked_into();

            // Logical resolution; we'll scale up via CSS so the canvas
            // stays crisp regardless of physical pixel ratio.
            canvas.set_width(ROOM_W as u32);
            canvas.set_height(ROOM_H as u32);

            let ctx = canvas
                .get_context("2d")
                .ok()
                .flatten()
                .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok());
            let Some(ctx) = ctx else { return };
            ctx.set_image_smoothing_enabled(false);

            let world = world.clone();
            let atlas = atlas.clone();
            let canvas_for_loop = canvas.clone();

            let cb: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> =
                Rc::new(RefCell::new(None));
            let cb_outer = cb.clone();

            let mut last_ts: Option<f64> = None;
            *cb.borrow_mut() = Some(Closure::wrap(Box::new(move |ts: f64| {
                // Bail out (and let the closure drop) once the canvas
                // has been removed from the DOM — e.g. user switched
                // away from the Office tab. Without this guard each
                // tab visit would leave a new rAF chain running.
                if !canvas_for_loop.is_connected() {
                    cb_outer.borrow_mut().take();
                    return;
                }

                let dt = match last_ts {
                    Some(prev) => ((ts - prev) as f32 / 1000.0).clamp(0.0, 0.25),
                    None => 0.0,
                };
                last_ts = Some(ts);

                world.borrow_mut().tick(dt);

                // Clear and draw.
                ctx.clear_rect(0.0, 0.0, ROOM_W as f64, ROOM_H as f64);
                ctx.set_image_smoothing_enabled(false);
                let palette = Palette::from_theme(&current_theme());
                draw_world(&ctx, &world.borrow(), &atlas.borrow(), &palette);

                // Schedule next frame.
                if let Some(window) = web_sys::window()
                    && let Some(cb_inner) = cb_outer.borrow().as_ref()
                {
                    let _ = window.request_animation_frame(cb_inner.as_ref().unchecked_ref());
                }
            }) as Box<dyn FnMut(f64)>));

            if let Some(window) = web_sys::window()
                && let Some(cb_inner) = cb.borrow().as_ref()
            {
                let _ = window.request_animation_frame(cb_inner.as_ref().unchecked_ref());
            }
        });
    }

    let is_empty = Signal::derive(move || agents.with(|a| a.is_empty()));

    view! {
        <div class="office-panel">
            <div class="office-stage">
                <canvas
                    class="office-canvas"
                    node_ref=canvas_ref
                    width=ROOM_W
                    height=ROOM_H
                ></canvas>
                <Show when=move || is_empty.get() fallback=|| ().into_any()>
                    <div class="office-empty">
                        <p class="muted">"Waiting for agents — start a Claude session and a character will walk in."</p>
                    </div>
                </Show>
            </div>
            <div class="office-legend muted">
                <span class="legend-row"><span class="legend-swatch legend-working"></span> "Working"</span>
                <span class="legend-row"><span class="legend-swatch legend-waiting"></span> "Waiting"</span>
                <span class="legend-row"><span class="legend-swatch legend-idle"></span> "Idle"</span>
                <span class="legend-row"><span class="legend-swatch legend-error"></span> "Error"</span>
            </div>
        </div>
    }
}

fn current_theme() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .unwrap_or_else(|| "dark".to_string())
}
