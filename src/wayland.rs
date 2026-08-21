// SPDX-License-Identifier: MPL-2.0

use std::collections::{HashMap, HashSet};

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use cctk::sctk::output::{OutputHandler, OutputState};
use cctk::sctk::registry::{ProvidesRegistryState, RegistryState};
use cctk::sctk::seat::{Capability, SeatHandler, SeatState};
use cctk::toplevel_info::{ToplevelInfoHandler, ToplevelInfoState};
use cctk::toplevel_management::{ToplevelManagerHandler, ToplevelManagerState};
use cctk::wayland_client::globals::registry_queue_init;
use cctk::wayland_client::protocol::{wl_output, wl_seat};
use cctk::wayland_client::{Connection, QueueHandle, WEnum};
use cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1;
use cctk::wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1;
use cctk::workspace::{WorkspaceHandler, WorkspaceState};
use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1;
use cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub title: String,
    pub app_id: String,
    pub identifier: String,
}

/// A command sent from the app thread back into the Wayland listener thread.
pub enum WaylandRequest {
    Activate(String),
}

/// App-side handle for sending [`WaylandRequest`]s into the listener thread.
/// The manual `Debug` impl exists because `calloop::channel::Sender` is not
/// `Debug`, but the iced `Message` enum that carries this handle derives it.
#[derive(Clone)]
pub struct WaylandHandle(calloop::channel::Sender<WaylandRequest>);

impl std::fmt::Debug for WaylandHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WaylandHandle")
    }
}

impl WaylandHandle {
    pub fn activate(&self, identifier: String) {
        let _ = self.0.send(WaylandRequest::Activate(identifier));
    }
}

/// Items delivered by the Wayland subscription: a one-time `Ready` carrying the
/// command handle, followed by `Windows` and `ScreenHeight` updates.
#[derive(Clone, Debug)]
pub enum WaylandEvent {
    Ready(WaylandHandle),
    Windows(Vec<WindowInfo>),
    /// Logical height of the smallest connected monitor, in pixels.
    ScreenHeight(u32),
}

/// Messages the listener thread sends over the internal channel to the
/// subscription bridge.
enum ThreadEvent {
    Windows(Vec<WindowInfo>),
    ScreenHeight(u32),
}

struct WaylandState {
    output_state: OutputState,
    registry_state: RegistryState,
    toplevel_info_state: ToplevelInfoState,
    toplevel_manager_state: ToplevelManagerState,
    seat_state: SeatState,
    workspace_state: WorkspaceState,
    sender: mpsc::Sender<ThreadEvent>,
    mru_order: Vec<String>,
    /// Last seen `Activated` value per identifier. cosmic-comp delivers
    /// events for several toplevels in one batch, and a toplevel whose state
    /// event hasn't been processed yet still reports its *stale* cached
    /// state (e.g. the just-defocused window still claims `Activated` while
    /// geometry/title events for it drain). Promoting on every Activated
    /// sighting therefore re-promotes the previous window above the new one;
    /// only a false→true transition is a real focus change.
    activated_last: HashMap<String, bool>,
}

impl WaylandState {
    fn note_toplevel_update(
        &mut self,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        let Some(info) = self.toplevel_info_state.info(handle) else {
            return;
        };
        let id = info.identifier.clone();
        let activated = info
            .state
            .contains(&zcosmic_toplevel_handle_v1::State::Activated);
        let was = self.activated_last.insert(id.clone(), activated);
        let pos = self.mru_order.iter().position(|x| x == &id);
        if activated && was != Some(true) {
            if let Some(p) = pos {
                self.mru_order.remove(p);
            }
            self.mru_order.push(id);
        } else if pos.is_none() {
            self.mru_order.insert(0, id);
        }
    }

    fn emit_window_list(&self) {
        let active: HashSet<_> = self
            .workspace_state
            .workspaces()
            .filter(|w| w.state.contains(ext_workspace_handle_v1::State::Active))
            .map(|w| w.handle.clone())
            .collect();

        let rank = |id: &str| -> usize {
            match self.mru_order.iter().rposition(|x| x == id) {
                Some(p) => self.mru_order.len() - 1 - p,
                None => usize::MAX,
            }
        };

        let mut windows: Vec<(usize, WindowInfo)> = self
            .toplevel_info_state
            .toplevels()
            .filter(|t| {
                // workspace field is empty on older protocol versions (< v3); include all then.
                t.workspace.is_empty() || t.workspace.iter().any(|h| active.contains(h))
            })
            .map(|t| {
                (
                    rank(&t.identifier),
                    WindowInfo {
                        title: t.title.clone(),
                        app_id: t.app_id.clone(),
                        identifier: t.identifier.clone(),
                    },
                )
            })
            .collect();

        windows.sort_by_key(|(r, _)| *r);
        let _ = self.sender.blocking_send(ThreadEvent::Windows(
            windows.into_iter().map(|(_, w)| w).collect(),
        ));
    }

    fn emit_screen_height(&self) {
        let min_height = self
            .output_state
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .filter_map(|info| {
                info.logical_size
                    .map(|(_, h)| u32::try_from(h).unwrap_or(0))
                    .or_else(|| {
                        info.modes
                            .iter()
                            .find(|m| m.current)
                            .map(|m| u32::try_from(m.dimensions.1).unwrap_or(0))
                            .map(|h| h / u32::try_from(info.scale_factor.max(1)).unwrap_or(1))
                    })
            })
            .filter(|h| *h > 0)
            .min();
        if let Some(h) = min_height {
            let _ = self.sender.blocking_send(ThreadEvent::ScreenHeight(h));
        }
    }

    fn activate_by_identifier(&self, identifier: &str) {
        let Some(info) = self
            .toplevel_info_state
            .toplevels()
            .find(|t| t.identifier == identifier)
        else {
            return;
        };
        let Some(cosmic) = info.cosmic_toplevel.as_ref() else {
            return;
        };
        for seat in self.seat_state.seats() {
            self.toplevel_manager_state.manager.activate(cosmic, &seat);
        }
    }
}

impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.emit_screen_height();
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.emit_screen_height();
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.emit_screen_height();
    }
}

impl ToplevelInfoHandler for WaylandState {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.toplevel_info_state
    }

    fn new_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        self.note_toplevel_update(handle);
        self.emit_window_list();
    }

    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        self.note_toplevel_update(handle);
        self.emit_window_list();
    }

    fn toplevel_closed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        if let Some(info) = self.toplevel_info_state.info(handle) {
            let id = info.identifier.clone();
            self.mru_order.retain(|x| x != &id);
            self.activated_last.remove(&id);
        }
        self.emit_window_list();
    }
}

impl ToplevelManagerHandler for WaylandState {
    fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
        &mut self.toplevel_manager_state
    }

    fn capabilities(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: Vec<WEnum<zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>>,
    ) {
    }
}

impl SeatHandler for WaylandState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl WorkspaceHandler for WaylandState {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

    fn done(&mut self) {
        self.emit_window_list();
    }
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    cctk::sctk::registry_handlers!(OutputState, SeatState);
}

cctk::sctk::delegate_output!(WaylandState);
cctk::sctk::delegate_registry!(WaylandState);
cctk::sctk::delegate_seat!(WaylandState);
cctk::delegate_toplevel_info!(WaylandState);
cctk::delegate_toplevel_manager!(WaylandState);
cctk::delegate_workspace!(WaylandState);

fn run_wayland_thread(
    sender: mpsc::Sender<ThreadEvent>,
    calloop_rx: calloop::channel::Channel<WaylandRequest>,
) {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("wayland: failed to connect: {e}");
            return;
        }
    };

    let (globals, event_queue) = match registry_queue_init::<WaylandState>(&conn) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("wayland: failed to init registry: {e}");
            return;
        }
    };
    let qh = event_queue.handle();

    let registry_state = RegistryState::new(&globals);
    let mut state = WaylandState {
        output_state: OutputState::new(&globals, &qh),
        toplevel_info_state: ToplevelInfoState::new(&registry_state, &qh),
        toplevel_manager_state: ToplevelManagerState::new(&registry_state, &qh),
        seat_state: SeatState::new(&globals, &qh),
        workspace_state: WorkspaceState::new(&registry_state, &qh),
        registry_state,
        sender,
        mru_order: Vec::new(),
        activated_last: HashMap::new(),
    };

    let mut event_loop: EventLoop<WaylandState> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("wayland: failed to create event loop: {e}");
            return;
        }
    };

    // Clone the connection before `WaylandSource` consumes it, so the request
    // handler can flush activate requests immediately — the app hides right
    // after committing, so we can't rely on a later dispatch cycle to flush.
    let flush_conn = conn.clone();
    if let Err(e) = event_loop.handle().insert_source(calloop_rx, move |event, (), state| {
        if let calloop::channel::Event::Msg(WaylandRequest::Activate(id)) = event {
            state.activate_by_identifier(&id);
            let _ = flush_conn.flush();
        }
    }) {
        eprintln!("wayland: failed to insert request channel into event loop: {e}");
        return;
    }

    if let Err(e) = WaylandSource::new(conn, event_queue).insert(event_loop.handle()) {
        eprintln!("wayland: failed to insert wayland source into event loop: {e}");
        return;
    }

    loop {
        if event_loop.dispatch(None, &mut state).is_err() {
            break;
        }
    }
}

pub fn subscribe() -> Subscription<WaylandEvent> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            64,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<WaylandEvent>| async move {
                let (tx, mut rx) = mpsc::channel::<ThreadEvent>(64);
                let (calloop_tx, calloop_rx) = calloop::channel::channel::<WaylandRequest>();
                let _ = output
                    .send(WaylandEvent::Ready(WaylandHandle(calloop_tx)))
                    .await;
                std::thread::spawn(move || run_wayland_thread(tx, calloop_rx));
                while let Some(event) = rx.recv().await {
                    let mapped = match event {
                        ThreadEvent::Windows(windows) => WaylandEvent::Windows(windows),
                        ThreadEvent::ScreenHeight(h) => WaylandEvent::ScreenHeight(h),
                    };
                    let _ = output.send(mapped).await;
                }
            },
        )
    })
}
