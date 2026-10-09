//! Stacking, focus and activation of toplevel windows.
//!
//! The compositor decides the order: the trusted shell's panel, which
//! fills the output, stays beneath every application window; a newly mapped
//! application window, a clicked one and one appd asks to activate is raised
//! to the top, marked activated and given keyboard focus.

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::seat::WaylandFocus;

use crate::protocols::WeftShellWindowData;
use crate::state::{WeftClientState, WeftCompositorState};

impl WeftCompositorState {
    /// Whether `surface` backs a window the trusted shell registered as its
    /// panel.
    pub fn is_panel_surface(&self, surface: &WlSurface) -> bool {
        self.weft_shell_state.panels().any(|panel| {
            panel
                .data::<WeftShellWindowData>()
                .and_then(|data| data.surface.as_ref())
                .is_some_and(|s| s == surface)
        })
    }

    fn is_panel(&self, window: &Window) -> bool {
        window
            .wl_surface()
            .is_some_and(|surface| self.is_panel_surface(&surface))
    }

    /// Raises `window` above the other application windows, marks it
    /// activated and gives it keyboard focus. A panel is focused but stays
    /// beneath the applications.
    pub fn activate_window(&mut self, window: &Window) {
        let panel = self.is_panel(window);
        if !panel {
            self.space.raise_element(window, true);
        }
        self.send_pending_configures();
        if let (Some(keyboard), Some(surface)) = (self.seat.get_keyboard(), window.wl_surface()) {
            keyboard.set_focus(
                self,
                Some(surface.into_owned()),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    /// Activates the topmost window of `session_id`; returns whether the
    /// session has one.
    pub fn activate_session(&mut self, session_id: u64) -> bool {
        let window = self
            .space
            .elements()
            .rev()
            .find(|window| window_session(window) == Some(session_id))
            .cloned();
        match window {
            Some(window) => {
                self.activate_window(&window);
                true
            }
            None => false,
        }
    }

    /// Sizes the panels to the output, places them at its origin and keeps
    /// them beneath every application window.
    pub fn fit_panels(&mut self) {
        let Some(geometry) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        let panels: Vec<Window> = self
            .space
            .elements()
            .filter(|window| self.is_panel(window))
            .cloned()
            .collect();
        if panels.is_empty() {
            return;
        }
        for panel in &panels {
            if let Some(toplevel) = panel.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some(geometry.size);
                    state.states.set(xdg_toplevel::State::Maximized);
                });
                toplevel.send_pending_configure();
                tracing::debug!(
                    width = geometry.size.w,
                    height = geometry.size.h,
                    "panel fitted to the output"
                );
            }
            self.space.map_element(panel.clone(), geometry.loc, false);
        }
        // Raising every other window, in its current order, leaves the
        // panels at the bottom.
        let others: Vec<Window> = self
            .space
            .elements()
            .filter(|window| !self.is_panel(window))
            .cloned()
            .collect();
        for window in &others {
            self.space.raise_element(window, false);
        }
    }

    /// Sends the configure for every toplevel whose pending state changed,
    /// such as its activated state.
    fn send_pending_configures(&self) {
        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }
}

/// The appd session `window`'s client is bound to, if any.
pub fn window_session(window: &Window) -> Option<u64> {
    window
        .toplevel()?
        .wl_surface()
        .client()?
        .get_data::<WeftClientState>()?
        .session
        .as_ref()
        .map(|binding| binding.session_id)
}
