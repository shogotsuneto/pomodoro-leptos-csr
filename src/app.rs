use std::rc::Rc;

use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;
use web_sys::{AudioContext, OscillatorType};

use crate::storage::indexeddb::IndexedDbStorage;
use crate::storage::{ActiveSession, PhaseKind, SessionRecord};
use crate::timer::Phase;

type StorageRef = StoredValue<Option<Rc<IndexedDbStorage>>, LocalStorage>;
type ActiveRef = StoredValue<Option<ActiveSession>, LocalStorage>;

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn log_err(prefix: &str, e: impl std::fmt::Display) {
    web_sys::console::error_1(&format!("{prefix}: {e}").into());
}

fn beep(frequency: f32, duration_ms: f64) {
    let Ok(ctx) = AudioContext::new() else { return };
    let Ok(oscillator) = ctx.create_oscillator() else { return };
    let Ok(gain) = ctx.create_gain() else { return };

    oscillator.set_type(OscillatorType::Sine);
    oscillator.frequency().set_value(frequency);
    gain.gain().set_value(0.4);

    let _ = oscillator.connect_with_audio_node(&gain);
    let _ = gain.connect_with_audio_node(&ctx.destination());

    let now = ctx.current_time();
    oscillator.start().ok();
    gain.gain()
        .exponential_ramp_to_value_at_time(0.001, now + duration_ms / 1000.0)
        .ok();
    oscillator.stop_with_when(now + duration_ms / 1000.0).ok();
    let _ = JsValue::from(ctx);
}

/// UI write/read signals bundled together for the async helpers.
#[derive(Clone, Copy)]
struct Signals {
    seconds_left: WriteSignal<u32>,
    set_phase: WriteSignal<Phase>,
    completed: WriteSignal<u32>,
    is_running: WriteSignal<bool>,
    run_version: ReadSignal<u32>,
    set_run_version: WriteSignal<u32>,
}

async fn finalize_completion(
    storage: StorageRef,
    active: ActiveRef,
    sigs: Signals,
    completed_at_ms: i64,
) {
    let Some(mut a) = active.get_value() else { return };
    if let Some(s) = storage.get_value() {
        if let Err(e) = s.complete_session(a.session_id, completed_at_ms).await {
            log_err("complete session failed", e);
        }
    }
    a.session.completed_at_ms = Some(completed_at_ms);

    beep(880.0, 600.0);
    if a.session.phase == PhaseKind::Work {
        sigs.completed.update(|c| *c += 1);
    }
    let next = Phase::from(a.session.phase).next();
    sigs.set_phase.set(next);
    sigs.seconds_left.set(next.duration_secs());
    sigs.is_running.set(false);
    active.set_value(None);
    sigs.set_run_version.update(|v| *v += 1);
}

/// Ticks once per second, recomputing remaining time from wall clock so the
/// display stays correct across timer throttling and OS sleep.
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
    let (seconds_left, set_seconds_left) = signal(Phase::Work.duration_secs());
    let (is_running, set_is_running) = signal(false);
    let (phase, set_phase) = signal(Phase::Work);
    let (completed, set_completed) = signal(0u32);
    let (run_version, set_run_version) = signal(0u32);

    let storage: StorageRef = StoredValue::new_local(None);
    let active: ActiveRef = StoredValue::new_local(None);

    let sigs = Signals {
        seconds_left: set_seconds_left,
        set_phase,
        completed: set_completed,
        is_running: set_is_running,
        run_version,
        set_run_version,
    };

    // Init: open DB, load count, restore any active session.
    spawn_local(async move {
        let s = match IndexedDbStorage::open().await {
            Ok(s) => Rc::new(s),
            Err(e) => {
                log_err("storage init failed", e);
                return;
            }
        };
        storage.set_value(Some(s.clone()));

        match s.completed_work_count().await {
            Ok(c) => set_completed.set(c),
            Err(e) => log_err("load count failed", e),
        }

        match s.load_active().await {
            Ok(Some(a)) => {
                let p = Phase::from(a.session.phase);
                set_phase.set(p);
                let now = now_ms();
                let remaining = a.remaining_secs(now);

                if remaining == 0 {
                    // Phase logically ended while the tab was closed. Anchor
                    // completion to when the timer would have actually hit
                    // zero, not now — keeps history accurate.
                    let synthetic = a.session.started_at_ms
                        + a.session.duration_secs as i64 * 1000
                        + a.closed_paused_ms;
                    active.set_value(Some(a));
                    finalize_completion(storage, active, sigs, synthetic).await;
                } else {
                    let was_running = !a.is_paused();
                    set_seconds_left.set(remaining);
                    active.set_value(Some(a));
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
            // Pause: open a new PauseRecord.
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
            });
        } else {
            // Start a new session, or close out the open pause and resume.
            let ver = run_version.get_untracked() + 1;
            set_run_version.set(ver);
            set_is_running.set(true);
            spawn_local(async move {
                let now = now_ms();
                let next = match active.get_value() {
                    Some(mut a) => {
                        if let Some((pause_id, paused_at)) = a.open_pause.take() {
                            if let Some(s) = storage.get_value() {
                                if let Err(e) = s.end_pause(pause_id, now).await {
                                    log_err("end_pause failed", e);
                                }
                            }
                            a.closed_paused_ms += (now - paused_at).max(0);
                        }
                        Some(a)
                    }
                    None => {
                        let p = phase.get_untracked();
                        let rec = SessionRecord::new(p.into(), now, p.duration_secs());
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
                        Some(ActiveSession::fresh(id, rec))
                    }
                };
                active.set_value(next);
                run_tick_loop(storage, active, sigs, ver).await;
            });
        }
    };

    let reset = move |_| {
        set_is_running.set(false);
        set_run_version.update(|v| *v += 1);
        set_seconds_left.set(phase.get_untracked().duration_secs());
        spawn_local(async move {
            if let Some(a) = active.get_value() {
                if let Some(s) = storage.get_value() {
                    if let Err(e) = s.delete_session(a.session_id).await {
                        log_err("delete on reset failed", e);
                    }
                }
                active.set_value(None);
            }
        });
    };

    let display = move || {
        let s = seconds_left.get();
        format!("{:02}:{:02}", s / 60, s % 60)
    };

    view! {
        <div class="container">
            <h1 class="phase">{move || phase.get().label()}</h1>
            <div class="clock">{display}</div>
            <div class="controls">
                <button
                    class:primary=move || !is_running.get()
                    on:click=toggle
                >
                    {move || if is_running.get() { "Pause" } else { "Start" }}
                </button>
                <button on:click=reset>"Reset"</button>
            </div>
            <p class="completed">"Completed: " {completed}</p>
        </div>
    }
}
