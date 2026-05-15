// SPDX-License-Identifier: MPL-2.0

use cosmic::iced::advanced::layout::Limits;
use cosmic::iced::core::window::Id as SurfaceId;
use cosmic::iced::event::listen_raw;
use cosmic::iced::futures::SinkExt;
use cosmic::iced::keyboard::{Event as KeyEvent, Key, Modifiers, key::Named};
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
use std::sync::LazyLock;
use tokio::signal::unix::{SignalKind, signal};

static AUTOSIZE_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("cosmic-switch-less-autosize"));
const SCREEN_HEIGHT_FRACTION: f32 = 0.8;
/// Header + paddings + column spacing, subtracted from the screen-fraction so
/// the *total* overlay (not just the list) stays within the fraction.
const OVERLAY_CHROME_HEIGHT: f32 = 96.0;
/// Used until the first `ScreenHeight` event arrives.
const DEFAULT_MAX_LIST_HEIGHT: f32 = 600.0;

pub struct AppModel {
    core: cosmic::Core,
    windows: Vec<crate::wayland::WindowInfo>,
    window_id: SurfaceId,
    scrollable_id: cosmic::widget::Id,
    desktop_entries: Vec<DesktopEntry>,
    shown: bool,
    highlighted_index: usize,
    wayland: Option<crate::wayland::WaylandHandle>,
    max_list_height: f32,
    overlay_width: f32,
    list_icon_size: u16,
    list_font_size: Option<f32>,
    /// Modifiers held when the overlay first received keyboard focus
    /// after a summon. Release of any flag in this set commits.
    /// `Some(Modifiers::empty())` after a no-modifier summon → Enter only.
    summon_modifiers: Option<Modifiers>,
    /// Set in `summon_with_highlight`, cleared on the next
    /// `ModifiersChanged` (which captures the snapshot).
    awaiting_snapshot: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// One-time handle for sending commands back into the Wayland thread.
    WaylandReady(crate::wayland::WaylandHandle),
    WindowsUpdated(Vec<crate::wayland::WindowInfo>),
    /// Logical height of the smallest connected monitor, in pixels.
    ScreenHeight(u32),
    /// Commit the selection: activate the highlighted window, then hide.
    Confirm,
    /// Dismiss the overlay without switching.
    Cancel,
    /// Forward summon-or-cycle. If hidden, summon with the previous window
    /// (MRU index 1) highlighted; if shown, advance the highlight by one.
    CycleNext,
    /// Backward summon-or-cycle. If hidden, summon with the least-recent
    /// window highlighted; if shown, move the highlight back by one.
    CyclePrev,
    /// Deferred scroll sync — runs one event-loop tick after a summon so the
    /// scrollable widget has registered its Id with iced's runtime.
    SyncScroll,
    /// Live keyboard modifier state. The first event after a summon
    /// captures the snapshot; later events drive the release-to-commit
    /// check.
    ModifiersChanged(Modifiers),
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.riverfr0zen.cosmic-switch-less";

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

        let config = crate::config::Config::load(Self::APP_ID);

        let app = AppModel {
            core,
            windows: Vec::new(),
            window_id,
            scrollable_id: cosmic::widget::Id::unique(),
            desktop_entries,
            shown: false,
            highlighted_index: 0,
            wayland: None,
            max_list_height: DEFAULT_MAX_LIST_HEIGHT,
            overlay_width: config.overlay_width,
            list_icon_size: config.list_icon_size,
            list_font_size: config.list_font_size,
            summon_modifiers: None,
            awaiting_snapshot: false,
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
        let header = widget::text::title4("cosmic-switch-less");

        let mut list = widget::column::with_capacity(self.windows.len().max(1));
        if self.windows.is_empty() {
            list = list.push(widget::text("(none yet — waiting for Wayland events)"));
        } else {
            for (i, w) in self.windows.iter().enumerate() {
                list = list.push(self.window_row(w, space_s, i == self.highlighted_index));
            }
        }

        let list_section = widget::container(
            widget::scrollable(list.spacing(space_s)).id(self.scrollable_id.clone()),
        )
        .max_height(self.max_list_height);

        let content = widget::container(
            widget::column::with_capacity(2)
                .push(header)
                .push(list_section)
                .spacing(space_s)
                .width(Length::Fixed(self.overlay_width))
                .height(Length::Shrink),
        )
        .padding(space_s)
        .class(cosmic::theme::Container::custom(|theme| {
            // Reuse the standard background style, then add an outline matching
            // cosmic-comp's active window hint: `window_hint` colour if the
            // theme sets one, otherwise the accent colour.
            let cosmic = theme.cosmic();
            let mut style = cosmic::theme::Container::background(cosmic);
            let hint = cosmic.window_hint.map_or_else(
                || cosmic.accent_color(),
                cosmic::cosmic_theme::palette::Srgba::from,
            );
            #[allow(clippy::cast_precision_loss)]
            let width = cosmic.active_hint as f32;
            style.border.color = hint.into();
            style.border.width = width;
            style
        }));

        cosmic::widget::autosize::autosize(content, AUTOSIZE_ID.clone()).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            crate::wayland::subscribe().map(|event| match event {
                crate::wayland::WaylandEvent::Ready(handle) => Message::WaylandReady(handle),
                crate::wayland::WaylandEvent::Windows(windows) => {
                    Message::WindowsUpdated(windows)
                }
                crate::wayland::WaylandEvent::ScreenHeight(h) => Message::ScreenHeight(h),
            }),
            signals_subscription(),
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
                Event::Keyboard(KeyEvent::KeyPressed {
                    key: Key::Named(Named::Enter),
                    ..
                }) => Some(Message::Confirm),
                Event::Keyboard(KeyEvent::KeyPressed {
                    key: Key::Named(Named::Escape),
                    ..
                }) => Some(Message::Cancel),
                Event::Keyboard(KeyEvent::ModifiersChanged(mods)) => {
                    Some(Message::ModifiersChanged(mods))
                }
                _ => None,
            }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::WaylandReady(handle) => {
                self.wayland = Some(handle);
            }
            Message::WindowsUpdated(windows) => {
                self.windows = windows;
                self.highlighted_index = self
                    .highlighted_index
                    .min(self.windows.len().saturating_sub(1));
            }
            #[allow(clippy::cast_precision_loss)]
            Message::ScreenHeight(h) => {
                self.max_list_height =
                    (h as f32 * SCREEN_HEIGHT_FRACTION - OVERLAY_CHROME_HEIGHT).max(120.0);
            }
            Message::Confirm => {
                if self.shown {
                    self.activate_highlighted();
                    self.shown = false;
                    self.summon_modifiers = None;
                    self.awaiting_snapshot = false;
                    return destroy_layer_surface(self.window_id);
                }
            }
            Message::Cancel => {
                if self.shown {
                    self.shown = false;
                    self.summon_modifiers = None;
                    self.awaiting_snapshot = false;
                    return destroy_layer_surface(self.window_id);
                }
            }
            Message::ModifiersChanged(mods) => {
                if self.awaiting_snapshot {
                    // Wayland guarantees a wl_keyboard.modifiers event right
                    // after wl_keyboard.enter, so this fires once focus lands.
                    self.summon_modifiers = Some(mods);
                    self.awaiting_snapshot = false;
                } else if let Some(snap) = self.summon_modifiers
                    && !snap.is_empty()
                    && ((snap.shift() && !mods.shift())
                        || (snap.alt() && !mods.alt())
                        || (snap.control() && !mods.control())
                        || (snap.logo() && !mods.logo()))
                {
                    return self.update(Message::Confirm);
                }
            }
            Message::SyncScroll => return self.scroll_to_highlighted(),
            Message::CycleNext => {
                if !self.shown {
                    return self.summon_with_highlight(1, false);
                }
                if !self.windows.is_empty() {
                    self.highlighted_index = (self.highlighted_index + 1) % self.windows.len();
                    return self.scroll_to_highlighted();
                }
            }
            Message::CyclePrev => {
                if !self.shown {
                    return self.summon_with_highlight(self.windows.len().saturating_sub(1), true);
                }
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

fn signals_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            4,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                let mut sig1 = signal(SignalKind::user_defined1())
                    .expect("install SIGUSR1 handler");
                let mut sig2 = signal(SignalKind::user_defined2())
                    .expect("install SIGUSR2 handler");
                loop {
                    tokio::select! {
                        Some(()) = sig1.recv() => {
                            let _ = output.send(Message::CycleNext).await;
                        }
                        Some(()) = sig2.recv() => {
                            let _ = output.send(Message::CyclePrev).await;
                        }
                        else => break,
                    }
                }
            },
        )
    })
}

impl AppModel {
    fn summon_with_highlight(
        &mut self,
        idx: usize,
        scroll_on_summon: bool,
    ) -> Task<cosmic::Action<Message>> {
        self.shown = true;
        self.awaiting_snapshot = true;
        self.highlighted_index = idx.min(self.windows.len().saturating_sub(1));
        let surface = get_layer_surface(SctkLayerSurfaceSettings {
            id: self.window_id,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::empty(),
            namespace: "cosmic-switch-less".into(),
            size: None,
            size_limits: Limits::NONE
                .min_width(1.0)
                .min_height(1.0)
                .max_width(self.overlay_width),
            exclusive_zone: -1,
            ..Default::default()
        });
        if !scroll_on_summon {
            return surface;
        }
        // The scrollable widget doesn't register its Id with iced's runtime
        // until after the first view() of the new surface, so a synchronous
        // snap_to issued here would target nothing. Schedule a SyncScroll
        // message via a brief async wait so it lands in the next event-loop
        // iteration, after view() has run.
        let deferred = cosmic::iced::Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            },
            |()| cosmic::Action::App(Message::SyncScroll),
        );
        Task::batch([surface, deferred])
    }

    fn activate_highlighted(&self) {
        if let (Some(handle), Some(window)) =
            (self.wayland.as_ref(), self.windows.get(self.highlighted_index))
        {
            handle.activate(window.identifier.clone());
        }
    }

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
                .size(self.list_icon_size)
                .into()
        } else {
            icon::from_name(icon_str).size(self.list_icon_size).into()
        };

        let mut text_widget = widget::text(format!("{} — {}", w.title, w.app_id));
        if let Some(list_font_size) = self.list_font_size {
            text_widget = text_widget.size(list_font_size);
        }
        let row = widget::row::with_capacity(2)
            .push(icon_widget)
            .push(text_widget)
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
