// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod wayland;

fn main() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false);

    cosmic::app::run::<app::AppModel>(settings, ())
}
