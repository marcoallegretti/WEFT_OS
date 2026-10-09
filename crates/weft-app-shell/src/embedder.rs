use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use servo::{
    DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent, LoadStatus,
    MouseButton as ServoMouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    NavigationRequest, RenderingContext, RgbaImage, ServoBuilder, ServoDelegate, ServoUrl,
    UserContentManager, UserScript, WebViewBuilder, WebViewDelegate,
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

struct WeftServoDelegate;

impl ServoDelegate for WeftServoDelegate {
    fn notify_error(&self, error: servo::ServoError) {
        tracing::error!(?error, "Servo error");
    }

    /// Loads not tied to the webview, such as `navigator.sendBeacon()`,
    /// worklets and notification icons, also come from the application page,
    /// and none of them is needed: all are refused.
    fn load_web_resource(&self, load: servo::WebResourceLoad) {
        let url = load.request().url.clone();
        tracing::warn!(%url, "resource load outside the webview denied");
        load.intercept(servo::WebResourceResponse::new(url))
            .cancel();
    }
}

/// Frame and load progress reported by Servo, shared with the event loop.
#[derive(Default)]
struct FrameSignals {
    /// A new frame is waiting to be painted.
    redraw: AtomicBool,
    /// Servo has requested at least one repaint.
    content: AtomicBool,
    /// The document has loaded and Servo reports its rendering is up to date.
    settled: AtomicBool,
}

struct WeftWebViewDelegate {
    signals: Arc<FrameSignals>,
    /// URL prefix of the application's UI directory; see `ui_scope`.
    scope: String,
    /// The UI directory with symbolic links resolved, if it exists.
    ui_dir: Option<std::path::PathBuf>,
    /// The bridge's WebSocket handshake URL, as the network layer requests it
    /// (`ws:` becomes `http:`).
    bridge: String,
}

impl WebViewDelegate for WeftWebViewDelegate {
    fn notify_new_frame_ready(&self, _webview: servo::WebView) {
        self.signals.content.store(true, Ordering::Relaxed);
        self.signals.redraw.store(true, Ordering::Relaxed);
    }

    /// The webview carries the session's bridge, so it stays within the
    /// application's own UI files.
    fn request_navigation(&self, _webview: servo::WebView, request: NavigationRequest) {
        if in_scope(request.url.as_str(), &self.scope) {
            request.allow();
        } else {
            tracing::warn!(url = %request.url, "navigation outside the application UI denied");
            request.deny();
        }
    }

    /// Every resource the webview loads passes through here, including
    /// `fetch()`, XHR, subresources and WebSocket handshakes. The page may
    /// read its own UI files and open the session bridge; any other local
    /// file or network destination is refused with a network error. Network
    /// access belongs to the component, under its fetch grants.
    ///
    /// On a redirect Servo reports the first URL of the chain again, not the
    /// target, so every redirect is refused.
    fn load_web_resource(&self, _webview: servo::WebView, load: servo::WebResourceLoad) {
        let request = load.request();
        let url = request.url.clone();
        if !request.is_redirect && self.allows(&ServoUrl::from_url(url.clone())) {
            return;
        }
        tracing::warn!(%url, "resource outside the application denied");
        load.intercept(servo::WebResourceResponse::new(url))
            .cancel();
    }

    fn notify_load_status_changed(&self, webview: servo::WebView, status: LoadStatus) {
        if status != LoadStatus::Complete {
            return;
        }
        // Servo completes a screenshot request only once every frame has fired
        // `load`, render-blocking resources, images and web fonts are done and
        // the rendering is up to date, which is the readiness this host reports.
        let signals = Arc::clone(&self.signals);
        webview.take_screenshot(None, move |result| match result {
            Ok(_) => {
                signals.settled.store(true, Ordering::Relaxed);
                signals.redraw.store(true, Ordering::Relaxed);
            }
            Err(e) => tracing::error!(?e, "document rendering did not settle; not ready"),
        });
    }
}

/// The readiness line for weft-appd: `READY <token>` with the session token
/// from `WEFT_READY_TOKEN`, or `READY` when run without appd.
fn ready_line() -> String {
    match std::env::var("WEFT_READY_TOKEN") {
        Ok(token) => format!("READY {token}"),
        Err(_) => "READY".to_owned(),
    }
}

/// The session's application bridge credential from weft-appd.
///
/// Only 32 lowercase hex characters are accepted, so the value can be placed
/// in the injected script without escaping.
fn bridge_token() -> Option<String> {
    let token = std::env::var("WEFT_BRIDGE_TOKEN").ok()?;
    (token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(token)
}

/// The URL prefix covering the application's UI: the directory holding its
/// entry document.
fn ui_scope(entry: &ServoUrl) -> String {
    let url = entry.as_str();
    let end = url.rfind('/').map_or(url.len(), |slash| slash + 1);
    url[..end].to_owned()
}

/// Whether `url` lies inside the UI directory `scope`. Encoded path
/// separators are refused, since a file loader may decode them into a path
/// that leaves the directory.
fn in_scope(url: &str, scope: &str) -> bool {
    url.strip_prefix(scope).is_some_and(|rest| {
        let rest = rest.to_ascii_lowercase();
        !rest.contains("%2f") && !rest.contains("%5c")
    })
}

impl WeftWebViewDelegate {
    /// Whether an application page may load `url`: a file inside its UI
    /// directory, also after resolving symbolic links, an inline (`data:`,
    /// `blob:`) resource, `about:blank`, or the session bridge.
    fn allows(&self, url: &ServoUrl) -> bool {
        match url.scheme() {
            "file" => {
                in_scope(url.as_str(), &self.scope)
                    && self.ui_dir.as_deref().is_some_and(|dir| {
                        url.to_file_path()
                            .ok()
                            .and_then(|path| path.canonicalize().ok())
                            .is_some_and(|path| path.starts_with(dir))
                    })
            }
            "data" | "blob" => true,
            "about" => url.as_str() == "about:blank",
            _ => url.as_str() == self.bridge,
        }
    }
}

/// The UI directory behind the URL prefix `scope`, with symbolic links
/// resolved.
fn ui_dir(scope: &str) -> Option<std::path::PathBuf> {
    ServoUrl::parse(scope)
        .ok()?
        .to_file_path()
        .ok()?
        .canonicalize()
        .ok()
}

/// `value` as a single-quoted JavaScript string literal.
fn js_string(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('\'');
    for c in value.chars() {
        match c {
            ' '..='~' if c != '\'' && c != '\\' => literal.push(c),
            _ => literal.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
        }
    }
    literal.push('\'');
    literal
}

/// The `window.weftIpc` bridge injected into the application document.
///
/// Servo runs user scripts in every document, frames included, so the bridge
/// installs itself only in a top-level document within the application's UI
/// directory `scope`. It authenticates as this session's application bridge and exchanges
/// `APP_MESSAGE` envelopes with weft-appd. `send` accepts a string or a
/// JSON-serialisable value; `onmessage` receives each payload string. The
/// credential stays inside the closure.
fn bridge_script(port: u16, session_id: u64, token: &str, scope: &str) -> String {
    let scope = js_string(scope);
    format!(
        r#"(function () {{
  var rest = location.href.indexOf({scope}) === 0 ? location.href.slice({scope}.length) : null;
  if (window !== window.top || rest === null || /%2f|%5c/i.test(rest)) return;
  var ws = new WebSocket('ws://127.0.0.1:{port}/app');
  var queue = [];
  var open = false;
  function envelope(m) {{
    return JSON.stringify({{ type: 'APP_MESSAGE', payload: typeof m === 'string' ? m : JSON.stringify(m) }});
  }}
  ws.onopen = function () {{
    ws.send(JSON.stringify({{ type: 'HELLO', role: 'app', session_id: {session_id}, token: '{token}' }}));
    open = true;
    queue.forEach(function (m) {{ ws.send(envelope(m)); }});
    queue.length = 0;
  }};
  ws.onmessage = function (e) {{
    var msg;
    try {{ msg = JSON.parse(e.data); }} catch (_) {{ return; }}
    if (msg.type === 'APP_MESSAGE' && window.weftIpc.onmessage) window.weftIpc.onmessage(msg.payload);
  }};
  window.weftSessionId = {session_id};
  window.weftIpc = {{
    send: function (m) {{ if (open) ws.send(envelope(m)); else queue.push(m); }},
    onmessage: null
  }};
}})();"#
    )
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

struct App {
    url: ServoUrl,
    app_id: String,
    session_id: u64,
    ws_port: u16,
    window: Option<Arc<Window>>,
    servo: Option<servo::Servo>,
    webview: Option<servo::WebView>,
    rendering_context: Option<RenderingCtx>,
    signals: Arc<FrameSignals>,
    waker: WeftEventLoopWaker,
    shutting_down: bool,
    ready_signalled: bool,
    modifiers: ModifiersState,
    cursor_pos: DevicePoint,
    shell_client: Option<crate::shell_client::ShellClient>,
}

impl App {
    fn new(
        url: ServoUrl,
        app_id: String,
        session_id: u64,
        ws_port: u16,
        waker: WeftEventLoopWaker,
    ) -> Self {
        Self {
            url,
            app_id,
            session_id,
            ws_port,
            window: None,
            servo: None,
            webview: None,
            rendering_context: None,
            signals: Arc::default(),
            waker,
            shutting_down: false,
            ready_signalled: false,
            modifiers: ModifiersState::default(),
            cursor_pos: DevicePoint::origin(),
            shell_client: None,
        }
    }

    /// Paints and presents once Servo has requested its first repaint.
    ///
    /// Nothing is presented before that request; the first frames can still be
    /// blank or unstyled while the document loads. READY is printed after a
    /// frame is presented once the document has loaded and its rendering has
    /// settled.
    fn render_frame(&mut self) {
        if !self.signals.content.load(Ordering::Relaxed) {
            return;
        }
        let (Some(webview), Some(context)) = (&self.webview, &mut self.rendering_context) else {
            return;
        };
        if let Err(e) = context.paint_and_present(webview) {
            tracing::warn!("frame not presented: {e}");
            return;
        }
        if !self.ready_signalled && self.signals.settled.load(Ordering::Relaxed) {
            self.ready_signalled = true;
            println!("{}", ready_line());
            use std::io::Write;
            let _ = std::io::stdout().flush();
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

        let title = format!("WEFT App — {}", self.url.host_str().unwrap_or("app"));
        let attrs = WindowAttributes::default().with_title(title);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!(error = %e, "failed to create app window; exiting");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.window = Some(Arc::clone(&window));

        if self.shell_client.is_none()
            && let Some((disp, surf)) = wayland_handles(event_loop, &window)
        {
            match crate::shell_client::ShellClient::connect_as_app_with_display(
                &self.app_id,
                self.session_id,
                disp,
                surf,
            ) {
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
            tracing::error!("no rendering context available; exiting without READY signal");
            event_loop.exit();
            return;
        };

        let ucm = Rc::new(UserContentManager::new(&servo));
        if let Some(kit_js) = load_ui_kit_script() {
            ucm.add_script(Rc::new(UserScript::new(kit_js, None)));
        }
        match bridge_token() {
            Some(token) => ucm.add_script(Rc::new(UserScript::new(
                bridge_script(self.ws_port, self.session_id, &token, &ui_scope(&self.url)),
                None,
            ))),
            None => {
                tracing::warn!("WEFT_BRIDGE_TOKEN missing or malformed; app messaging disabled")
            }
        }

        let webview = WebViewBuilder::new(&servo, rendering_context.as_dyn())
            .delegate(Rc::new(WeftWebViewDelegate {
                signals: Arc::clone(&self.signals),
                scope: ui_scope(&self.url),
                ui_dir: ui_dir(&ui_scope(&self.url)),
                bridge: format!("http://127.0.0.1:{}/app", self.ws_port),
            }))
            .user_content_manager(ucm)
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

fn ui_kit_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("WEFT_UI_KIT_JS") {
        return std::path::PathBuf::from(p);
    }
    std::path::PathBuf::from("/usr/share/weft/system/weft-ui-kit.js")
}

fn load_ui_kit_script() -> Option<String> {
    std::fs::read_to_string(ui_kit_path()).ok()
}

#[allow(dead_code)]
fn resolve_weft_system_url(url: &ServoUrl) -> Option<ServoUrl> {
    if url.scheme() != "weft-system" {
        return None;
    }
    let host = url.host_str().unwrap_or("");
    let path = url.path().trim_start_matches('/');
    let system_root = std::env::var("WEFT_SYSTEM_RESOURCES")
        .unwrap_or_else(|_| "/usr/share/weft/system".to_owned());
    let file = std::path::Path::new(&system_root).join(host).join(path);
    ServoUrl::parse(&format!("file://{}", file.display())).ok()
}

/// Shows the UI document `ui` of `app_id`, the file weft-appd resolved from
/// the package; the shell never looks for the package itself.
pub fn run(
    app_id: &str,
    session_id: u64,
    ws_port: u16,
    ui: &std::path::Path,
) -> anyhow::Result<()> {
    let url = ServoUrl::from_file_path(ui)
        .map_err(|()| anyhow::anyhow!("{} is not an absolute path", ui.display()))?;
    tracing::info!(%app_id, %url, "showing application UI");

    let event_loop = EventLoop::<ServoWake>::with_user_event()
        .build()
        .map_err(|e| anyhow::anyhow!("event loop: {e}"))?;

    let waker = WeftEventLoopWaker {
        proxy: Arc::new(Mutex::new(event_loop.create_proxy())),
    };

    let mut app = App::new(url, app_id.to_owned(), session_id, ws_port, waker);
    event_loop
        .run_app(&mut app)
        .map_err(|e| anyhow::anyhow!("event loop run: {e}"))
}
