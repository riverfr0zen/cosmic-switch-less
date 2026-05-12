// SPDX-License-Identifier: MPL-2.0

use std::collections::HashSet;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use cctk::sctk::output::{OutputHandler, OutputState};
use cctk::sctk::registry::{ProvidesRegistryState, RegistryState};
use cctk::toplevel_info::{ToplevelInfoHandler, ToplevelInfoState};
use cctk::wayland_client::globals::registry_queue_init;
use cctk::wayland_client::protocol::wl_output;
use cctk::wayland_client::{Connection, QueueHandle};
use cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1;
use cctk::wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1;
use cctk::workspace::{WorkspaceHandler, WorkspaceState};
use cosmic::iced::Subscription;
use cosmic::iced::futures::SinkExt;
use cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub title: String,
    pub app_id: String,
}

struct WaylandState {
    output_state: OutputState,
    registry_state: RegistryState,
    toplevel_info_state: ToplevelInfoState,
    workspace_state: WorkspaceState,
    sender: mpsc::Sender<Vec<WindowInfo>>,
    mru_order: Vec<String>,
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
        let pos = self.mru_order.iter().position(|x| x == &id);
        if info
            .state
            .contains(&zcosmic_toplevel_handle_v1::State::Activated)
        {
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
                    },
                )
            })
            .collect();

        windows.sort_by_key(|(r, _)| *r);
        let _ = self
            .sender
            .blocking_send(windows.into_iter().map(|(_, w)| w).collect());
    }
}

impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
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
        }
        self.emit_window_list();
    }
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
    cctk::sctk::registry_handlers!(OutputState);
}

cctk::sctk::delegate_output!(WaylandState);
cctk::sctk::delegate_registry!(WaylandState);
cctk::delegate_toplevel_info!(WaylandState);
cctk::delegate_workspace!(WaylandState);

fn run_wayland_thread(sender: mpsc::Sender<Vec<WindowInfo>>) {
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
        workspace_state: WorkspaceState::new(&registry_state, &qh),
        registry_state,
        sender,
        mru_order: Vec::new(),
    };

    let mut event_loop: EventLoop<WaylandState> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("wayland: failed to create event loop: {e}");
            return;
        }
    };

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

pub fn subscribe() -> Subscription<Vec<WindowInfo>> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            64,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Vec<WindowInfo>>| async move {
                let (tx, mut rx) = mpsc::channel::<Vec<WindowInfo>>(64);
                std::thread::spawn(move || run_wayland_thread(tx));
                while let Some(windows) = rx.recv().await {
                    let _ = output.send(windows).await;
                }
            },
        )
    })
}
