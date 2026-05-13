use wasm_bindgen::JsValue;
use web_sys::{AudioContext, OscillatorType};

pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

pub fn log_err(prefix: &str, e: impl std::fmt::Display) {
    web_sys::console::error_1(&format!("{prefix}: {e}").into());
}

/// Plays a short sine-wave beep via WebAudio. No-op when volume is 0.
pub fn beep(frequency: f32, duration_ms: f64, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    let Ok(ctx) = AudioContext::new() else { return };
    let Ok(oscillator) = ctx.create_oscillator() else { return };
    let Ok(gain) = ctx.create_gain() else { return };

    oscillator.set_type(OscillatorType::Sine);
    oscillator.frequency().set_value(frequency);
    gain.gain().set_value(volume);

    let _ = oscillator.connect_with_audio_node(&gain);
    let _ = gain.connect_with_audio_node(&ctx.destination());

    let now = ctx.current_time();
    oscillator.start().ok();
    gain.gain()
        .exponential_ramp_to_value_at_time(0.001, now + duration_ms / 1000.0)
        .ok();
    oscillator.stop_with_when(now + duration_ms / 1000.0).ok();
    // Keep the context alive until the sound finishes by leaking into JS.
    let _ = JsValue::from(ctx);
}
