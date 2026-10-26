//! Windows App for Linux.

fn main() -> gtk4::glib::ExitCode {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "warn"
    };
    env_logger::init_from_env(env_logger::Env::default().default_filter_or(default_level));
    windows_app_linux::frontend::gtk::run()
}
