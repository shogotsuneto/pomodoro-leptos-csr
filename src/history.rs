// Session history drawer. Loads recent terminated sessions from
// IndexedDB lazily when the drawer opens, then filters down to
// completed Work sessions for display. Task names are resolved
// reactively against the live `tasks` signal so a rename shows up
// immediately in the history list too.

use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;

use crate::settings_panel::DrawerShell;
use crate::storage::indexeddb::IndexedDbStorage;
use crate::storage::{PhaseKind, SessionRecord};
use crate::tasks::TaskRow;
use crate::util::{log_err, start_of_today_ms};

type StorageRef = StoredValue<Option<Rc<IndexedDbStorage>>, LocalStorage>;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const HISTORY_LIMIT: usize = 100;

/// "Today HH:MM" / "Yesterday HH:MM" / "Mon D HH:MM". The local-midnight
/// boundary is the same one used by `completed_today`, so today's items
/// here line up with the main-screen counter.
fn format_when(started_at_ms: i64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(started_at_ms as f64));
    let time_str = format_time_of_day_inner(&d);
    let today_start = start_of_today_ms();
    let day_ms: i64 = 24 * 60 * 60 * 1000;
    if started_at_ms >= today_start {
        format!("Today {time_str}")
    } else if started_at_ms >= today_start - day_ms {
        format!("Yesterday {time_str}")
    } else {
        let month_idx = (d.get_month() as usize).min(11);
        let day = d.get_date() as u32;
        format!("{} {} {}", MONTHS[month_idx], day, time_str)
    }
}

fn format_time_of_day(ms: i64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    format_time_of_day_inner(&d)
}

fn format_time_of_day_inner(d: &js_sys::Date) -> String {
    format!("{:02}:{:02}", d.get_hours() as u32, d.get_minutes() as u32)
}

fn format_duration(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    if s == 0 {
        format!("{m}m")
    } else {
        format!("{m}:{s:02}")
    }
}

#[component]
pub fn HistoryPanel(
    is_open: Signal<bool>,
    on_close: Callback<()>,
    storage: StorageRef,
    tasks: ReadSignal<Vec<TaskRow>>,
) -> impl IntoView {
    let (entries, set_entries) = signal::<Vec<(u64, SessionRecord)>>(Vec::new());

    // Refetch each time the drawer is opened so a session completed since
    // last view shows up. Cheap relative to user gesture cost — no need to
    // also auto-refresh while open.
    Effect::new(move |_| {
        if is_open.get() {
            spawn_local(async move {
                let Some(s) = storage.get_value() else { return };
                match s.list_session_history(HISTORY_LIMIT).await {
                    Ok(list) => set_entries.set(list),
                    Err(e) => log_err("list_session_history failed", e),
                }
            });
        }
    });

    // History is intentionally minimal — only completed Work sessions, since
    // those are what users actually want to look back at. Break sessions and
    // abandoned attempts are noise here.
    let visible_entries = move || {
        entries
            .get()
            .into_iter()
            .filter(|(_, rec)| {
                matches!(rec.phase, PhaseKind::Work) && rec.completed_at_ms.is_some()
            })
            .collect::<Vec<_>>()
    };

    view! {
        <DrawerShell is_open=is_open on_close=on_close title="History">
            <Show
                when=move || !visible_entries().is_empty()
                fallback=|| view! { <p class="task-empty">"No sessions yet."</p> }
            >
                <ul class="history-list">
                    <For
                        each=visible_entries
                        key=|(id, _)| *id
                        children=move |(_id, rec)| {
                            let started = format_when(rec.started_at_ms);
                            let ended = rec
                                .completed_at_ms
                                .map(format_time_of_day)
                                .unwrap_or_default();
                            let dur = format_duration(rec.duration_secs);
                            let task_id = rec.task_id;
                            let task_name = move || {
                                task_id.and_then(|tid| {
                                    tasks
                                        .get()
                                        .iter()
                                        .find(|r| r.id == tid)
                                        .map(|r| r.name.get())
                                })
                            };
                            view! {
                                <li class="history-item">
                                    <Show when=move || task_name().is_some()>
                                        <div class="history-task">
                                            {move || task_name().unwrap_or_default()}
                                        </div>
                                    </Show>
                                    <div class="history-meta">
                                        {started} " – " {ended} " · " {dur}
                                    </div>
                                </li>
                            }
                        }
                    />
                </ul>
            </Show>
        </DrawerShell>
    }
}
