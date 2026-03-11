#![cfg(feature = "servo-embed")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use servo::{
    EventLoopWaker, InputEvent, MouseButton as ServoMouseButton, MouseButtonAction,
    MouseButtonEvent, MouseMoveEvent, ServoBuilder, ServoDelegate, ServoUrl, UserContentManager,
    UserScript, WebViewBuilder, WebViewDelegate,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
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

struct WeftWebViewDelegate {
    redraw_requested: Arc<std::sync::atomic::AtomicBool>,
}

impl WebViewDelegate for WeftWebViewDelegate {
    fn notify_new_frame_ready(&self, _webview: servo::WebView) {
        self.redraw_requested
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── Rendering context abstraction (GAP-2) ───────────────────────────────────

enum RenderingCtx {
    Software(Rc<servo::SoftwareRenderingContext>),
    Egl(Rc<servo::WindowRenderingContext>),
}

impl RenderingCtx {
    fn as_dyn(&self) -> Rc<dyn servo::RenderingContext> {
        match self {
            Self::Software(rc) => Rc::clone(rc) as Rc<dyn servo::RenderingContext>,
            Self::Egl(rc) => Rc::clone(rc) as Rc<dyn servo::RenderingContext>,
        }
    }
}

// ── Application state ─────────────────────────────────────────────────────────

struct App {
    url: ServoUrl,
    ws_port: u16,
    window: Option<Arc<Window>>,
    servo: Option<servo::Servo>,
    webview: Option<servo::WebView>,
    rendering_context: Option<RenderingCtx>,
    app_webviews: HashMap<u64, servo::WebView>,
    active_session: Option<u64>,
    app_rx: mpsc::Receiver<crate::appd_ws::AppdCmd>,
    redraw_requested: Arc<std::sync::atomic::AtomicBool>,
    waker: WeftEventLoopWaker,
    shutting_down: bool,
    modifiers: ModifiersState,
    cursor_pos: servo::euclid::default::Point2D<f32>,
    shell_client: Option<crate::shell_client::ShellClient>,
}

impl App {
    fn new(
        url: ServoUrl,
        waker: WeftEventLoopWaker,
        ws_port: u16,
        app_rx: mpsc::Receiver<crate::appd_ws::AppdCmd>,
        shell_client: Option<crate::shell_client::ShellClient>,
    ) -> Self {
        Self {
            url,
            ws_port,
            window: None,
            servo: None,
            webview: None,
            rendering_context: None,
            app_webviews: HashMap::new(),
            active_session: None,
            app_rx,
            redraw_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            waker,
            shutting_down: false,
            modifiers: ModifiersState::default(),
            cursor_pos: servo::euclid::default::Point2D::zero(),
            shell_client,
        }
    }

    fn active_webview(&self) -> Option<&servo::WebView> {
        self.active_session
            .and_then(|sid| self.app_webviews.get(&sid))
            .or(self.webview.as_ref())
    }

    fn create_app_webview(&mut self, session_id: u64, app_id: &str) {
        if self.app_webviews.contains_key(&session_id) {
            return;
        }
        let (Some(servo), Some(rc)) = (&self.servo, &self.rendering_context) else {
            return;
        };
        let url_str = format!("weft-app://{app_id}/index.html");
        let raw = match ServoUrl::parse(&url_str) {
            Ok(u) => u,
            Err(_) => return,
        };
        let url = resolve_weft_app_url(&raw).unwrap_or(raw);
        let ucm = Rc::new(UserContentManager::new(servo));
        let bridge_js = format!(
            r#"(function(){{var ws=new WebSocket('ws://127.0.0.1:{p}');var sid={sid};var q=[];var r=false;ws.onopen=function(){{r=true;q.forEach(function(m){{ws.send(JSON.stringify(m))}});q.length=0}};window.weftSessionId=sid;window.weftIpc={{send:function(m){{if(r)ws.send(JSON.stringify(m));else q.push(m)}},onmessage:null}};ws.onmessage=function(e){{if(window.weftIpc.onmessage)window.weftIpc.onmessage(JSON.parse(e.data))}}}})()"#,
            p = self.ws_port,
            sid = session_id,
        );
        ucm.add_script(Rc::new(UserScript::new(bridge_js, None)));
        let wv = WebViewBuilder::new(servo, rc.as_dyn())
            .delegate(Rc::new(WeftWebViewDelegate {
                redraw_requested: Arc::clone(&self.redraw_requested),
            }))
            .user_content_manager(ucm)
            .url(url)
            .build();
        self.app_webviews.insert(session_id, wv);
        self.active_session = Some(session_id);
    }

    fn render_frame(window: &Arc<Window>, ctx: &RenderingCtx) {
        match ctx {
            RenderingCtx::Software(rc) => Self::blit_software(window, rc),
            RenderingCtx::Egl(_) => {}
        }
    }

    fn blit_software(window: &Arc<Window>, rendering_context: &servo::SoftwareRenderingContext) {
        let size = window.inner_size();
        let Some(pixels) = rendering_context.read_pixels() else {
            return;
        };
        let ctx = softbuffer::Context::new(Arc::clone(window)).expect("softbuffer context");
        let mut surface =
            softbuffer::Surface::new(&ctx, Arc::clone(window)).expect("softbuffer surface");
        let _ = surface.resize(
            std::num::NonZeroU32::new(size.width).unwrap_or(std::num::NonZeroU32::new(1).unwrap()),
            std::num::NonZeroU32::new(size.height).unwrap_or(std::num::NonZeroU32::new(1).unwrap()),
        );
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };
        for (dst, src) in buf.iter_mut().zip(pixels.chunks(4)) {
            *dst = u32::from_be_bytes([0, src[0], src[1], src[2]]);
        }
        let _ = buf.present();
    }
}

impl ApplicationHandler<ServoWake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = WindowAttributes::default().with_title("WEFT Shell");
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create shell window"),
        );
        let size = window.inner_size();
        self.window = Some(Arc::clone(&window));

        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(self.waker.clone()))
            .build();

        servo.set_delegate(Rc::new(WeftServoDelegate));

        let rendering_context = build_rendering_ctx(event_loop, &window, size);

        let user_content_manager = Rc::new(UserContentManager::new(&servo));
        let bridge_js = format!(
            r#"(function(){{var ws=new WebSocket('ws://127.0.0.1:{p}');var q=[];var r=false;ws.onopen=function(){{r=true;q.forEach(function(m){{ws.send(JSON.stringify(m))}});q.length=0}};window.weftIpc={{send:function(m){{if(r)ws.send(JSON.stringify(m));else q.push(m)}},onmessage:null}};ws.onmessage=function(e){{if(window.weftIpc.onmessage)window.weftIpc.onmessage(JSON.parse(e.data))}}}})()"#,
            p = self.ws_port
        );
        user_content_manager.add_script(Rc::new(UserScript::new(bridge_js, None)));

        let webview = WebViewBuilder::new(&servo, rendering_context.as_dyn())
            .delegate(Rc::new(WeftWebViewDelegate {
                redraw_requested: Arc::clone(&self.redraw_requested),
            }))
            .user_content_manager(Rc::clone(&user_content_manager))
            .url(self.url.clone())
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
        if let Some(sc) = &mut self.shell_client {
            match sc.dispatch_pending() {
                Ok(false) => {
                    self.shutting_down = true;
                    if let Some(servo) = &self.servo {
                        servo.start_shutting_down();
                    }
                }
                Err(e) => tracing::warn!("shell client dispatch error: {e}"),
                Ok(true) => {}
            }
        }
        while let Ok(cmd) = self.app_rx.try_recv() {
            match cmd {
                crate::appd_ws::AppdCmd::Launch { session_id, app_id } => {
                    self.create_app_webview(session_id, &app_id);
                }
                crate::appd_ws::AppdCmd::Stop { session_id } => {
                    self.app_webviews.remove(&session_id);
                    if self.active_session == Some(session_id) {
                        self.active_session = None;
                    }
                }
                crate::appd_ws::AppdCmd::SyncSessions { active } => {
                    let active_ids: std::collections::HashSet<u64> =
                        active.iter().map(|(sid, _)| *sid).collect();
                    self.app_webviews.retain(|sid, _| active_ids.contains(sid));
                    if let Some(cur) = self.active_session {
                        if !active_ids.contains(&cur) {
                            self.active_session = None;
                        }
                    }
                    for (session_id, app_id) in active {
                        self.create_app_webview(session_id, &app_id);
                    }
                }
            }
        }
        if let Some(servo) = &self.servo {
            servo.spin_event_loop();
        }
        if self
            .redraw_requested
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => {
                if let (Some(window), Some(servo)) = (&self.window, &self.servo) {
                    if let Some(rc) = &self.rendering_context {
                        Self::render_frame(window, rc);
                    }
                    servo.spin_event_loop();
                }
            }
            WindowEvent::Resized(new_size) => {
                let sz = servo::euclid::Size2D::new(new_size.width, new_size.height);
                if let Some(wv) = &self.webview {
                    wv.resize(sz);
                }
                for wv in self.app_webviews.values() {
                    wv.resize(sz);
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(wv) = self.active_webview() {
                    let ev = super::keyutils::keyboard_event_from_winit(&event, self.modifiers);
                    let _ = wv.notify_input_event(InputEvent::Keyboard(ev));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pt = servo::euclid::default::Point2D::new(position.x as f32, position.y as f32);
                self.cursor_pos = pt;
                if let Some(wv) = self.active_webview() {
                    let _ = wv.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(pt)));
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
                    ElementState::Pressed => MouseButtonAction::Click,
                    ElementState::Released => MouseButtonAction::Up,
                };
                if let Some(wv) = self.active_webview() {
                    let _ = wv.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        btn,
                        self.cursor_pos.cast_unit(),
                    )));
                }
            }
            WindowEvent::CloseRequested => {
                self.shutting_down = true;
                if let Some(servo) = &self.servo {
                    servo.start_shutting_down();
                }
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
    size: winit::dpi::PhysicalSize<u32>,
) -> RenderingCtx {
    if std::env::var_os("WEFT_EGL_RENDERING").is_some() {
        let display_handle = event_loop.display_handle();
        let window_handle = window.window_handle();
        if let (Ok(dh), Ok(wh)) = (display_handle, window_handle) {
            match servo::WindowRenderingContext::new(dh, wh, size) {
                Ok(rc) => {
                    tracing::info!("using EGL rendering context");
                    return RenderingCtx::Egl(Rc::new(rc));
                }
                Err(e) => {
                    tracing::warn!("EGL rendering context failed ({e}), falling back to software");
                }
            }
        }
    }
    RenderingCtx::Software(Rc::new(
        servo::SoftwareRenderingContext::new(servo::euclid::Size2D::new(size.width, size.height))
            .expect("SoftwareRenderingContext"),
    ))
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

pub fn run(
    html_path: &Path,
    ws_port: u16,
    shell_client: Option<crate::shell_client::ShellClient>,
) -> anyhow::Result<()> {
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

    let (app_tx, app_rx) = mpsc::sync_channel::<crate::appd_ws::AppdCmd>(64);
    let waker_for_thread = waker.clone();
    crate::appd_ws::spawn_appd_listener(ws_port, app_tx, Box::new(move || waker_for_thread.wake()));

    let mut app = App::new(url, waker, ws_port, app_rx, shell_client);
    event_loop
        .run_app(&mut app)
        .map_err(|e| anyhow::anyhow!("event loop run: {e}"))
}
