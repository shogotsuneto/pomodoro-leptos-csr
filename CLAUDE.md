# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Pomodoro timer built as a client-side Rust/WebAssembly app using [Leptos](https://leptos.dev/) (CSR mode) and bundled with [Trunk](https://trunkrs.dev/). All CSS lives inline in `index.html`. Deployed to GitHub Pages on push to `main`.

## Commands

```bash
# Serve locally with hot reload
trunk serve

# Production build (outputs to dist/)
trunk build --release

# Format (uses leptosfmt via rust-analyzer.toml override)
leptosfmt src/**/*.rs

# Type-check without building WASM
cargo check --target wasm32-unknown-unknown
```

There are no tests in this project.

## Architecture

The app has one reactive root (`App` in `src/app.rs`) that owns all signals and spawns async tasks via `spawn_local`. There is no router or server.

**Timer loop** — `run_tick_loop` is a `spawn_local` future that polls `now_ms()` every second and writes to `seconds_left`. Cancellation is done by incrementing `run_version`: the loop checks its captured `ver` against the current signal each tick and exits if they diverge. This avoids any explicit task handles or cancellation tokens.

**Storage** (`src/storage/`) — Two-layer design:
- `mod.rs`: backend-agnostic domain types (`SessionRecord`, `PauseRecord`, `ActiveSession`, `Settings`) and `StorageError`.
- `indexeddb.rs`: the only backend, using the `idb` crate. Three object stores: `sessions` (auto-increment), `pauses` (auto-increment, indexed by `session_id`), `settings` (singleton at key `1.0`).

Sessions and pauses are separate stores so pause history doesn't bloat session records. `ActiveSession` is a read-side aggregate (not stored directly) built by `load_active` by joining the two stores.

**Settings persistence** — `Settings` uses `#[serde(default)]` at the struct level so new fields added later are backward-compatible with old IndexedDB records.

**Schema migrations** — `DB_VERSION` in `indexeddb.rs` is bumped to trigger `on_upgrade_needed`. The handler is idempotent (checks `store_names()` before creating), so existing data survives upgrades. For destructive changes, branch on `event.old_version()`.

**UI panels** — `DrawerShell` in `src/settings_panel.rs` is a reusable slide-in panel shell (backdrop + header + body slot). Adding a new drawer (history, stats, etc.) means: adding a variant to `DrawerKind` in `app.rs`, writing a `*Panel` component that wraps `DrawerShell`, and rendering it in `App`.

**Audio** — `beep()` in `src/util.rs` creates a fresh `AudioContext` per call and leaks it into JS via `JsValue::from(ctx)` to keep it alive until the oscillator stops. The fade-out uses an exponential ramp (not linear) to avoid audible clicks.
