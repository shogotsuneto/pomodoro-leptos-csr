mod app;
mod settings_panel;
mod storage;
mod timer;
mod util;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
