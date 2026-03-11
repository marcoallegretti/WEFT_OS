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

struct AppData {
    manager: Option<ZweftShellManagerV1>,
    window: Option<ZweftShellWindowV1>,
    window_state: ShellWindowState,
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
            let mgr = registry.bind::<ZweftShellManagerV1, _, _>(name, version.min(1), qh, ());
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
        }
    }
}

// ── Public client ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct ShellClient {
    event_queue: EventQueue<AppData>,
    data: AppData,
}

impl ShellClient {
    pub fn connect() -> anyhow::Result<Self> {
        let conn =
            Connection::connect_to_env().context("failed to connect to Wayland compositor")?;

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

        let manager = data.manager.as_ref().unwrap();
        let window = manager.create_window(
            "org.weft.system.shell".to_string(),
            "WEFT Shell".to_string(),
            "panel".to_string(),
            None::<&WlSurface>,
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

    #[allow(dead_code)]
    pub fn dispatch_pending(&mut self) -> anyhow::Result<bool> {
        self.event_queue
            .dispatch_pending(&mut self.data)
            .context("Wayland dispatch")?;
        self.event_queue.flush().context("Wayland flush")?;
        Ok(!self.data.window_state.closed)
    }

    #[allow(dead_code)]
    pub fn window_state(&self) -> &ShellWindowState {
        &self.data.window_state
    }
}
