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

use wayland_server::{DisplayHandle, GlobalDispatch, backend::GlobalId};

pub struct WeftShellState {
    _global: GlobalId,
}

#[allow(dead_code)]
pub struct WeftShellWindowData {
    pub app_id: String,
    pub title: String,
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
        let global = display.create_global::<D, ZweftShellManagerV1, ()>(1, ());
        Self { _global: global }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use wayland_server::Resource;

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
        assert_eq!(iface.version, 1);
    }

    #[test]
    fn window_interface_name_and_version() {
        let iface = ZweftShellWindowV1::interface();
        assert_eq!(iface.name, "zweft_shell_window_v1");
        assert_eq!(iface.version, 1);
    }

    #[test]
    fn defunct_window_error_code() {
        let code = server::zweft_shell_window_v1::Error::DefunctWindow as u32;
        assert_eq!(code, 0);
    }
}
