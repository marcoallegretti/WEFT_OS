use anyhow::Context;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    protocol::{wl_registry, wl_surface::WlSurface},
};

use crate::protocols::{
    ZweftShellManagerV1, ZweftShellWindowV1,
    client::{zweft_shell_manager_v1, zweft_shell_window_v1},
};

// ── Window state ──────────────────────────────────────────────────────────────

pub struct ShellWindowState {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub state_flags: u32,
    pub focused: bool,
    pub closed: bool,
}

// ── Internal Wayland dispatch state ──────────────────────────────────────────

pub struct PendingGesture {
    pub gesture_type: u32,
    pub fingers: u32,
    pub dx: f64,
    pub dy: f64,
}

struct AppData {
    manager: Option<ZweftShellManagerV1>,
    window: Option<ZweftShellWindowV1>,
    window_state: ShellWindowState,
    pending_gestures: Vec<PendingGesture>,
}

impl AppData {
    fn new() -> Self {
        Self {
            manager: None,
            window: None,
            window_state: ShellWindowState {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                state_flags: 0,
                focused: false,
                closed: false,
            },
            pending_gestures: Vec::new(),
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppData {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "zweft_shell_manager_v1"
        {
            let mgr = registry.bind::<ZweftShellManagerV1, _, _>(name, version.min(2), qh, ());
            state.manager = Some(mgr);
        }
    }
}

impl Dispatch<ZweftShellManagerV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &ZweftShellManagerV1,
        _event: zweft_shell_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZweftShellWindowV1, ()> for AppData {
    fn event(
        state: &mut Self,
        window: &ZweftShellWindowV1,
        event: zweft_shell_window_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zweft_shell_window_v1::Event::Configure {
                x,
                y,
                width,
                height,
                state: flags,
            } => {
                let ws = &mut state.window_state;
                ws.x = x;
                ws.y = y;
                ws.width = width;
                ws.height = height;
                ws.state_flags = flags;
                tracing::debug!(x, y, width, height, flags, "shell window configure");
            }
            zweft_shell_window_v1::Event::FocusChanged { focused } => {
                state.window_state.focused = focused != 0;
                tracing::debug!(focused, "shell window focus changed");
            }
            zweft_shell_window_v1::Event::WindowClosed => {
                tracing::info!("shell window closed by compositor");
                state.window_state.closed = true;
                window.destroy();
            }
            zweft_shell_window_v1::Event::PresentationFeedback {
                tv_sec,
                tv_nsec,
                refresh,
            } => {
                tracing::trace!(tv_sec, tv_nsec, refresh, "shell presentation feedback");
            }
            zweft_shell_window_v1::Event::NavigationGesture {
                _type,
                fingers,
                dx,
                dy,
            } => {
                tracing::debug!(_type, fingers, dx, dy, "navigation gesture from compositor");
                state.pending_gestures.push(PendingGesture {
                    gesture_type: _type,
                    fingers,
                    dx,
                    dy,
                });
            }
        }
    }
}

// ── Public client ─────────────────────────────────────────────────────────────

pub struct ShellClient {
    event_queue: EventQueue<AppData>,
    data: AppData,
}

impl ShellClient {
    /// Connect to the compositor using winit's existing Wayland display handle.
    ///
    /// Using `Backend::from_foreign_display` shares winit's `wl_display` connection so
    /// that `surface_ptr` (a `wl_surface*` owned by winit) is a valid object on the same
    /// client, enabling the compositor to link the shell window to the actual surface.
    #[cfg(feature = "servo-embed")]
    pub fn connect_with_display(
        display_ptr: *mut std::ffi::c_void,
        surface_ptr: *mut std::ffi::c_void,
    ) -> anyhow::Result<Self> {
        use wayland_client::Proxy;
        use wayland_client::backend::{Backend, ObjectId};

        // Safety: display_ptr is winit's wl_display*, valid for the event loop lifetime.
        let conn = unsafe {
            Connection::from_backend(Backend::from_foreign_display(display_ptr as *mut _))
        };

        let mut event_queue = conn.new_event_queue::<AppData>();
        let qh = event_queue.handle();

        conn.display().get_registry(&qh, ());

        let mut data = AppData::new();
        event_queue
            .roundtrip(&mut data)
            .context("Wayland globals roundtrip")?;

        anyhow::ensure!(
            data.manager.is_some(),
            "zweft_shell_manager_v1 not advertised; WEFT compositor must be running"
        );

        // Safety: surface_ptr is winit's wl_surface* on the same wl_display connection.
        let surface = unsafe {
            let id = ObjectId::from_ptr(WlSurface::interface(), surface_ptr as *mut _)
                .context("wl_surface ObjectId import")?;
            WlSurface::from_id(&conn, id).context("wl_surface from_id")?
        };

        let manager = data
            .manager
            .as_ref()
            .expect("manager is Some; guaranteed by ensure! above");
        let window = manager.create_window(
            "org.weft.system.shell".to_string(),
            "WEFT Shell".to_string(),
            "panel".to_string(),
            Some(&surface),
            0,
            0,
            0,
            0,
            &qh,
            (),
        );
        data.window = Some(window);

        event_queue
            .roundtrip(&mut data)
            .context("Wayland create_window roundtrip")?;

        tracing::info!(
            x = data.window_state.x,
            y = data.window_state.y,
            width = data.window_state.width,
            height = data.window_state.height,
            state_flags = data.window_state.state_flags,
            "shell window registered with compositor"
        );

        Ok(Self { event_queue, data })
    }

    pub fn dispatch_pending(&mut self) -> anyhow::Result<bool> {
        self.event_queue
            .dispatch_pending(&mut self.data)
            .context("Wayland dispatch")?;
        self.event_queue.flush().context("Wayland flush")?;
        Ok(!self.data.window_state.closed)
    }

    pub fn window_state(&self) -> &ShellWindowState {
        &self.data.window_state
    }

    pub fn take_pending_gestures(&mut self) -> Vec<PendingGesture> {
        std::mem::take(&mut self.data.pending_gestures)
    }
}
