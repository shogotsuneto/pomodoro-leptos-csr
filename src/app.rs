use std::rc::Rc;

use gloo_timers::future::TimeoutFuture;
use leptos::callback::Callback;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;

use crate::settings_panel::SettingsPanel;
use crate::storage::indexeddb::IndexedDbStorage;
use crate::storage::{ActiveSession, PhaseKind, SessionRecord, Settings, Task};
use crate::tasks::{TaskPicker, TasksPanel};
use crate::timer::Phase;
use crate::util::{beep, log_err, now_ms, start_of_today_ms};

type StorageRef = StoredValue<Option<Rc<IndexedDbStorage>>, LocalStorage>;
type ActiveRef = StoredValue<Option<ActiveSession>, LocalStorage>;

/// Identifies which side panel is currently shown. `None` = none open. Adding
/// future panels (history, stats, ...) is a matter of extending this enum and
/// rendering another `*Panel` component in `App`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DrawerKind {
    Settings,
    Tasks,
}

/// UI write/read signals bundled together for the async helpers.
#[derive(Clone, Copy)]
struct Signals {
    seconds_left: WriteSignal<u32>,
    set_phase: WriteSignal<Phase>,
    completed: WriteSignal<u32>,
    completed_today: WriteSignal<u32>,
    is_running: WriteSignal<bool>,
    run_version: ReadSignal<u32>,
    set_run_version: WriteSignal<u32>,
    settings: ReadSignal<Settings>,
    /// Mirror of `active.get_value().is_some()` for reactive UI (the picker
    /// needs to know whether a session is in flight).
    set_active_present: WriteSignal<bool>,
    /// Mirror of `active.get_value().and_then(|a| a.session.task_id)` for the
    /// same reason — used by the picker to detect a mid-session switch.
    set_active_task_id: WriteSignal<Option<u64>>,
}

/// Recompute the reactive mirrors of `active`. Call this after every
/// `active.set_value(..)` so the picker's "applies from next session" hint
/// stays in sync.
fn sync_active_ui(active: ActiveRef, sigs: Signals) {
    let snap = active.get_value();
    sigs.set_active_present.set(snap.is_some());
    sigs.set_active_task_id
        .set(snap.and_then(|a| a.session.task_id));
}

async fn start_fresh_session(
    storage: StorageRef,
    active: ActiveRef,
    sigs: Signals,
    phase: Phase,
    ver: u32,
) {
    let now = now_ms();
    let pk: PhaseKind = phase.into();
    let settings = sigs.settings.get_untracked();
    let dur = settings.duration_secs(pk);
    // Only Work sessions are attributed to a task.
    let task_id = match pk {
        PhaseKind::Work => settings.selected_task_id,
        PhaseKind::Break => None,
    };
    let rec = SessionRecord::new(pk, now, dur, task_id);
    let id = match storage.get_value() {
        Some(s) => match s.start_session(&rec).await {
            Ok(id) => id,
            Err(e) => {
                log_err("start_session failed", e);
                0
            }
        },
        None => 0,
    };
    active.set_value(Some(ActiveSession::fresh(id, rec)));
    sync_active_ui(active, sigs);
    run_tick_loop(storage, active, sigs, ver).await;
}

async fn finalize_completion(
    storage: StorageRef,
    active: ActiveRef,
    sigs: Signals,
    completed_at_ms: i64,
) {
    let Some(a) = active.get_value() else { return };
    if let Some(s) = storage.get_value() {
        if let Err(e) = s.complete_session(a.session_id, completed_at_ms).await {
            log_err("complete session failed", e);
        }
    }

    let settings = sigs.settings.get_untracked();
    beep(880.0, 600.0, settings.beep_volume);
    if a.session.phase == PhaseKind::Work {
        sigs.completed.update(|c| *c += 1);
        // Only count toward today if the completion timestamp itself is
        // today — handles stale sessions finalized on app reopen after the
        // day has rolled over.
        if completed_at_ms >= start_of_today_ms() {
            sigs.completed_today.update(|c| *c += 1);
        }
    }
    let next = Phase::from(a.session.phase).next();
    sigs.set_phase.set(next);
    sigs.seconds_left.set(settings.duration_secs(next.into()));
    active.set_value(None);
    sync_active_ui(active, sigs);

    if settings.auto_start_next {
        let ver = sigs.run_version.get_untracked() + 1;
        sigs.set_run_version.set(ver);
        spawn_local(async move {
            start_fresh_session(storage, active, sigs, next, ver).await;
        });
    } else {
        sigs.is_running.set(false);
        sigs.set_run_version.update(|v| *v += 1);
    }
}

async fn run_tick_loop(storage: StorageRef, active: ActiveRef, sigs: Signals, ver: u32) {
    loop {
        if sigs.run_version.get_untracked() != ver {
            break;
        }
        let Some(a) = active.get_value() else { break };
        let remaining = a.remaining_secs(now_ms());
        sigs.seconds_left.set(remaining);
        if remaining == 0 {
            finalize_completion(storage, active, sigs, now_ms()).await;
            break;
        }
        TimeoutFuture::new(1_000).await;
    }
}

#[component]
pub fn App() -> impl IntoView {
    let initial = Settings::default();
    let (seconds_left, set_seconds_left) = signal(initial.duration_secs(PhaseKind::Work));
    let (is_running, set_is_running) = signal(false);
    let (phase, set_phase) = signal(Phase::Work);
    let (completed, set_completed) = signal(0u32);
    let (completed_today, set_completed_today) = signal(0u32);
    let (run_version, set_run_version) = signal(0u32);
    let (settings, set_settings) = signal(initial);
    let (drawer, set_drawer) = signal::<Option<DrawerKind>>(None);
    let (tasks, set_tasks) = signal::<Vec<(u64, Task)>>(Vec::new());
    let (active_present, set_active_present) = signal(false);
    let (active_task_id, set_active_task_id) = signal::<Option<u64>>(None);

    let storage: StorageRef = StoredValue::new_local(None);
    let active: ActiveRef = StoredValue::new_local(None);

    let sigs = Signals {
        seconds_left: set_seconds_left,
        set_phase,
        completed: set_completed,
        completed_today: set_completed_today,
        is_running: set_is_running,
        run_version,
        set_run_version,
        settings,
        set_active_present,
        set_active_task_id,
    };

    // When settings or phase change while no timer is in flight, reflect the
    // new configured duration in the display. During a running/paused session
    // the tick loop owns `seconds_left`, and changing settings mid-session
    // doesn't retroactively alter the active SessionRecord's duration.
    Effect::new(move || {
        let s = settings.get();
        let p = phase.get();
        if active.get_value().is_none() && !is_running.get_untracked() {
            set_seconds_left.set(s.duration_secs(p.into()));
        }
    });

    // Init: open DB, load settings + count, restore any active session.
    spawn_local(async move {
        let s = match IndexedDbStorage::open().await {
            Ok(s) => Rc::new(s),
            Err(e) => {
                log_err("storage init failed", e);
                return;
            }
        };
        storage.set_value(Some(s.clone()));

        match s.load_settings().await {
            Ok(loaded) => set_settings.set(loaded),
            Err(e) => log_err("load settings failed", e),
        }

        match s.completed_work_counts(start_of_today_ms()).await {
            Ok((total, today)) => {
                set_completed.set(total);
                set_completed_today.set(today);
            }
            Err(e) => log_err("load counts failed", e),
        }

        match s.list_tasks().await {
            Ok(list) => set_tasks.set(list),
            Err(e) => log_err("list_tasks failed", e),
        }

        match s.load_active().await {
            Ok(Some(a)) => {
                let p = Phase::from(a.session.phase);
                set_phase.set(p);
                let now = now_ms();
                let remaining = a.remaining_secs(now);

                if remaining == 0 {
                    let synthetic = a.session.started_at_ms
                        + a.session.duration_secs as i64 * 1000
                        + a.closed_paused_ms;
                    active.set_value(Some(a));
                    sync_active_ui(active, sigs);
                    finalize_completion(storage, active, sigs, synthetic).await;
                } else {
                    let was_running = !a.is_paused();
                    set_seconds_left.set(remaining);
                    active.set_value(Some(a));
                    sync_active_ui(active, sigs);
                    if was_running {
                        let ver = run_version.get_untracked() + 1;
                        set_run_version.set(ver);
                        set_is_running.set(true);
                        spawn_local(async move {
                            run_tick_loop(storage, active, sigs, ver).await;
                        });
                    }
                }
            }
            Ok(None) => {}
            Err(e) => log_err("load_active failed", e),
        }
    });

    let toggle = move |_| {
        if is_running.get_untracked() {
            set_is_running.set(false);
            set_run_version.update(|v| *v += 1);
            spawn_local(async move {
                let Some(mut a) = active.get_value() else { return };
                let paused_at = now_ms();
                let pause_id = match storage.get_value() {
                    Some(s) => match s.start_pause(a.session_id, paused_at).await {
                        Ok(id) => id,
                        Err(e) => {
                            log_err("start_pause failed", e);
                            0
                        }
                    },
                    None => 0,
                };
                a.open_pause = Some((pause_id, paused_at));
                active.set_value(Some(a));
                sync_active_ui(active, sigs);
            });
        } else {
            let ver = run_version.get_untracked() + 1;
            set_run_version.set(ver);
            set_is_running.set(true);
            spawn_local(async move {
                if let Some(mut a) = active.get_value() {
                    let now = now_ms();
                    if let Some((pause_id, paused_at)) = a.open_pause.take() {
                        if let Some(s) = storage.get_value() {
                            if let Err(e) = s.end_pause(pause_id, now).await {
                                log_err("end_pause failed", e);
                            }
                        }
                        a.closed_paused_ms += (now - paused_at).max(0);
                    }
                    active.set_value(Some(a));
                    sync_active_ui(active, sigs);
                    run_tick_loop(storage, active, sigs, ver).await;
                } else {
                    let p = phase.get_untracked();
                    start_fresh_session(storage, active, sigs, p, ver).await;
                }
            });
        }
    };

    let new_work = move |_| {
        let ver = run_version.get_untracked() + 1;
        set_run_version.set(ver);
        set_is_running.set(true);
        set_phase.set(Phase::Work);
        let work_dur = settings.get_untracked().duration_secs(PhaseKind::Work);
        set_seconds_left.set(work_dur);

        spawn_local(async move {
            if let Some(mut a) = active.get_value() {
                let now = now_ms();
                if let Some((pause_id, _)) = a.open_pause.take() {
                    if let Some(s) = storage.get_value() {
                        if let Err(e) = s.end_pause(pause_id, now).await {
                            log_err("end_pause on new_work failed", e);
                        }
                    }
                }
                if let Some(s) = storage.get_value() {
                    if let Err(e) = s.abandon_session(a.session_id, now).await {
                        log_err("abandon session failed", e);
                    }
                }
                active.set_value(None);
                sync_active_ui(active, sigs);
            }
            start_fresh_session(storage, active, sigs, Phase::Work, ver).await;
        });
    };

    let display = move || {
        let s = seconds_left.get();
        format!("{:02}:{:02}", s / 60, s % 60)
    };

    view! {
        <div class="top-bar">
            <button
                class="icon-btn"
                on:click=move |_| set_drawer.set(Some(DrawerKind::Tasks))
                aria-label="Tasks"
                title="Tasks"
            >
                "≡"
            </button>
            <button
                class="icon-btn"
                on:click=move |_| set_drawer.set(Some(DrawerKind::Settings))
                aria-label="Settings"
                title="Settings"
            >
                "⚙"
            </button>
        </div>
        <div class="container">
            <h1 class="phase">{move || phase.get().label()}</h1>
            <TaskPicker
                tasks=tasks
                set_tasks=set_tasks
                settings=settings
                set_settings=set_settings
                storage=storage
                has_active_session=active_present.into()
                active_task_id=active_task_id.into()
                on_open_manage=Callback::new(move |_| set_drawer.set(Some(DrawerKind::Tasks)))
            />
            <div class="clock">{display}</div>
            <div class="controls">
                <button
                    class:primary=move || !is_running.get()
                    on:click=toggle
                >
                    {move || if is_running.get() { "Pause" } else { "Start" }}
                </button>
                <button on:click=new_work>"New Work"</button>
            </div>
            <p class="completed">
                "Completed: " {completed_today} " today / " {completed} " total"
            </p>
        </div>
        <SettingsPanel
            is_open=Signal::derive(move || drawer.get() == Some(DrawerKind::Settings))
            on_close=Callback::new(move |_| set_drawer.set(None))
            settings=settings
            set_settings=set_settings
            storage=storage
        />
        <TasksPanel
            is_open=Signal::derive(move || drawer.get() == Some(DrawerKind::Tasks))
            on_close=Callback::new(move |_| set_drawer.set(None))
            tasks=tasks
            set_tasks=set_tasks
            storage=storage
        />
    }
}
