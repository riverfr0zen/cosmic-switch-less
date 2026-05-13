// SPDX-License-Identifier: MPL-2.0

use cosmic::iced::advanced::layout::Limits;
use cosmic::iced::core::window::Id as SurfaceId;
use cosmic::iced::event::listen_raw;
use cosmic::iced::futures::SinkExt;
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
use tokio::signal::unix::{SignalKind, signal};

pub struct AppModel {
    core: cosmic::Core,
    windows: Vec<crate::wayland::WindowInfo>,
    window_id: SurfaceId,
    scrollable_id: cosmic::widget::Id,
    desktop_entries: Vec<DesktopEntry>,
    shown: bool,
    highlighted_index: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    WindowsUpdated(Vec<crate::wayland::WindowInfo>),
    Show,
    Hide,
    CycleNext,
    CyclePrev,
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
            scrollable_id: cosmic::widget::Id::unique(),
            desktop_entries,
            shown: false,
            highlighted_index: 0,
        };

        (app, Task::none())
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
            for (i, w) in self.windows.iter().enumerate() {
                list = list.push(self.window_row(w, space_s, i == self.highlighted_index));
            }
        }

        widget::container(
            widget::column::with_capacity(2)
                .push(header)
                .push(
                    widget::scrollable(list.spacing(space_s))
                        .id(self.scrollable_id.clone())
                        .height(Length::Fill),
                )
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
            sigusr1_subscription(),
            listen_raw(|event, _status, _id| match event {
                Event::Keyboard(KeyEvent::KeyPressed {
                    key: Key::Named(Named::Tab),
                    modifiers,
                    ..
                }) => Some(if modifiers.shift() {
                    Message::CyclePrev
                } else {
                    Message::CycleNext
                }),
                Event::Keyboard(
                    KeyEvent::KeyPressed {
                        key: Key::Named(Named::Escape),
                        ..
                    }
                    | KeyEvent::KeyReleased {
                        key: Key::Named(Named::Alt),
                        ..
                    },
                ) => Some(Message::Hide),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::WindowsUpdated(windows) => {
                self.windows = windows;
                self.highlighted_index = self
                    .highlighted_index
                    .min(self.windows.len().saturating_sub(1));
            }
            Message::Show => {
                if !self.shown {
                    self.shown = true;
                    self.highlighted_index = 0;
                    return get_layer_surface(SctkLayerSurfaceSettings {
                        id: self.window_id,
                        keyboard_interactivity: KeyboardInteractivity::Exclusive,
                        anchor: Anchor::empty(),
                        namespace: "cosmic-app-switcher".into(),
                        size: Some((Some(600), Some(400))),
                        size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                        exclusive_zone: -1,
                        ..Default::default()
                    });
                }
            }
            Message::Hide => {
                if self.shown {
                    self.shown = false;
                    return destroy_layer_surface(self.window_id);
                }
            }
            Message::CycleNext => {
                if !self.windows.is_empty() {
                    self.highlighted_index = (self.highlighted_index + 1) % self.windows.len();
                    return self.scroll_to_highlighted();
                }
            }
            Message::CyclePrev => {
                if !self.windows.is_empty() {
                    let len = self.windows.len();
                    self.highlighted_index = (self.highlighted_index + len - 1) % len;
                    return self.scroll_to_highlighted();
                }
            }
        }
        Task::none()
    }
}

fn sigusr1_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            4,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                let mut sig = signal(SignalKind::user_defined1())
                    .expect("install SIGUSR1 handler");
                while sig.recv().await.is_some() {
                    let _ = output.send(Message::Show).await;
                }
            },
        )
    })
}

impl AppModel {
    fn scroll_to_highlighted(&self) -> Task<cosmic::Action<Message>> {
        if self.windows.len() <= 1 {
            return Task::none();
        }
        // Relative offset: 0.0 puts the top of the content at the top of the
        // viewport, 1.0 puts the bottom of the content at the bottom. Mapping
        // index → fraction this way keeps the highlighted row in view at both
        // ends and roughly centred in the middle of the list.
        #[allow(clippy::cast_precision_loss)]
        let fraction = self.highlighted_index as f32 / (self.windows.len() - 1) as f32;
        cosmic::iced::widget::scrollable::snap_to(
            self.scrollable_id.clone(),
            cosmic::iced::widget::scrollable::RelativeOffset {
                x: Some(0.0),
                y: Some(fraction),
            },
        )
    }

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
        highlighted: bool,
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

        let row = widget::row::with_capacity(2)
            .push(icon_widget)
            .push(widget::text(format!("{} — {}", w.title, w.app_id)))
            .spacing(spacing)
            .align_y(Alignment::Center);

        let mut container = widget::container(row).width(Length::Fill);
        if highlighted {
            container = container.class(cosmic::theme::Container::custom(|theme| {
                let cosmic = theme.cosmic();
                cosmic::widget::container::Style {
                    icon_color: Some(cosmic.accent.on.into()),
                    text_color: Some(cosmic.accent.on.into()),
                    background: Some(cosmic::iced::Background::Color(
                        cosmic.accent.base.into(),
                    )),
                    border: cosmic::iced::Border {
                        radius: cosmic.corner_radii.radius_xs.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }));
        }
        container.into()
    }
}
