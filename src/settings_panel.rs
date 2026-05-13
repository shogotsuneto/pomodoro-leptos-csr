// Settings drawer + a tiny generic `DrawerShell` it sits inside.
//
// `DrawerShell` is the reusable bit: backdrop, slide-in panel, header with
// title and close button, and a body slot. Future drawers (history, stats)
// drop into the same shell. The shell only deals with open/close + layout;
// each consumer renders its own form/list as children.

use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{Event, HtmlInputElement};

use crate::storage::indexeddb::IndexedDbStorage;
use crate::storage::Settings;
use crate::util::{beep, log_err};

type StorageRef = StoredValue<Option<Rc<IndexedDbStorage>>, LocalStorage>;

#[component]
pub fn DrawerShell(
    is_open: Signal<bool>,
    on_close: Callback<()>,
    title: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            class="drawer-backdrop"
            class:open=move || is_open.get()
            on:click=move |_| on_close.run(())
        ></div>
        <aside class="drawer" class:open=move || is_open.get()>
            <header class="drawer-header">
                <h2>{title}</h2>
                <button
                    class="drawer-close"
                    on:click=move |_| on_close.run(())
                    aria-label="Close"
                >
                    "×"
                </button>
            </header>
            <div class="drawer-body">
                {children()}
            </div>
        </aside>
    }
}

fn input_value(ev: &Event) -> Option<String> {
    ev.target()?
        .dyn_into::<HtmlInputElement>()
        .ok()
        .map(|el| el.value())
}

#[component]
pub fn SettingsPanel(
    is_open: Signal<bool>,
    on_close: Callback<()>,
    settings: ReadSignal<Settings>,
    set_settings: WriteSignal<Settings>,
    storage: StorageRef,
) -> impl IntoView {
    // Apply a new Settings to both the live signal and IndexedDB.
    let persist = move |new: Settings| {
        set_settings.set(new.clone());
        spawn_local(async move {
            if let Some(s) = storage.get_value() {
                if let Err(e) = s.save_settings(&new).await {
                    log_err("save settings failed", e);
                }
            }
        });
    };

    let on_work_change = move |ev: Event| {
        if let Some(v) = input_value(&ev).and_then(|s| s.parse::<u32>().ok()) {
            let mut s = settings.get_untracked();
            s.work_minutes = v.clamp(1, 120);
            persist(s);
        }
    };

    let on_break_change = move |ev: Event| {
        if let Some(v) = input_value(&ev).and_then(|s| s.parse::<u32>().ok()) {
            let mut s = settings.get_untracked();
            s.break_minutes = v.clamp(1, 60);
            persist(s);
        }
    };

    let on_volume_change = move |ev: Event| {
        if let Some(v) = input_value(&ev).and_then(|s| s.parse::<f32>().ok()) {
            let volume = v.clamp(0.0, 1.0);
            let mut s = settings.get_untracked();
            s.beep_volume = volume;
            persist(s);
            // Preview at the new level so the user can hear what they picked.
            beep(880.0, 250.0, volume);
        }
    };

    let on_auto_start_change = move |ev: Event| {
        let checked = event_target_checked(&ev);
        let mut s = settings.get_untracked();
        s.auto_start_next = checked;
        persist(s);
    };

    view! {
        <DrawerShell is_open=is_open on_close=on_close title="Settings">
            <div class="setting">
                <label for="work-min">"Work duration (minutes)"</label>
                <input
                    id="work-min"
                    type="number"
                    min="1"
                    max="120"
                    prop:value=move || settings.get().work_minutes.to_string()
                    on:change=on_work_change
                />
            </div>
            <div class="setting">
                <label for="break-min">"Break duration (minutes)"</label>
                <input
                    id="break-min"
                    type="number"
                    min="1"
                    max="60"
                    prop:value=move || settings.get().break_minutes.to_string()
                    on:change=on_break_change
                />
            </div>
            <div class="setting">
                <label for="volume">
                    {move || format!("Beep volume: {:.0}%", settings.get().beep_volume * 100.0)}
                </label>
                <input
                    id="volume"
                    type="range"
                    min="0"
                    max="1"
                    step="0.05"
                    prop:value=move || settings.get().beep_volume.to_string()
                    on:change=on_volume_change
                />
            </div>
            <div class="setting row">
                <input
                    id="auto-start"
                    type="checkbox"
                    prop:checked=move || settings.get().auto_start_next
                    on:change=on_auto_start_change
                />
                <label for="auto-start">"Auto-start next session"</label>
            </div>
        </DrawerShell>
    }
}
