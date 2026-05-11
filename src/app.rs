use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::timer::Phase;

#[component]
pub fn App() -> impl IntoView {
    let (seconds_left, set_seconds_left) = signal(Phase::Work.duration_secs());
    let (is_running, set_is_running) = signal(false);
    let (phase, set_phase) = signal(Phase::Work);
    let (completed, set_completed) = signal(0u32);
    // Incrementing this cancels the running async tick loop.
    let (run_version, set_run_version) = signal(0u32);

    let display = move || {
        let s = seconds_left.get();
        format!("{:02}:{:02}", s / 60, s % 60)
    };

    let toggle = move |_| {
        if is_running.get_untracked() {
            set_is_running.set(false);
            set_run_version.update(|v| *v += 1);
        } else {
            // Capture the new version before spawning so the loop can self-cancel.
            let ver = run_version.get_untracked() + 1;
            set_run_version.set(ver);
            set_is_running.set(true);

            spawn_local(async move {
                loop {
                    TimeoutFuture::new(1_000).await;

                    // Another start/pause press issued a new version — stop.
                    if run_version.get_untracked() != ver {
                        break;
                    }

                    if seconds_left.get_untracked() == 0 {
                        // Phase complete: advance and wait for user to start next phase.
                        let current = phase.get_untracked();
                        if current == Phase::Work {
                            set_completed.update(|c| *c += 1);
                        }
                        let next = current.next();
                        set_phase.set(next);
                        set_seconds_left.set(next.duration_secs());
                        set_is_running.set(false);
                        set_run_version.update(|v| *v += 1);
                        break;
                    } else {
                        set_seconds_left.update(|s| *s -= 1);
                    }
                }
            });
        }
    };

    let reset = move |_| {
        set_is_running.set(false);
        set_run_version.update(|v| *v += 1);
        set_seconds_left.set(phase.get_untracked().duration_secs());
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
