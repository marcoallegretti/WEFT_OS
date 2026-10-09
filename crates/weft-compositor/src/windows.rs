//! Stacking, focus and activation of toplevel windows.
//!
//! The compositor decides the order and the geometry. The trusted shell's
//! panel fills the output and starts beneath every application window;
//! application windows fill the work area, the output less the strips the
//! panel reserves. A session's first window, a clicked window (the panel
//! included, which then shows the shell's home over the applications) and
//! one appd asks to activate is raised to the top, marked activated and
//! given keyboard focus; apart from a panel being lowered when it
//! registers, nothing else changes the order, and a window that maps
//! without taking focus goes beneath the window in front. When the
//! focused window closes, the topmost remaining application window gets
//! focus.

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use crate::protocols::WeftShellWindowData;
use crate::state::{WeftClientState, WeftCompositorState};

impl WeftCompositorState {
    /// Whether `surface` backs a window the trusted shell registered as its
    /// panel.
    pub fn is_panel_surface(&self, surface: &WlSurface) -> bool {
        self.weft_shell_state
            .panels()
            .filter(|p| p.is_alive())
            .any(|panel| {
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

    /// Raises `window` to the top, marks it activated and gives it keyboard
    /// focus. Activating the panel shows the shell's home, with its launcher,
    /// over the applications until one is activated again.
    pub fn activate_window(&mut self, window: &Window) {
        self.space.raise_element(window, true);
        self.send_pending_configures();
        if let (Some(keyboard), Some(surface)) = (self.seat.get_keyboard(), window.wl_surface()) {
            keyboard.set_focus(
                self,
                Some(surface.into_owned()),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    /// Asks the focused application window to close. Its client decides
    /// how to end; for an app shell that ends the session. The panel is not
    /// closed this way.
    pub fn close_focused_window(&mut self) {
        let Some(focus) = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
        else {
            return;
        };
        if self.is_panel_surface(&focus) {
            return;
        }
        let window = self
            .space
            .elements()
            .find(|window| window.wl_surface().is_some_and(|s| *s == focus))
            .cloned();
        if let Some(toplevel) = window.as_ref().and_then(|w| w.toplevel()) {
            tracing::info!(session = ?window.as_ref().and_then(window_session), "closing the focused window");
            toplevel.send_close();
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

    /// Sizes the panels to the output and keeps them beneath every
    /// application window; used when a panel registers.
    pub fn fit_panels(&mut self) {
        self.place_windows(true, None);
        // A shell that took focus before registering as the panel, such as
        // one restarted while applications run, hands it to the window that
        // is now in front.
        let focus_on_panel = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .is_some_and(|focus| self.is_panel_surface(&focus));
        let front = self.space.elements().last().cloned();
        if focus_on_panel
            && let Some(front) = front
            && !self.is_panel(&front)
        {
            self.activate_window(&front);
        }
    }

    /// Places every window for the current output and reservations: panels
    /// fill the output, application windows the work area. The stacking
    /// order is kept, except that `lower_panels` moves the panels beneath
    /// the applications, and `below_top` slips that window under the
    /// window that was on top.
    pub fn place_windows(&mut self, lower_panels: bool, below_top: Option<&Window>) {
        // Without an output the order still changes; windows keep their
        // size and place until one is mapped.
        let output = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output));
        let area = self.work_area();
        let mut order: Vec<Window> = self.space.elements().cloned().collect();
        if lower_panels {
            let (panels, apps): (Vec<Window>, Vec<Window>) =
                order.into_iter().partition(|window| self.is_panel(window));
            order = panels.into_iter().chain(apps).collect();
        }
        if let Some(window) = below_top
            && let Some(position) = order.iter().position(|w| w == window)
            && position + 1 == order.len()
            && order.len() > 1
        {
            order.swap(position, position - 1);
        }
        for window in &order {
            let panel = self.is_panel(window);
            let Some(geometry) = (if panel { output } else { area }) else {
                let location = self.space.element_location(window).unwrap_or_default();
                self.space.map_element(window.clone(), location, false);
                continue;
            };
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some(geometry.size);
                    state.states.set(xdg_toplevel::State::Maximized);
                });
                toplevel.send_pending_configure();
            }
            // Mapping puts a window on top; mapping all of them in `order`
            // leaves exactly that order.
            self.space.map_element(window.clone(), geometry.loc, false);
        }
    }

    /// Maps a new toplevel. The first window a session maps in its lifetime
    /// comes to the front with keyboard focus; its later windows, including
    /// recreated ones, are shown without taking focus, so a session cannot
    /// take keystrokes meant for another application by creating windows.
    /// A client outside any session (one connected through the display
    /// socket) takes focus only while no other such window is mapped. A
    /// panel is fitted and kept beneath.
    pub fn map_new_window(&mut self, window: Window) {
        let session = window_session(&window);
        let first = match session {
            Some(session_id) => self
                .appd_ipc
                .as_mut()
                .is_some_and(|ipc| ipc.take_focus_on_map(session_id)),
            None => !self
                .space
                .elements()
                .any(|other| window_session(other).is_none()),
        };
        self.space.map_element(window.clone(), (0, 0), false);
        if self.is_panel(&window) {
            self.fit_panels();
        } else if first {
            self.layout_app_windows();
            self.activate_window(&window);
        } else {
            // A window that does not take focus does not cover the window
            // in front either, the shell's home included.
            self.place_windows(false, Some(&window));
        }
    }

    /// Unmaps a closed toplevel and, if it had keyboard focus, gives focus
    /// to the topmost remaining application window, or the panel.
    pub fn window_closed(&mut self, surface: &WlSurface) {
        let Some(window) = self
            .space
            .elements()
            .find(|window| window.wl_surface().is_some_and(|s| *s == *surface))
            .cloned()
        else {
            return;
        };
        self.space.unmap_elem(&window);
        let had_focus = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .is_none_or(|focus| focus == *surface);
        if !had_focus {
            return;
        }
        let next = self
            .space
            .elements()
            .rev()
            .find(|window| !self.is_panel(window))
            .or_else(|| self.space.elements().next())
            .cloned();
        match next {
            Some(next) => self.activate_window(&next),
            None => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                }
            }
        }
    }

    /// The part of the output that application windows occupy: the output
    /// less the strips the panels reserved.
    pub fn work_area(&self) -> Option<Rectangle<i32, Logical>> {
        use crate::protocols::server::zweft_shell_window_v1::Edge;
        let output = self.space.outputs().next()?;
        let mut area = self.space.output_geometry(output)?;
        for panel in self.weft_shell_state.panels().filter(|p| p.is_alive()) {
            let Some(data) = panel.data::<WeftShellWindowData>() else {
                continue;
            };
            let zone = *data
                .exclusive_zone
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let Some((edge, size)) = zone else {
                continue;
            };
            match edge {
                Edge::Top => {
                    let size = size.min(area.size.h);
                    area.loc.y += size;
                    area.size.h -= size;
                }
                Edge::Bottom => area.size.h -= size.min(area.size.h),
                Edge::Left => {
                    let size = size.min(area.size.w);
                    area.loc.x += size;
                    area.size.w -= size;
                }
                Edge::Right => area.size.w -= size.min(area.size.w),
            }
        }
        // A zone covering the output still leaves applications a size the
        // client must honour, never 0, which would let it pick its own.
        area.size.w = area.size.w.max(1);
        area.size.h = area.size.h.max(1);
        Some(area)
    }

    /// The output's geometry as (x, y, width, height).
    pub fn output_geometry(&self) -> (i32, i32, i32, i32) {
        self.space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map_or((0, 0, 0, 0), |g| (g.loc.x, g.loc.y, g.size.w, g.size.h))
    }

    /// The geometry the layout gives application windows, as
    /// (x, y, width, height).
    pub fn app_geometry(&self) -> (i32, i32, i32, i32) {
        self.work_area()
            .map_or((0, 0, 0, 0), |a| (a.loc.x, a.loc.y, a.size.w, a.size.h))
    }

    /// Lays the windows out again after the work area changed, keeping the
    /// stacking order.
    pub fn layout_app_windows(&mut self) {
        self.place_windows(false, None);
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
