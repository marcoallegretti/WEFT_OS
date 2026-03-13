#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#[allow(non_upper_case_globals, non_snake_case, unused_imports)]
#[allow(missing_docs, clippy::all)]
pub mod server {
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("../../protocol/weft-shell-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("../../protocol/weft-shell-unstable-v1.xml");
}

pub use server::zweft_shell_manager_v1::ZweftShellManagerV1;
pub use server::zweft_shell_window_v1::ZweftShellWindowV1;

use wayland_server::{DisplayHandle, GlobalDispatch, Resource, backend::GlobalId};

pub struct WeftShellState {
    _global: GlobalId,
    panels: Vec<ZweftShellWindowV1>,
}

pub struct WeftShellWindowData {
    #[allow(dead_code)]
    pub app_id: String,
    #[allow(dead_code)]
    pub title: String,
    #[allow(dead_code)]
    pub role: String,
    pub surface: Option<wayland_server::protocol::wl_surface::WlSurface>,
    pub closed: std::sync::atomic::AtomicBool,
}

impl WeftShellState {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZweftShellManagerV1, ()>,
        D: 'static,
    {
        let global = display.create_global::<D, ZweftShellManagerV1, ()>(2, ());
        Self {
            _global: global,
            panels: Vec::new(),
        }
    }

    pub fn add_panel(&mut self, window: ZweftShellWindowV1) {
        self.panels.push(window);
    }

    pub fn reconfigure_panels(&self, x: i32, y: i32, width: i32, height: i32) {
        for panel in &self.panels {
            if panel.is_alive() {
                panel.configure(x, y, width, height, 0);
            }
        }
    }

    pub fn send_navigation_gesture_to_panels(
        &self,
        gesture_type: u32,
        fingers: u32,
        dx: f64,
        dy: f64,
    ) {
        for panel in &self.panels {
            if panel.is_alive() && panel.version() >= 2 {
                panel.navigation_gesture(gesture_type, fingers, dx, dy);
            }
        }
    }

    pub fn retain_alive_panels(&mut self) {
        self.panels.retain(|p| p.is_alive());
    }

    pub fn panels(&self) -> impl Iterator<Item = &ZweftShellWindowV1> {
        self.panels.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn window_data_stores_fields() {
        let d = WeftShellWindowData {
            app_id: "com.example.test".into(),
            title: "Test Window".into(),
            role: "normal".into(),
            surface: None,
            closed: std::sync::atomic::AtomicBool::new(false),
        };
        assert_eq!(d.app_id, "com.example.test");
        assert_eq!(d.title, "Test Window");
        assert_eq!(d.role, "normal");
        assert!(!d.closed.load(Ordering::Relaxed));
    }

    #[test]
    fn closed_flag_transition() {
        let d = WeftShellWindowData {
            app_id: String::new(),
            title: String::new(),
            role: String::new(),
            surface: None,
            closed: std::sync::atomic::AtomicBool::new(false),
        };
        assert!(!d.closed.load(Ordering::Relaxed));
        d.closed.store(true, Ordering::Relaxed);
        assert!(d.closed.load(Ordering::Relaxed));
    }

    #[test]
    fn manager_interface_name_and_version() {
        let iface = ZweftShellManagerV1::interface();
        assert_eq!(iface.name, "zweft_shell_manager_v1");
        assert_eq!(iface.version, 2);
    }

    #[test]
    fn window_interface_name_and_version() {
        let iface = ZweftShellWindowV1::interface();
        assert_eq!(iface.name, "zweft_shell_window_v1");
        assert_eq!(iface.version, 2);
    }

    #[test]
    fn defunct_window_error_code() {
        let code = server::zweft_shell_window_v1::Error::DefunctWindow as u32;
        assert_eq!(code, 0);
    }
}
