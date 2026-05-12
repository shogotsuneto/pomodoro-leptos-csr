use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;
use web_sys::{AudioContext, OscillatorType};

use crate::timer::Phase;

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

    // Fade out just before stopping to avoid a click.
    gain.gain()
        .exponential_ramp_to_value_at_time(0.001, now + duration_ms / 1000.0)
        .ok();
    oscillator
        .stop_with_when(now + duration_ms / 1000.0)
        .ok();

    // Keep ctx alive until the sound finishes by leaking it into JS.
    let _ = JsValue::from(ctx);
}

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
                        // Phase complete: play a beep then advance.
                        beep(880.0, 600.0);
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
