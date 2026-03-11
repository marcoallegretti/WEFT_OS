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
