// SPDX-License-Identifier: MPL-2.0

use cosmic::iced::advanced::layout::Limits;
use cosmic::iced::core::window::Id as SurfaceId;
use cosmic::iced::event::listen_raw;
use cosmic::iced::keyboard::{Event as KeyEvent, Key, key::Named};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced::{Alignment, Event, Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, icon};
use freedesktop_desktop_entry as fde;
use freedesktop_desktop_entry::DesktopEntry;
use std::path::PathBuf;

pub struct AppModel {
    core: cosmic::Core,
    windows: Vec<crate::wayland::WindowInfo>,
    window_id: SurfaceId,
    desktop_entries: Vec<DesktopEntry>,
}

#[derive(Debug, Clone)]
pub enum Message {
    WindowsUpdated(Vec<crate::wayland::WindowInfo>),
    Dismiss,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.riverfr0zen.cosmic-app-switcher";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let window_id = SurfaceId::unique();

        let locales = fde::get_languages_from_env();
        let desktop_entries = fde::Iter::new(fde::default_paths())
            .filter_map(|path| DesktopEntry::from_path(path, Some(&locales)).ok())
            .collect::<Vec<_>>();

        let app = AppModel {
            core,
            windows: Vec::new(),
            window_id,
            desktop_entries,
        };

        let task = get_layer_surface(SctkLayerSurfaceSettings {
            id: window_id,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::empty(),
            namespace: "cosmic-app-switcher".into(),
            size: Some((Some(600), Some(400))),
            size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
            exclusive_zone: -1,
            ..Default::default()
        });

        (app, task)
    }

    // Required by the trait but never rendered: the app runs as a
    // layer-shell surface (`no_main_window(true)`), so libcosmic only ever
    // calls `view_window`.
    fn view(&self) -> Element<'_, Self::Message> {
        widget::space().height(Length::Fixed(1.0)).into()
    }

    fn view_window(&self, id: SurfaceId) -> Element<'_, Self::Message> {
        if id != self.window_id {
            return widget::space().height(Length::Fixed(1.0)).into();
        }

        let space_s = cosmic::theme::spacing().space_s;
        let header = widget::text::title3("Open windows on this workspace");

        let mut list = widget::column::with_capacity(self.windows.len().max(1));
        if self.windows.is_empty() {
            list = list.push(widget::text("(none yet — waiting for Wayland events)"));
        } else {
            for w in &self.windows {
                list = list.push(self.window_row(w, space_s));
            }
        }

        widget::container(
            widget::column::with_capacity(2)
                .push(header)
                .push(list.spacing(space_s))
                .spacing(space_s),
        )
        .padding(space_s)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::Background)
        .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            crate::wayland::subscribe().map(Message::WindowsUpdated),
            listen_raw(|event, _status, _id| match event {
                Event::Keyboard(KeyEvent::KeyPressed {
                    key: Key::Named(Named::Escape),
                    ..
                }) => Some(Message::Dismiss),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::WindowsUpdated(windows) => {
                self.windows = windows;
            }
            Message::Dismiss => {
                return Task::batch([
                    destroy_layer_surface(self.window_id),
                    cosmic::iced::exit(),
                ]);
            }
        }
        Task::none()
    }
}

impl AppModel {
    /// Resolves an `app_id` to a freedesktop icon name, mirroring
    /// pop-launcher's `cosmic_toplevel` plugin (the lookup behind the
    /// shipping COSMIC alt-tab).
    fn icon_name_for(&self, app_id: &str) -> String {
        let key = fde::unicase::Ascii::new(app_id);
        let entry = fde::find_app_by_id(&self.desktop_entries, key)
            .cloned()
            .unwrap_or_else(|| DesktopEntry::from_appid(app_id.to_string()));
        entry
            .icon()
            .map_or_else(|| "application-x-executable".to_owned(), str::to_owned)
    }

    fn window_row<'a>(
        &self,
        w: &'a crate::wayland::WindowInfo,
        spacing: u16,
    ) -> Element<'a, Message> {
        // .desktop entries sometimes set `Icon=` to an absolute path rather
        // than a freedesktop theme name. `icon::from_name` only resolves
        // names, so mirror cosmic-launcher and branch on `/` to pick the
        // right loader.
        let icon_str = self.icon_name_for(&w.app_id);
        let icon_widget: Element<'_, Message> = if icon_str.contains('/') {
            icon::icon(icon::from_path(PathBuf::from(icon_str)))
                .size(24)
                .into()
        } else {
            icon::from_name(icon_str).size(24).into()
        };
        widget::row::with_capacity(2)
            .push(icon_widget)
            .push(widget::text(format!("{} — {}", w.title, w.app_id)))
            .spacing(spacing)
            .align_y(Alignment::Center)
            .into()
    }
}
