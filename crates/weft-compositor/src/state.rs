#[cfg(target_os = "linux")]
use crate::backend::drm_device::WeftDrmData;
use crate::protocols::{
    WeftShellState, WeftShellWindowData, ZweftShellManagerV1, ZweftShellWindowV1,
    server::{zweft_shell_manager_v1, zweft_shell_window_v1},
};

use smithay::{
    backend::{input::TabletToolDescriptor, renderer::utils::on_commit_buffer_handler},
    delegate_compositor, delegate_cursor_shape, delegate_dmabuf, delegate_input_method_manager,
    delegate_layer_shell, delegate_output, delegate_pointer_constraints, delegate_presentation,
    delegate_seat, delegate_shm, delegate_text_input_manager, delegate_xdg_shell,
    desktop::{
        LayerSurface as DesktopLayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType, layer_map_for_output,
    },
    input::{Seat, SeatHandler, SeatState, keyboard::XkbConfig, pointer::CursorImageStatus},
    output::Output,
    reexports::{
        calloop::{LoopHandle, LoopSignal},
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        input_method::{
            InputMethodHandler, InputMethodManagerState, PopupSurface as ImPopupSurface,
        },
        output::OutputManagerState,
        pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState},
        presentation::PresentationState,
        seat::WaylandFocus,
        shell::{
            wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
            xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        },
        shm::{ShmHandler, ShmState},
        tablet_manager::TabletSeatHandler,
        text_input::TextInputManagerState,
    },
};

// Per-client state that Smithay compositor protocol handlers need.
#[derive(Default)]
pub struct WeftClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for WeftClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[allow(dead_code)]
pub struct WeftCompositorState {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, WeftCompositorState>,

    // Wayland protocol globals
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub output_manager_state: OutputManagerState,
    pub presentation_state: PresentationState,
    pub text_input_state: TextInputManagerState,
    pub input_method_state: InputMethodManagerState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub cursor_shape_state: CursorShapeManagerState,

    // Desktop abstraction layer
    pub space: Space<Window>,
    pub popups: PopupManager,

    // Seat and input state
    pub seat_state: SeatState<Self>,
    pub seat: Seat<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_image_status: CursorImageStatus,

    // Set by the backend after renderer initialisation when DMA-BUF is supported.
    pub dmabuf_global: Option<DmabufGlobal>,

    // Set to false when the compositor should exit the event loop.
    pub running: bool,

    // WEFT compositor–shell protocol global.
    pub weft_shell_state: WeftShellState,

    #[cfg(target_os = "linux")]
    pub drm: Option<WeftDrmData>,
}

impl WeftCompositorState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        loop_handle: LoopHandle<'static, Self>,
        seat_name: String,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let dmabuf_state = DmabufState::new();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        // Clock ID 1 = CLOCK_MONOTONIC
        let presentation_state = PresentationState::new::<Self>(&display_handle, 1);
        let text_input_state = TextInputManagerState::new::<Self>(&display_handle);
        let input_method_state =
            InputMethodManagerState::new::<Self, _>(&display_handle, |_client| true);
        let pointer_constraints_state = PointerConstraintsState::new::<Self>(&display_handle);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&display_handle);
        let weft_shell_state = WeftShellState::new::<Self>(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, seat_name);
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("no xkb config errors expected with default config");
        seat.add_pointer();
        seat.add_touch();

        Self {
            display_handle,
            loop_signal,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            dmabuf_state,
            output_manager_state,
            presentation_state,
            text_input_state,
            input_method_state,
            pointer_constraints_state,
            cursor_shape_state,
            weft_shell_state,
            space: Space::default(),
            popups: PopupManager::default(),
            seat_state,
            seat,
            pointer_location: Point::from((0.0_f64, 0.0_f64)),
            cursor_image_status: CursorImageStatus::Hidden,
            dmabuf_global: None,
            running: true,
            #[cfg(target_os = "linux")]
            drm: None,
        }
    }
}

// --- CompositorHandler ---

impl CompositorHandler for WeftCompositorState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<WeftClientState>()
            .expect("client must carry WeftClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if let Some(window) = self
            .space
            .elements()
            .find(|w| w.wl_surface().as_deref() == Some(surface))
            .cloned()
        {
            window.on_commit();
        }

        // Re-arrange layer surfaces for any output that contains this surface.
        let outputs: Vec<Output> = self
            .space
            .outputs()
            .filter(|o| {
                let map = layer_map_for_output(o);
                map.layer_for_surface(surface, WindowSurfaceType::ALL)
                    .is_some()
            })
            .cloned()
            .collect();
        for output in outputs {
            layer_map_for_output(&output).arrange();
        }
    }
}

delegate_compositor!(WeftCompositorState);

// --- ShmHandler ---

impl ShmHandler for WeftCompositorState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(WeftCompositorState);

// --- XdgShellHandler ---

impl XdgShellHandler for WeftCompositorState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Send initial configure before wrapping — the toplevel needs a configure to map.
        surface.send_configure();
        let window = Window::new_wayland_window(surface);
        // Map at origin; proper placement policy comes with the shell protocol wave.
        self.space.map_element(window, (0, 0), false);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        if surface.send_configure().is_ok() {
            self.popups.track_popup(PopupKind::Xdg(surface)).ok();
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
    }
}

delegate_xdg_shell!(WeftCompositorState);

// --- WlrLayerShellHandler ---

impl WlrLayerShellHandler for WeftCompositorState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let desktop_surface = DesktopLayerSurface::new(surface, namespace);
        if let Some(output) = self.space.outputs().next().cloned() {
            layer_map_for_output(&output)
                .map_layer(&desktop_surface)
                .expect("layer surface must not already be mapped");
            layer_map_for_output(&output).arrange();
        }
    }
}

delegate_layer_shell!(WeftCompositorState);

// --- SeatHandler ---

impl SeatHandler for WeftCompositorState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_image_status = image;
    }
}

delegate_seat!(WeftCompositorState);

// --- BufferHandler (required supertrait for DmabufHandler) ---

impl BufferHandler for WeftCompositorState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

// --- DmabufHandler ---

impl DmabufHandler for WeftCompositorState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        #[cfg(target_os = "linux")]
        if let Some(drm) = self.drm.as_mut() {
            use smithay::backend::renderer::ImportDma;
            let node = drm.primary_gpu;
            if drm
                .gpu_manager
                .single_renderer(&node)
                .ok()
                .and_then(|mut r| r.import_dmabuf(&dmabuf, None).ok())
                .is_some()
            {
                let _ = notifier.successful::<Self>();
                return;
            }
        }
        drop(notifier);
    }
}

delegate_dmabuf!(WeftCompositorState);

// --- OutputHandler ---

impl smithay::wayland::output::OutputHandler for WeftCompositorState {}
delegate_output!(WeftCompositorState);

// PresentationState has no handler trait; delegate macro only requires Dispatch bounds.
delegate_presentation!(WeftCompositorState);

// TextInputManagerState has no handler trait; delegate macro only requires SeatHandler.
delegate_text_input_manager!(WeftCompositorState);

// --- InputMethodHandler ---

impl InputMethodHandler for WeftCompositorState {
    fn new_popup(&mut self, _surface: ImPopupSurface) {}
    fn dismiss_popup(&mut self, _surface: ImPopupSurface) {}
    fn popup_repositioned(&mut self, _surface: ImPopupSurface) {}

    fn parent_geometry(&self, parent_surface: &WlSurface) -> Rectangle<i32, Logical> {
        self.space
            .elements()
            .find_map(|w: &Window| {
                if w.wl_surface().as_deref() == Some(parent_surface) {
                    Some(w.geometry())
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }
}

delegate_input_method_manager!(WeftCompositorState);

// --- PointerConstraintsHandler ---

impl PointerConstraintsHandler for WeftCompositorState {
    fn new_constraint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
        _location: smithay::utils::Point<f64, Logical>,
    ) {
    }
}

delegate_pointer_constraints!(WeftCompositorState);

// --- TabletSeatHandler (required by delegate_cursor_shape!) ---

impl TabletSeatHandler for WeftCompositorState {
    fn tablet_tool_image(&mut self, _tool: &TabletToolDescriptor, image: CursorImageStatus) {
        self.cursor_image_status = image;
    }
}

// CursorShapeManagerState has no handler trait; it calls SeatHandler::cursor_image directly.
delegate_cursor_shape!(WeftCompositorState);

// --- weft-shell-protocol ---

impl GlobalDispatch<ZweftShellManagerV1, ()> for WeftCompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZweftShellManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZweftShellManagerV1, ()> for WeftCompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ZweftShellManagerV1,
        request: zweft_shell_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zweft_shell_manager_v1::Request::Destroy => {}
            zweft_shell_manager_v1::Request::CreateWindow {
                id,
                app_id,
                title,
                role,
                x,
                y,
                width,
                height,
            } => {
                let window = data_init.init(
                    id,
                    WeftShellWindowData {
                        app_id,
                        title,
                        role,
                    },
                );
                window.configure(x, y, width, height, 0);
            }
        }
    }
}

impl Dispatch<ZweftShellWindowV1, WeftShellWindowData> for WeftCompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &ZweftShellWindowV1,
        request: zweft_shell_window_v1::Request,
        _data: &WeftShellWindowData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zweft_shell_window_v1::Request::Destroy => {}
            zweft_shell_window_v1::Request::UpdateMetadata { title, role } => {
                let _ = (title, role);
            }
            zweft_shell_window_v1::Request::SetGeometry {
                x,
                y,
                width,
                height,
            } => {
                resource.configure(x, y, width, height, 0);
            }
        }
    }
}
