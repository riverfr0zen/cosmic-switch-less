**WARNING:** This document was composed by an LLM and thus should be evaluated carefully while consumed.


# Querying the Wayland Compositor from a libcosmic App

This tutorial walks through how to open a secondary Wayland connection from a libcosmic application, bind to compositor protocol extensions, and stream live window and workspace data into your iced UI — the same technique used to build the window listing in this app.

---

## Background: How Wayland Works

Wayland is a display server protocol. The *compositor* (e.g. COSMIC's `cosmic-comp`) is the server; applications are clients. Communication happens over Unix domain sockets via a strictly typed, binary protocol.

### Globals and the registry

When a client connects, it asks the compositor for a list of *globals* — named objects the compositor offers. Each global has:
- A **interface name** (e.g. `"ext_foreign_toplevel_list_v1"`)
- A **version** number
- A numeric **name** (ID) used to bind to it

To use a global you *bind* it, which creates a proxy object your code can call methods on and receive events from. The `wayland-client` crate handles all of this in Rust.

### Protocol extensions

The base Wayland protocol covers drawing surfaces and input. Higher-level things — like listing open windows — are defined in separate *protocol extension* files (`.xml`). The compositor only advertises extensions it supports, so code must handle the case where an extension is absent.

The two extensions we care about:

| Extension | Purpose |
|---|---|
| `ext_foreign_toplevel_list_v1` | Notifies clients when windows (toplevels) open, close, or change title/app-id |
| `ext_workspace_handle_v1` / `zcosmic_workspace_handle_v1` | Describes workspace groups and individual workspaces, including which one is currently active |

### Events are push-based

Unlike a poll-based API, Wayland events arrive asynchronously. The compositor pushes `new_toplevel`, `closed`, `state`, `done`, etc. events whenever something changes. Your client must run an event loop to receive and dispatch them.

---

## Why a Separate Wayland Connection

libcosmic manages its own Wayland connection internally to drive the UI (drawing, input, etc.). That connection is not exposed for application code to piggyback on.

**You must open a second, independent connection** dedicated to your own protocol queries. This is the established pattern in the COSMIC ecosystem — `cosmic-applets` and `cosmic-workspaces-epoch` both use it.

The secondary connection runs on a **dedicated OS thread** with its own event loop. It communicates back to the iced application via a channel.

---

## Architecture Overview

```
┌─────────────────────────────────────┐      ┌─────────────────────────────────────┐
│         OS thread (blocking)        │      │        tokio async runtime           │
│                                     │      │                                     │
│  run_wayland_thread()               │      │  subscribe() / iced Subscription    │
│  ┌─────────────────────────────┐    │      │  ┌─────────────────────────────┐    │
│  │  calloop EventLoop          │    │      │  │  stream::channel closure    │    │
│  │  ├─ WaylandSource           │    │      │  │                             │    │
│  │  └─ WaylandState            │    │      │  │  rx.recv().await            │    │
│  │      ├─ ToplevelInfoState   │    │      │  │  output.send(windows)       │    │
│  │      ├─ WorkspaceState      │    │      │  └──────────────┬──────────────┘    │
│  │      └─ sender ─────────────┼────┼──────┼────────────────┘                   │
│  └─────────────────────────────┘    │      │                 │                   │
└─────────────────────────────────────┘      │                 ▼                   │
                                             │        Message::WindowsUpdated      │
                                             │        → AppModel::update()         │
                                             └─────────────────────────────────────┘
```

Two channels are involved:
1. **`tokio::sync::mpsc`** — bridges the blocking Wayland thread to async code. `blocking_send` on the thread side; `recv().await` on the async side.
2. **`cosmic::iced::futures::channel::mpsc`** — the iced-internal channel connecting the `stream::channel` async closure to the iced runtime, which dispatches it as a `Message`.

---

## Step 1: Dependencies

Add to `Cargo.toml`:

```toml
wayland-client = "0.31"
calloop = "0.14"
calloop-wayland-source = "0.4"
cosmic-protocols = { git = "https://github.com/pop-os/cosmic-protocols", features = ["client"] }
cctk = { package = "cosmic-client-toolkit", git = "https://github.com/pop-os/cosmic-protocols" }
```

`wayland-client` is already pulled in transitively by libcosmic, but you need it as a direct dependency to import its types. `cctk` is the `cosmic-client-toolkit` crate — it provides high-level handler traits and state structs for COSMIC's Wayland protocols, built on top of `smithay-client-toolkit` (sctk).

> **Version alignment**: `calloop` must match the version that `smithay-client-toolkit` (already in your dependency tree) uses. Check `Cargo.lock` after adding and resolve any conflicts.

---

## Step 2: Shared Types

Create `src/wayland.rs` and define the data type you'll send back to the UI:

```rust
#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub title: String,
    pub app_id: String,
}
```

`app_id` is the application identifier — typically the desktop entry name (e.g. `"org.gnome.Nautilus"`, `"firefox"`).

---

## Step 3: State Struct

The Wayland listener thread centers on a state struct that implements all the required handler traits:

```rust
struct WaylandState {
    output_state: OutputState,
    registry_state: RegistryState,
    toplevel_info_state: ToplevelInfoState,
    workspace_state: WorkspaceState,
    sender: mpsc::Sender<Vec<WindowInfo>>,
}
```

**Why `OutputState` if we don't care about monitors?**  
`smithay-client-toolkit`'s registry machinery requires every state struct to handle outputs because the registry handler chain is statically typed. Even if you ignore output events, you must include `OutputState` and implement `OutputHandler` with empty methods, or the code won't compile.

**Why `RegistryState`?**  
It tracks which globals the compositor has advertised and drives the initial binding of `ToplevelInfoState` and `WorkspaceState`.

---

## Step 4: The `emit_window_list` Helper

This is the core logic. It runs after every relevant event to recompute and push the current filtered window list:

```rust
impl WaylandState {
    fn emit_window_list(&self) {
        // Collect handles of all currently active workspaces.
        let active: HashSet<_> = self
            .workspace_state
            .workspaces()
            .filter(|w| w.state.contains(ext_workspace_handle_v1::State::Active))
            .map(|w| w.handle.clone())
            .collect();

        let windows: Vec<WindowInfo> = self
            .toplevel_info_state
            .toplevels()
            .filter(|t| {
                // Workspace list is empty on older protocol versions; include all windows then.
                t.workspace.is_empty() || t.workspace.iter().any(|h| active.contains(h))
            })
            .map(|t| WindowInfo {
                title: t.title.clone(),
                app_id: t.app_id.clone(),
            })
            .collect();

        let _ = self.sender.blocking_send(windows);
    }
}
```

**Workspace state filtering**: Each workspace advertises a `state` bitfield. The active workspace has the `Active` flag set. We collect active workspace handles and then filter toplevels to those assigned to any active workspace.

**Fallback for old protocol versions**: The `ext_foreign_toplevel_handle_v1` protocol added workspace association in a later version (v3). On an older compositor the `toplevel.workspace` vec will be empty. The `t.workspace.is_empty()` guard makes the code work on both: when there's no association data, show all windows; when there is, filter by active workspace.

**`blocking_send`**: The Wayland thread is synchronous (blocking). `tokio::sync::mpsc::Sender::blocking_send` is the correct way to push from a sync context into a tokio channel. Do not use `.send().await` here — `.await` requires an async context.

> **Gotcha: bitflag naming**  
> The Wayland protocol XML uses `snake_case` entry names. `wayland-scanner` converts them to `PascalCase` enum variants, not `SCREAMING_SNAKE_CASE`. So `<entry name="active" />` becomes `State::Active`, not `State::ACTIVE`. If you write `State::ACTIVE` the compiler will silently accept it as a bitflag combination expression and your filter will always be false.

---

## Step 5: Implementing the Handler Traits

`cctk` defines handler traits your `WaylandState` must implement. Each trait gives you callbacks for compositor events and requires you to expose the relevant sub-state struct.

### `ToplevelInfoHandler`

```rust
impl ToplevelInfoHandler for WaylandState {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.toplevel_info_state
    }

    fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>,
                    _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1) {
        self.emit_window_list();
    }

    fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>,
                       _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1) {
        self.emit_window_list();
    }

    fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>,
                       _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1) {
        self.emit_window_list();
    }
}
```

Each callback receives the raw protocol handle. By the time these methods are called, `ToplevelInfoState` has already updated its internal map — so calling `emit_window_list()` immediately always reads fresh data.

### `WorkspaceHandler`

```rust
impl WorkspaceHandler for WaylandState {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

    fn done(&mut self) {
        self.emit_window_list();
    }
}
```

The workspace protocol delivers events in *batches* terminated by a `done` event. Individual workspace property events (name, state, coordinates) arrive first; `done` signals the batch is complete and the state is consistent. **Always emit inside `done`, not inside the per-property callbacks**, otherwise you may emit a half-updated state mid-batch.

### `OutputHandler` and `ProvidesRegistryState`

These are mandatory plumbing. `OutputHandler` can have no-op method bodies. `ProvidesRegistryState` uses a macro to wire up the registry handler chain:

```rust
impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output_state }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }
    cctk::sctk::registry_handlers!(OutputState);
}
```

---

## Step 6: Delegate Macros

`smithay-client-toolkit` and `cctk` use *delegate macros* to generate the low-level `wayland-client` dispatch glue. These replace several hundred lines of boilerplate that would otherwise map raw protocol object IDs to your handler methods:

```rust
cctk::sctk::delegate_output!(WaylandState);
cctk::sctk::delegate_registry!(WaylandState);
cctk::delegate_toplevel_info!(WaylandState);
cctk::delegate_workspace!(WaylandState);
```

Each macro is paired with a handler trait. If you implement `ToplevelInfoHandler` but forget `delegate_toplevel_info!`, the events will be received but never dispatched to your methods, and nothing will happen. There's no compiler error — just silence.

---

## Step 7: The Wayland Thread Function

```rust
fn run_wayland_thread(sender: mpsc::Sender<Vec<WindowInfo>>) {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => { eprintln!("wayland: failed to connect: {e}"); return; }
    };

    let (globals, event_queue) = match registry_queue_init::<WaylandState>(&conn) {
        Ok(r) => r,
        Err(e) => { eprintln!("wayland: failed to init registry: {e}"); return; }
    };
    let qh = event_queue.handle();

    let registry_state = RegistryState::new(&globals);
    let mut state = WaylandState {
        output_state: OutputState::new(&globals, &qh),
        toplevel_info_state: ToplevelInfoState::new(&registry_state, &qh),
        workspace_state: WorkspaceState::new(&registry_state, &qh),
        registry_state,
        sender,
    };

    let mut event_loop: EventLoop<WaylandState> = match EventLoop::try_new() {
        Ok(el) => el,
        Err(e) => { eprintln!("wayland: failed to create event loop: {e}"); return; }
    };

    if let Err(e) = WaylandSource::new(conn, event_queue).insert(event_loop.handle()) {
        eprintln!("wayland: failed to insert wayland source: {e}");
        return;
    }

    loop {
        if event_loop.dispatch(None, &mut state).is_err() {
            break;
        }
    }
}
```

**`Connection::connect_to_env()`** reads `$WAYLAND_DISPLAY` and opens the socket. Under COSMIC this is always set.

**`registry_queue_init`** does two things in one call: it enumerates the current globals from the compositor and returns an `EventQueue`. The type parameter (`WaylandState`) tells it which dispatch type to use.

**State construction order matters**: `RegistryState` must be created from `&globals` before `ToplevelInfoState` and `WorkspaceState`, because those constructors need to look up globals by name. `registry_state` is therefore moved into the struct last.

**`WaylandSource`** bridges the Wayland socket into `calloop`. Events arriving on the Wayland socket wake the calloop event loop, which dispatches them through your handler methods.

**`event_loop.dispatch(None, &mut state)`**: The first argument is an optional timeout (`None` = block forever). The call returns `Err` if the event loop encounters a fatal error; hence the `loop { if .is_err() { break } }` pattern.

---

## Step 8: The iced Subscription

```rust
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
```

**`Subscription::run`** takes a function that returns a `Stream`. It is deduplicated by the function's type identity — returning the same function pointer means iced will only start one instance of this subscription no matter how many times `subscription()` is called.

**`stream::channel`** spawns the async closure on the iced runtime and gives it a `Sender` connected to the iced event pipeline. Messages sent through this `Sender` become `Message` values your `update()` handles.

**The two-channel bridge**:
- `tokio::sync::mpsc::channel` — the Wayland thread calls `blocking_send` on the sync sender `tx`. The async receiver `rx` is awaited in the closure.
- `cosmic::iced::futures::channel::mpsc::Sender` — this is `output`. The closure forwards each batch of windows from `rx` into `output`.

> **Gotcha: `SinkExt` import**  
> `output.send(windows).await` requires the `SinkExt` trait in scope. `futures::SinkExt` is not a direct dependency. Use `cosmic::iced::futures::SinkExt` (re-exported by libcosmic) or add `use cosmic::iced::futures::SinkExt;` at the top of the file.

> **Gotcha: Sender type annotation**  
> The `stream::channel` closure needs an explicit type annotation on `output`. The compiler cannot infer it from the closure body alone. Write `|mut output: cosmic::iced::futures::channel::mpsc::Sender<Vec<WindowInfo>>|`.

---

## Step 9: Wiring into `app.rs`

**Add to `AppModel`:**
```rust
windows: Vec<crate::wayland::WindowInfo>,
```

Initialize it in `init()`:
```rust
windows: Vec::new(),
```

**Add to `Message`:**
```rust
WindowsUpdated(Vec<crate::wayland::WindowInfo>),
```

**In `subscription()`:**
```rust
crate::wayland::subscribe().map(Message::WindowsUpdated),
```

**In `update()`:**
```rust
Message::WindowsUpdated(windows) => {
    self.windows = windows;
}
```

**In `view()`**, iterate `self.windows` to render:
```rust
for w in &self.windows {
    list = list.push(widget::text(format!("{} — {}", w.title, w.app_id)));
}
```

**In `src/main.rs`**, expose the module:
```rust
mod wayland;
```

---

## Full Imports Reference

```rust
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
use tokio::sync::mpsc;
```

Note that `wayland-client`, `wayland-protocols`, and `sctk` types are all accessed through the `cctk` re-export path. This ensures version coherence — `cctk` re-exports the exact versions it was built against.

---

## Quirks and Gotchas Summary

| # | Issue | Detail |
|---|---|---|
| 1 | Bitflag naming | `State::Active`, not `State::ACTIVE` — `wayland-scanner` uses `PascalCase` for enum variants generated from XML `snake_case` names. |
| 2 | `OutputState` is mandatory | Must be present in your state struct and `OutputHandler` must be implemented, even if you never use it. Required by sctk's registry dispatch machinery. |
| 3 | Delegate macros are silent | Forgetting a `delegate_*!` macro means events arrive but are never dispatched. No compiler error, no runtime error — just no callbacks. |
| 4 | `WorkspaceHandler::done()` vs per-property callbacks | Emit your window list from `done()`, not from individual workspace event callbacks. `done` signals a consistent batch boundary. |
| 5 | `blocking_send` vs `.send().await` | The Wayland thread is synchronous. Use `blocking_send` on `tokio::sync::mpsc::Sender`. Using `.send().await` will panic or fail to compile outside an async context. |
| 6 | `SinkExt` import | `output.send(windows).await` needs `SinkExt` in scope. Use `cosmic::iced::futures::SinkExt`, not `futures::SinkExt` (which is not a direct dep). |
| 7 | `output` type annotation | Annotate the `stream::channel` closure's sender explicitly: `\|mut output: cosmic::iced::futures::channel::mpsc::Sender<T>\|`. Type inference is insufficient here. |
| 8 | Empty workspace list = older protocol | `t.workspace.is_empty()` must be treated as "include this window", not "exclude it". On compositors with older protocol versions, the workspace association field is simply absent. |
| 9 | State construction order | Build `RegistryState` from `&globals` first, then pass `&registry_state` to `ToplevelInfoState::new` and `WorkspaceState::new`. |
| 10 | `settings::item::builder` cannot be used in `section.add()` with `.description()` | `builder(label).description(text)` does not implement `IntoListItem`. Use bare `builder(label)` or a plain `widget::text` column for multi-field display in the window list. |

---

## Reference Implementations

These are the upstream sources that informed this implementation:

- [`cosmic-applets/cosmic-applet-workspaces/src/wayland_subscription.rs`](https://github.com/pop-os/cosmic-applets) — canonical channel + Subscription bridge pattern
- [`cosmic-workspaces-epoch/src/backend/wayland/mod.rs`](https://github.com/pop-os/cosmic-workspaces-epoch) — full `ToplevelInfoHandler` + `WorkspaceHandler` dispatch
