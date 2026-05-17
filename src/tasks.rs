// Tasks UI: main-screen `TaskPicker` (chip + popover) and the
// management `TasksPanel` (drawer). Both operate on a `Vec<TaskRow>`
// signal held by `App`; persistence is delegated to `IndexedDbStorage`.

use std::rc::Rc;

use leptos::callback::Callback;
use leptos::html::Input;
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

/// In-memory view of a Task with its mutable fields as fine-grained signals.
/// Renaming or archiving an item updates only the affected signal, so the
/// minimal set of dependent nodes re-renders — no list re-scan needed.
/// `created_at_ms` lives on the persisted `Task` but isn't surfaced in UI,
/// so we drop it from the row.
#[derive(Clone, Copy)]
pub struct TaskRow {
    pub id: u64,
    pub name: RwSignal<String>,
    pub archived: RwSignal<bool>,
}

impl TaskRow {
    pub fn from_loaded(id: u64, task: Task) -> Self {
        Self {
            id,
            name: RwSignal::new(task.name),
            archived: RwSignal::new(task.archived),
        }
    }
}

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
    tasks: ReadSignal<Vec<TaskRow>>,
    set_tasks: WriteSignal<Vec<TaskRow>>,
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
                    let row = TaskRow::from_loaded(id, task);
                    set_tasks.update(|v| v.push(row));
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
            // Tracks both `tasks` (membership) and the row's `name` signal
            // (in-place rename), so the chip always shows the current label.
            Some(id) => tasks
                .get()
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.name.get())
                .unwrap_or_else(|| "(unknown task)".to_string()),
        }
    };

    let active_tasks = move || {
        tasks
            .get()
            .into_iter()
            .filter(|r| !r.archived.get())
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
                            key=|r| r.id
                            children=move |row| {
                                let id = row.id;
                                let name = row.name;
                                view! {
                                    <li
                                        class="task-item"
                                        class:selected=move || settings.get().selected_task_id == Some(id)
                                        on:click=move |_| select_task(Some(id))
                                    >
                                        {move || name.get()}
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
    tasks: ReadSignal<Vec<TaskRow>>,
    set_tasks: WriteSignal<Vec<TaskRow>>,
    storage: StorageRef,
) -> impl IntoView {
    // Only one row is editable at a time.
    let (editing_id, set_editing_id) = signal::<Option<u64>>(None);
    let (edit_name, set_edit_name) = signal(String::new());
    let (new_name, set_new_name) = signal(String::new());

    // Shared across all rows — only one rename input is mounted at a time
    // (whichever row's id matches `editing_id`), so the ref always points
    // at the live one.
    let input_ref: NodeRef<Input> = NodeRef::new();

    // Auto-focus + select-all when the rename input mounts. Clicking Edit
    // should feel like immediate text entry, and an unfocused input would
    // also bypass `on:blur=cancel_edit` — leaving the row stuck in edit
    // mode if the drawer is closed without ever touching the field.
    Effect::new(move |_| {
        if let Some(el) = input_ref.get() {
            let _ = el.focus();
            el.select();
        }
    });

    // Defensive: closing the drawer always cancels in-progress edits, even
    // when blur didn't fire (focus moved elsewhere first, or the input
    // somehow never had focus).
    Effect::new(move |_| {
        if !is_open.get() && editing_id.get_untracked().is_some() {
            set_editing_id.set(None);
        }
    });

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
        if let Some(row) = tasks.get_untracked().iter().find(|r| r.id == id) {
            row.name.set(name.clone());
        }
        spawn_local(async move {
            let Some(s) = storage.get_value() else { return };
            if let Err(e) = s.rename_task(id, &name).await {
                log_err("rename_task failed", e);
            }
        });
    };

    let toggle_archive = move |id: u64| {
        let Some(row) = tasks.get_untracked().iter().find(|r| r.id == id).copied() else {
            return;
        };
        let next_state = !row.archived.get_untracked();
        row.archived.set(next_state);
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
                Ok(id) => {
                    let row = TaskRow::from_loaded(id, task);
                    set_tasks.update(|v| v.push(row));
                }
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
                        key=|r| r.id
                        children=move |row| {
                            let id = row.id;
                            let name = row.name;
                            let archived = row.archived;
                            let editing = move || editing_id.get() == Some(id);
                            view! {
                                <li
                                    class="task-manage-item"
                                    class:archived=move || archived.get()
                                >
                                    <Show
                                        when=editing
                                        fallback=move || view! {
                                            <span class="task-name">{move || name.get()}</span>
                                        }
                                    >
                                        <input
                                            class="task-edit-input"
                                            type="text"
                                            node_ref=input_ref
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
                                            // Blur-cancels (clicking the close
                                            // button, backdrop, or anywhere
                                            // else fires blur before our
                                            // on_close runs, so committing
                                            // here would save the in-progress
                                            // rename against the user's
                                            // intent). Enter is the only
                                            // explicit commit.
                                            on:blur=move |_| cancel_edit()
                                        />
                                    </Show>
                                    <div class="task-actions">
                                        <Show when=move || !editing()>
                                            <button on:click=move |_| begin_edit(id, name.get_untracked())>
                                                "Edit"
                                            </button>
                                            <button on:click=move |_| toggle_archive(id)>
                                                {move || if archived.get() { "Unarchive" } else { "Archive" }}
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
