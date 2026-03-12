#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#[allow(non_upper_case_globals, non_snake_case, unused_imports)]
#[allow(missing_docs, clippy::all)]
pub mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("../../protocol/weft-shell-unstable-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("../../protocol/weft-shell-unstable-v1.xml");
}

#[allow(unused_imports)]
pub use client::zweft_shell_manager_v1::ZweftShellManagerV1;
#[allow(unused_imports)]
pub use client::zweft_shell_window_v1::ZweftShellWindowV1;
