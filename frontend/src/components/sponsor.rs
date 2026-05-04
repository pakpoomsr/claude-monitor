use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::invoke;

#[derive(Serialize)]
struct OpenUrlArgs {
    url: String,
}

const GITHUB_SPONSORS: &str = "https://github.com/sponsors/pakpoomsr";
const BUY_ME_A_COFFEE: &str = "https://buymeacoffee.com/pakpoomsr";
const ISSUES_URL: &str = "https://github.com/pakpoomsr/claude-monitor/issues";

fn open(url: &'static str) -> impl Fn(leptos::ev::MouseEvent) + 'static {
    move |_| {
        leptos::task::spawn_local(async move {
            let _ = invoke::<(), _>(
                "plugin:opener|open_url",
                &OpenUrlArgs { url: url.to_string() },
            )
            .await;
        });
    }
}

#[component]
pub fn SponsorPanel() -> impl IntoView {
    view! {
        <section class="panel sponsor-panel">
            <div class="sponsor-hero">
                <h2>"Support Claude Monitor"</h2>
                <p class="sponsor-pitch">
                    "Claude Monitor is free and MIT-licensed. If it saves you time watching your agents, \
                     consider sponsoring continued work — it directly funds new features on the roadmap."
                </p>
            </div>

            <div class="sponsor-actions">
                <button class="btn primary sponsor-btn" on:click=open(GITHUB_SPONSORS)>
                    <span class="sponsor-btn-icon sponsor-btn-icon--heart">"♥"</span>
                    <span>
                        <strong>"Sponsor on GitHub"</strong>
                        <small class="muted">"Recurring or one-time, every tier helps."</small>
                    </span>
                </button>

                <button class="btn sponsor-btn" on:click=open(BUY_ME_A_COFFEE)>
                    <span class="sponsor-btn-icon">"☕"</span>
                    <span>
                        <strong>"Buy Me a Coffee"</strong>
                        <small class="muted">"One-off tip, no account needed."</small>
                    </span>
                </button>

                <button class="btn sponsor-btn" on:click=open(ISSUES_URL)>
                    <span class="sponsor-btn-icon">"!"</span>
                    <span>
                        <strong>"Open an issue"</strong>
                        <small class="muted">"Bugs, feature requests, ideas."</small>
                    </span>
                </button>
            </div>

            <small class="muted sponsor-footer">
                "Using this in a team or company context? Open a discussion to talk priority features \
                 (team rollup, cloud sync, integrations)."
            </small>
        </section>
    }
}
