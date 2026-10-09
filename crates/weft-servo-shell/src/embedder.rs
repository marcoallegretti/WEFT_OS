use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use servo::{
    DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent,
    MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    RenderingContext, RgbaImage, ServoBuilder, ServoDelegate, ServoUrl, UserContentManager,
    UserScript, WebViewBuilder, WebViewDelegate,
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::{Window, WindowAttributes, WindowId},
};

// ── Event loop waker ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct WeftEventLoopWaker {
    proxy: Arc<Mutex<EventLoopProxy<ServoWake>>>,
}

#[derive(Debug, Clone)]
struct ServoWake;

impl EventLoopWaker for WeftEventLoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self
            .proxy
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .send_event(ServoWake);
    }
}

// ── Servo delegate ────────────────────────────────────────────────────────────

struct WeftServoDelegate;

impl ServoDelegate for WeftServoDelegate {
    fn notify_error(&self, error: servo::ServoError) {
        tracing::error!(?error, "Servo error");
    }
}

// ── WebView delegate ──────────────────────────────────────────────────────────

/// Frame and load progress reported by Servo, shared with the event loop.
#[derive(Default)]
struct FrameSignals {
    /// A new frame is waiting to be painted.
    redraw: AtomicBool,
    /// Servo has requested at least one repaint.
    content: AtomicBool,
}

struct WeftWebViewDelegate {
    signals: Arc<FrameSignals>,
}

impl WebViewDelegate for WeftWebViewDelegate {
    fn notify_new_frame_ready(&self, _webview: servo::WebView) {
        self.signals.content.store(true, Ordering::Relaxed);
        self.signals.redraw.store(true, Ordering::Relaxed);
    }
}

/// Servo preferences shared by the WEFT hosts.
fn host_preferences() -> servo::Preferences {
    servo::Preferences {
        // The system UI and application pages lay out with CSS Grid, which
        // Servo disables by default.
        layout_grid_enabled: true,
        ..Default::default()
    }
}

// ── Rendering ───────────────────────────────────────────────────────────────

type BlitSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

enum RenderingCtx {
    /// Servo renders offscreen; each frame is read back and copied to the window.
    Software {
        context: Rc<servo::SoftwareRenderingContext>,
        surface: BlitSurface,
    },
    /// Servo renders directly to the window's EGL surface.
    Egl(Rc<servo::WindowRenderingContext>),
}

impl RenderingCtx {
    fn as_dyn(&self) -> Rc<dyn RenderingContext> {
        match self {
            Self::Software { context, .. } => Rc::clone(context) as Rc<dyn RenderingContext>,
            Self::Egl(context) => Rc::clone(context) as Rc<dyn RenderingContext>,
        }
    }

    /// Paints the webview into this context and presents the frame on the window.
    fn paint_and_present(&mut self, webview: &servo::WebView) -> Result<(), String> {
        match self {
            Self::Software { context, surface } => {
                context
                    .make_current()
                    .map_err(|e| format!("make_current: {e:?}"))?;
                webview.paint();
                let size = context.size();
                let rect = DeviceIntRect::from_size(DeviceIntSize::new(
                    size.width as i32,
                    size.height as i32,
                ));
                let image = context.read_to_image(rect);
                context.present();
                let image = image.ok_or("frame readback failed")?;
                blit(surface, &image)
            }
            Self::Egl(context) => {
                context
                    .make_current()
                    .map_err(|e| format!("make_current: {e:?}"))?;
                webview.paint();
                context.present();
                Ok(())
            }
        }
    }
}

/// Copies an RGBA frame into the window's software surface and presents it.
fn blit(surface: &mut BlitSurface, image: &RgbaImage) -> Result<(), String> {
    let (width, height) = image.dimensions();
    let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
        return Ok(());
    };
    surface.resize(w, h).map_err(|e| format!("resize: {e}"))?;
    let mut buffer = surface.buffer_mut().map_err(|e| format!("buffer: {e}"))?;
    for (dst, src) in buffer.iter_mut().zip(image.as_raw().chunks_exact(4)) {
        *dst = u32::from(src[0]) << 16 | u32::from(src[1]) << 8 | u32::from(src[2]);
    }
    buffer.present().map_err(|e| format!("present: {e}"))
}

/// Returns winit's `wl_display` and `wl_surface` pointers when running on Wayland.
fn wayland_handles(
    event_loop: &ActiveEventLoop,
    window: &Window,
) -> Option<(*mut std::ffi::c_void, *mut std::ffi::c_void)> {
    let RawDisplayHandle::Wayland(display) = event_loop.display_handle().ok()?.as_raw() else {
        return None;
    };
    let RawWindowHandle::Wayland(surface) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    Some((display.display.as_ptr(), surface.surface.as_ptr()))
}

// ── Application state ─────────────────────────────────────────────────────────

struct App {
    url: ServoUrl,
    ws_port: u16,
    window: Option<Arc<Window>>,
    servo: Option<servo::Servo>,
    webview: Option<servo::WebView>,
    rendering_context: Option<RenderingCtx>,
    signals: Arc<FrameSignals>,
    waker: WeftEventLoopWaker,
    shutting_down: bool,
    modifiers: ModifiersState,
    cursor_pos: DevicePoint,
    shell_client: Option<crate::shell_client::ShellClient>,
    gesture_thread: Option<std::thread::JoinHandle<()>>,
}

impl App {
    fn new(url: ServoUrl, waker: WeftEventLoopWaker, ws_port: u16) -> Self {
        Self {
            url,
            ws_port,
            window: None,
            servo: None,
            webview: None,
            rendering_context: None,
            signals: Arc::default(),
            waker,
            shutting_down: false,
            modifiers: ModifiersState::default(),
            cursor_pos: DevicePoint::origin(),
            shell_client: None,
            gesture_thread: None,
        }
    }

    /// Paints and presents once Servo has requested its first repaint.
    ///
    /// Nothing is presented before that request; the first frames can still be
    /// blank or unstyled while the document loads.
    fn render_frame(&mut self) {
        if !self.signals.content.load(Ordering::Relaxed) {
            return;
        }
        let (Some(webview), Some(context)) = (&self.webview, &mut self.rendering_context) else {
            return;
        };
        if let Err(e) = context.paint_and_present(webview) {
            tracing::warn!("frame not presented: {e}");
        }
    }

    /// Drops the webview and Servo; dropping the last `Servo` handle shuts it down.
    fn shut_down(&mut self) {
        self.shutting_down = true;
        self.webview = None;
        self.servo = None;
    }
}

impl ApplicationHandler<ServoWake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = WindowAttributes::default().with_title("WEFT Shell");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!(error = %e, "failed to create shell window; exiting");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.window = Some(Arc::clone(&window));

        if self.shell_client.is_none()
            && let Some((disp, surf)) = wayland_handles(event_loop, &window)
        {
            match crate::shell_client::ShellClient::connect_with_display(disp, surf) {
                Ok(sc) => self.shell_client = Some(sc),
                Err(e) => tracing::warn!(error = %e, "shell protocol unavailable"),
            }
        }

        let servo = ServoBuilder::default()
            .preferences(host_preferences())
            .event_loop_waker(Box::new(self.waker.clone()))
            .build();

        servo.set_delegate(Rc::new(WeftServoDelegate));

        let Some(rendering_context) = build_rendering_ctx(event_loop, &window, size) else {
            tracing::error!("no rendering context available; shell cannot start");
            event_loop.exit();
            return;
        };

        let user_content_manager = Rc::new(UserContentManager::new(&servo));
        if let Some(kit_js) = load_ui_kit_script() {
            user_content_manager.add_script(Rc::new(UserScript::new(kit_js, None)));
        }
        let bridge_js = format!(
            r#"(function(){{var ws=new WebSocket('ws://127.0.0.1:{p}');var q=[];var r=false;ws.onopen=function(){{r=true;q.forEach(function(m){{ws.send(JSON.stringify(m))}});q.length=0}};window.weftIpc={{send:function(m){{if(r)ws.send(JSON.stringify(m));else q.push(m)}},onmessage:null}};ws.onmessage=function(e){{if(window.weftIpc.onmessage)window.weftIpc.onmessage(JSON.parse(e.data))}}}})()"#,
            p = self.ws_port
        );
        user_content_manager.add_script(Rc::new(UserScript::new(bridge_js, None)));

        let webview = WebViewBuilder::new(&servo, rendering_context.as_dyn())
            .delegate(Rc::new(WeftWebViewDelegate {
                signals: Arc::clone(&self.signals),
            }))
            .user_content_manager(Rc::clone(&user_content_manager))
            .url(self.url.clone().into_url())
            .build();

        self.servo = Some(servo);
        self.webview = Some(webview);
        self.rendering_context = Some(rendering_context);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ServoWake) {
        if let Some(servo) = &self.servo {
            servo.spin_event_loop();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.shutting_down {
            event_loop.exit();
            return;
        }
        let mut compositor_closed = false;
        if let Some(sc) = &mut self.shell_client {
            match sc.dispatch_pending() {
                Ok(false) => compositor_closed = true,
                Err(e) => tracing::warn!("shell client dispatch error: {e}"),
                Ok(true) => {}
            }
            let gestures = sc.take_pending_gestures();
            if !gestures.is_empty() {
                let prev_done = self
                    .gesture_thread
                    .as_ref()
                    .map(|h| h.is_finished())
                    .unwrap_or(true);
                if prev_done {
                    let ws_port = self.ws_port;
                    self.gesture_thread = Some(std::thread::spawn(move || {
                        forward_gestures_to_appd(ws_port, &gestures);
                    }));
                } else {
                    tracing::debug!(
                        count = gestures.len(),
                        "gesture forwarding in progress; dropping batch"
                    );
                }
            }
        }
        if compositor_closed {
            self.shut_down();
            event_loop.exit();
            return;
        }
        if let Some(servo) = &self.servo {
            servo.spin_event_loop();
        }
        if self.signals.redraw.swap(false, Ordering::Relaxed)
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => self.render_frame(),
            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                if let Some(wv) = &self.webview {
                    wv.resize(new_size);
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(wv) = &self.webview {
                    let ev = super::keyutils::keyboard_event_from_winit(&event, self.modifiers);
                    let _ = wv.notify_input_event(InputEvent::Keyboard(ev));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pt = DevicePoint::new(position.x as f32, position.y as f32);
                self.cursor_pos = pt;
                if let Some(wv) = &self.webview {
                    let _ = wv
                        .notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(pt.into())));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let btn = match button {
                    MouseButton::Left => ServoMouseButton::Left,
                    MouseButton::Right => ServoMouseButton::Right,
                    MouseButton::Middle => ServoMouseButton::Middle,
                    _ => return,
                };
                let action = match state {
                    ElementState::Pressed => MouseButtonAction::Down,
                    ElementState::Released => MouseButtonAction::Up,
                };
                if let Some(wv) = &self.webview {
                    let _ = wv.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        btn,
                        self.cursor_pos.into(),
                    )));
                }
            }
            WindowEvent::CloseRequested => {
                self.shut_down();
                event_loop.exit();
            }
            _ => {}
        }
    }
}

// ── Rendering context factory ─────────────────────────────────────────────────

fn build_rendering_ctx(
    event_loop: &ActiveEventLoop,
    window: &Arc<Window>,
    size: PhysicalSize<u32>,
) -> Option<RenderingCtx> {
    if std::env::var_os("WEFT_EGL_RENDERING").is_some() {
        let display_handle = event_loop.display_handle();
        let window_handle = window.window_handle();
        if let (Ok(dh), Ok(wh)) = (display_handle, window_handle) {
            match servo::WindowRenderingContext::new(dh, wh, size) {
                Ok(rc) => {
                    tracing::info!("using EGL rendering context");
                    return Some(RenderingCtx::Egl(Rc::new(rc)));
                }
                Err(e) => {
                    tracing::warn!(
                        "EGL rendering context failed ({e:?}), falling back to software"
                    );
                }
            }
        }
    }
    let context = match servo::SoftwareRenderingContext::new(size) {
        Ok(rc) => Rc::new(rc),
        Err(e) => {
            tracing::error!("SoftwareRenderingContext failed: {e:?}");
            return None;
        }
    };
    let surface = softbuffer::Context::new(Arc::clone(window))
        .and_then(|display| softbuffer::Surface::new(&display, Arc::clone(window)));
    match surface {
        Ok(surface) => Some(RenderingCtx::Software { context, surface }),
        Err(e) => {
            tracing::error!("software presentation surface failed: {e}");
            None
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

fn resolve_weft_app_url(url: &ServoUrl) -> Option<ServoUrl> {
    if url.scheme() != "weft-app" {
        return None;
    }
    let app_id = url.host_str()?;
    let rel = url.path().trim_start_matches('/');
    let file_path = app_store_roots()
        .into_iter()
        .map(|r| r.join(app_id).join("ui").join(rel))
        .find(|p| p.exists())?;
    let s = format!("file://{}", file_path.display());
    ServoUrl::parse(&s).ok()
}

fn load_ui_kit_script() -> Option<String> {
    let path = std::env::var("WEFT_UI_KIT_JS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/usr/share/weft/system/weft-ui-kit.js"));
    std::fs::read_to_string(path).ok()
}

fn app_store_roots() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("WEFT_APP_STORE") {
        return vec![PathBuf::from(v)];
    }
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("weft")
                .join("apps"),
        );
    }
    roots.push(PathBuf::from("/usr/share/weft/apps"));
    roots
}

pub fn run(html_path: &Path, ws_port: u16) -> anyhow::Result<()> {
    let url_str = format!("file://{}", html_path.display());
    let raw_url =
        ServoUrl::parse(&url_str).map_err(|e| anyhow::anyhow!("invalid URL {url_str}: {e}"))?;
    let url = resolve_weft_app_url(&raw_url).unwrap_or(raw_url);

    let event_loop = EventLoop::<ServoWake>::with_user_event()
        .build()
        .map_err(|e| anyhow::anyhow!("event loop: {e}"))?;

    let waker = WeftEventLoopWaker {
        proxy: Arc::new(Mutex::new(event_loop.create_proxy())),
    };

    let mut app = App::new(url, waker, ws_port);
    event_loop
        .run_app(&mut app)
        .map_err(|e| anyhow::anyhow!("event loop run: {e}"))
}

fn forward_gestures_to_appd(ws_port: u16, gestures: &[crate::shell_client::PendingGesture]) {
    use std::net::TcpStream;
    let addr = format!("127.0.0.1:{ws_port}");
    let stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("gesture forward: connect to {addr} failed: {e}");
            return;
        }
    };
    let url = format!("ws://{addr}/");
    let (mut ws, _) = match tungstenite::client::client(url, stream) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!("gesture forward: WebSocket handshake failed: {e}");
            return;
        }
    };
    for g in gestures {
        let json = format!(
            r#"{{"type":"PANEL_GESTURE","gesture_type":{},"fingers":{},"dx":{},"dy":{}}}"#,
            g.gesture_type, g.fingers, g.dx, g.dy
        );
        if let Err(e) = ws.send(tungstenite::Message::Text(json)) {
            tracing::warn!("gesture forward: send failed: {e}");
            break;
        }
    }
    let _ = ws.close(None);
}
