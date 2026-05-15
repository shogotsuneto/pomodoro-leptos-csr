// Tasks UI: main-screen `TaskPicker` (chip + popover) and the
// management `TasksPanel` (drawer). Both operate on the same `tasks`
// signal pair held by `App`; persistence is delegated to `IndexedDbStorage`.

use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{Event, HtmlInputElement, KeyboardEvent};

use crate::settings_panel::DrawerShell;
use crate::storage::indexeddb::IndexedDbStorage;
use crate::storage::{Settings, Task};
use crate::util::{log_err, now_ms};

type StorageRef = StoredValue<Option<Rc<IndexedDbStorage>>, LocalStorage>;

fn input_value(ev: &Event) -> Option<String> {
    ev.target()?
        .dyn_into::<HtmlInputElement>()
        .ok()
        .map(|el| el.value())
}

fn persist_settings(storage: StorageRef, set_settings: WriteSignal<Settings>, new: Settings) {
    set_settings.set(new.clone());
    spawn_local(async move {
        if let Some(s) = storage.get_value() {
            if let Err(e) = s.save_settings(&new).await {
                log_err("save settings failed", e);
            }
        }
    });
}

#[component]
pub fn TaskPicker(
    tasks: ReadSignal<Vec<(u64, Task)>>,
    set_tasks: WriteSignal<Vec<(u64, Task)>>,
    settings: ReadSignal<Settings>,
    set_settings: WriteSignal<Settings>,
    storage: StorageRef,
    /// Whether a session is currently in flight (running OR paused).
    has_active_session: Signal<bool>,
    /// The `task_id` attached to the currently-active session, if any. Used
    /// to detect a mid-session task switch and surface the "applies from next
    /// session" hint.
    active_task_id: Signal<Option<u64>>,
    /// Called when the user picks "Manage tasks…" — host closes this popover
    /// implicitly and opens the management drawer.
    on_open_manage: Callback<()>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let (new_name, set_new_name) = signal(String::new());

    let select_task = move |id: Option<u64>| {
        let mut s = settings.get_untracked();
        s.selected_task_id = id;
        persist_settings(storage, set_settings, s);
        set_is_open.set(false);
    };

    let create_and_select = move || {
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        let task = Task {
            name,
            created_at_ms: now_ms(),
            archived: false,
        };
        set_new_name.set(String::new());
        spawn_local(async move {
            let Some(s) = storage.get_value() else { return };
            match s.create_task(&task).await {
                Ok(id) => {
                    set_tasks.update(|v| v.push((id, task)));
                    let mut next = settings.get_untracked();
                    next.selected_task_id = Some(id);
                    persist_settings(storage, set_settings, next);
                    set_is_open.set(false);
                }
                Err(e) => log_err("create_task failed", e),
            }
        });
    };

    let selected_name = move || {
        let sid = settings.get().selected_task_id;
        match sid {
            None => "No task".to_string(),
            Some(id) => tasks
                .get()
                .iter()
                .find(|(tid, _)| *tid == id)
                .map(|(_, t)| t.name.clone())
                .unwrap_or_else(|| "(unknown task)".to_string()),
        }
    };

    let active_tasks = move || {
        tasks
            .get()
            .into_iter()
            .filter(|(_, t)| !t.archived)
            .collect::<Vec<_>>()
    };

    let show_hint = move || {
        has_active_session.get() && active_task_id.get() != settings.get().selected_task_id
    };

    view! {
        <div class="task-picker">
            <button
                class="task-chip"
                class:has-task=move || settings.get().selected_task_id.is_some()
                on:click=move |_| set_is_open.update(|v| *v = !*v)
                aria-label="Current task"
            >
                <span class="task-chip-label">{selected_name}</span>
                <span class="task-chip-caret">"▾"</span>
            </button>
            <Show when=move || is_open.get()>
                <div
                    class="task-popover-backdrop"
                    on:click=move |_| set_is_open.set(false)
                ></div>
                <div class="task-popover" on:click=move |ev| ev.stop_propagation()>
                    <ul class="task-list">
                        <li
                            class="task-item none"
                            class:selected=move || settings.get().selected_task_id.is_none()
                            on:click=move |_| select_task(None)
                        >
                            "(no task)"
                        </li>
                        <For
                            each=active_tasks
                            key=|(id, _)| *id
                            children=move |(id, task)| {
                                let label = task.name.clone();
                                view! {
                                    <li
                                        class="task-item"
                                        class:selected=move || settings.get().selected_task_id == Some(id)
                                        on:click=move |_| select_task(Some(id))
                                    >
                                        {label}
                                    </li>
                                }
                            }
                        />
                    </ul>
                    <div class="task-new">
                        <input
                            type="text"
                            placeholder="New task"
                            prop:value=move || new_name.get()
                            on:input=move |ev: Event| {
                                if let Some(v) = input_value(&ev) {
                                    set_new_name.set(v);
                                }
                            }
                            on:keydown=move |ev: KeyboardEvent| {
                                if ev.key() == "Enter" {
                                    create_and_select();
                                }
                            }
                        />
                        <button
                            class="task-new-btn"
                            on:click=move |_| create_and_select()
                            aria-label="Add task"
                        >"+"</button>
                    </div>
                    <button
                        class="task-manage"
                        on:click=move |_| {
                            set_is_open.set(false);
                            on_open_manage.run(());
                        }
                    >"Manage tasks…"</button>
                </div>
            </Show>
            <Show when=show_hint>
                <p class="task-hint">"Switch will apply from the next session"</p>
            </Show>
        </div>
    }
}

#[component]
pub fn TasksPanel(
    is_open: Signal<bool>,
    on_close: Callback<()>,
    tasks: ReadSignal<Vec<(u64, Task)>>,
    set_tasks: WriteSignal<Vec<(u64, Task)>>,
    storage: StorageRef,
) -> impl IntoView {
    // Only one row is editable at a time.
    let (editing_id, set_editing_id) = signal::<Option<u64>>(None);
    let (edit_name, set_edit_name) = signal(String::new());
    let (new_name, set_new_name) = signal(String::new());

    let begin_edit = move |id: u64, current: String| {
        set_edit_name.set(current);
        set_editing_id.set(Some(id));
    };

    let cancel_edit = move || set_editing_id.set(None);

    let commit_edit = move || {
        let Some(id) = editing_id.get_untracked() else {
            return;
        };
        let name = edit_name.get_untracked().trim().to_string();
        set_editing_id.set(None);
        if name.is_empty() {
            return;
        }
        set_tasks.update(|v| {
            if let Some((_, t)) = v.iter_mut().find(|(tid, _)| *tid == id) {
                t.name = name.clone();
            }
        });
        spawn_local(async move {
            let Some(s) = storage.get_value() else { return };
            if let Err(e) = s.rename_task(id, &name).await {
                log_err("rename_task failed", e);
            }
        });
    };

    let toggle_archive = move |id: u64| {
        let mut next_state = false;
        set_tasks.update(|v| {
            if let Some((_, t)) = v.iter_mut().find(|(tid, _)| *tid == id) {
                t.archived = !t.archived;
                next_state = t.archived;
            }
        });
        spawn_local(async move {
            let Some(s) = storage.get_value() else { return };
            if let Err(e) = s.set_task_archived(id, next_state).await {
                log_err("set_task_archived failed", e);
            }
        });
    };

    let create_new = move || {
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        let task = Task {
            name,
            created_at_ms: now_ms(),
            archived: false,
        };
        set_new_name.set(String::new());
        spawn_local(async move {
            let Some(s) = storage.get_value() else { return };
            match s.create_task(&task).await {
                Ok(id) => set_tasks.update(|v| v.push((id, task))),
                Err(e) => log_err("create_task failed", e),
            }
        });
    };

    view! {
        <DrawerShell is_open=is_open on_close=on_close title="Tasks">
            <div class="task-new">
                <input
                    type="text"
                    placeholder="New task"
                    prop:value=move || new_name.get()
                    on:input=move |ev: Event| {
                        if let Some(v) = input_value(&ev) {
                            set_new_name.set(v);
                        }
                    }
                    on:keydown=move |ev: KeyboardEvent| {
                        if ev.key() == "Enter" {
                            create_new();
                        }
                    }
                />
                <button
                    class="task-new-btn"
                    on:click=move |_| create_new()
                    aria-label="Add task"
                >"+"</button>
            </div>
            <Show
                when=move || !tasks.get().is_empty()
                fallback=|| view! { <p class="task-empty">"No tasks yet."</p> }
            >
                <ul class="task-manage-list">
                    <For
                        each=move || tasks.get()
                        key=|(id, _)| *id
                        children=move |(id, _)| {
                            // `<For>` keys by id, so an in-place rename keeps
                            // the same view alive. Read name/archived from the
                            // tasks signal each render so edits land here, not
                            // just on screens that re-derive from `tasks` (the
                            // chip).
                            let name = move || {
                                tasks
                                    .get()
                                    .iter()
                                    .find(|(tid, _)| *tid == id)
                                    .map(|(_, t)| t.name.clone())
                                    .unwrap_or_default()
                            };
                            let archived = move || {
                                tasks
                                    .get()
                                    .iter()
                                    .find(|(tid, _)| *tid == id)
                                    .map(|(_, t)| t.archived)
                                    .unwrap_or(false)
                            };
                            let editing = move || editing_id.get() == Some(id);

                            view! {
                                <li class="task-manage-item" class:archived=archived>
                                    <Show
                                        when=editing
                                        fallback=move || view! {
                                            <span class="task-name">{move || name()}</span>
                                        }
                                    >
                                        <input
                                            class="task-edit-input"
                                            type="text"
                                            prop:value=move || edit_name.get()
                                            on:input=move |ev: Event| {
                                                if let Some(v) = input_value(&ev) {
                                                    set_edit_name.set(v);
                                                }
                                            }
                                            on:keydown=move |ev: KeyboardEvent| {
                                                if ev.key() == "Enter" {
                                                    commit_edit();
                                                } else if ev.key() == "Escape" {
                                                    cancel_edit();
                                                }
                                            }
                                            on:blur=move |_| commit_edit()
                                        />
                                    </Show>
                                    <div class="task-actions">
                                        <Show when=move || !editing()>
                                            <button on:click=move |_| begin_edit(id, name())>
                                                "Edit"
                                            </button>
                                            <button on:click=move |_| toggle_archive(id)>
                                                {move || if archived() { "Unarchive" } else { "Archive" }}
                                            </button>
                                        </Show>
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
