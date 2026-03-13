#![cfg(feature = "servo-embed")]

use std::path::PathBuf;
use std::rc::Rc;
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
    platform::wayland::{ActiveEventLoopExtWayland, WindowExtWayland},
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
}

struct WeftWebViewDelegate {
    redraw_requested: Arc<std::sync::atomic::AtomicBool>,
}

impl WebViewDelegate for WeftWebViewDelegate {
    fn notify_new_frame_ready(&self, _webview: servo::WebView) {
        self.redraw_requested
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

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

struct App {
    url: ServoUrl,
    app_id: String,
    session_id: u64,
    ws_port: u16,
    window: Option<Arc<Window>>,
    servo: Option<servo::Servo>,
    webview: Option<servo::WebView>,
    rendering_context: Option<RenderingCtx>,
    redraw_requested: Arc<std::sync::atomic::AtomicBool>,
    waker: WeftEventLoopWaker,
    shutting_down: bool,
    ready_signalled: bool,
    modifiers: ModifiersState,
    cursor_pos: servo::euclid::default::Point2D<f32>,
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
            redraw_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            waker,
            shutting_down: false,
            ready_signalled: false,
            modifiers: ModifiersState::default(),
            cursor_pos: servo::euclid::default::Point2D::zero(),
            shell_client: None,
        }
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

        let title = format!("WEFT App — {}", self.url.host_str().unwrap_or("app"));
        let attrs = WindowAttributes::default().with_title(title);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create app window"),
        );
        let size = window.inner_size();
        self.window = Some(Arc::clone(&window));

        if self.shell_client.is_none() {
            if let (Some(disp), Some(surf)) =
                (event_loop.wayland_display(), window.wayland_surface())
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
        }

        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(self.waker.clone()))
            .build();

        servo.set_delegate(Rc::new(WeftServoDelegate));

        let rendering_context = build_rendering_ctx(event_loop, &window, size);

        let ucm = Rc::new(UserContentManager::new(&servo));
        if let Some(kit_js) = load_ui_kit_script() {
            ucm.add_script(Rc::new(UserScript::new(kit_js, None)));
        }
        let bridge_js = format!(
            r#"(function(){{var ws=new WebSocket('ws://127.0.0.1:{p}');var sid={sid};var q=[];var r=false;ws.onopen=function(){{r=true;q.forEach(function(m){{ws.send(JSON.stringify(m))}});q.length=0}};window.weftSessionId=sid;window.weftIpc={{send:function(m){{if(r)ws.send(JSON.stringify(m));else q.push(m)}},onmessage:null}};ws.onmessage=function(e){{if(window.weftIpc.onmessage)window.weftIpc.onmessage(JSON.parse(e.data))}}}})()"#,
            p = self.ws_port,
            sid = self.session_id,
        );
        ucm.add_script(Rc::new(UserScript::new(bridge_js, None)));

        let webview = WebViewBuilder::new(&servo, rendering_context.as_dyn())
            .delegate(Rc::new(WeftWebViewDelegate {
                redraw_requested: Arc::clone(&self.redraw_requested),
            }))
            .user_content_manager(ucm)
            .url(self.url.clone())
            .build();

        self.servo = Some(servo);
        self.webview = Some(webview);
        self.rendering_context = Some(rendering_context);

        if !self.ready_signalled {
            self.ready_signalled = true;
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
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
                let pt = servo::euclid::default::Point2D::new(position.x as f32, position.y as f32);
                self.cursor_pos = pt;
                if let Some(wv) = &self.webview {
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
                if let Some(wv) = &self.webview {
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

fn read_ui_entry(app_id: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Ui {
        entry: String,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        ui: Ui,
    }

    let erofs_manifest = std::path::Path::new("/run/weft/apps")
        .join(app_id)
        .join("merged")
        .join("wapp.toml");
    let toml_text = std::fs::read_to_string(&erofs_manifest).ok().or_else(|| {
        app_store_roots()
            .into_iter()
            .find_map(|r| std::fs::read_to_string(r.join(app_id).join("wapp.toml")).ok())
    })?;
    let m: Manifest = toml::from_str(&toml_text).ok()?;
    Some(m.ui.entry)
}

fn resolve_app_file_path(app_id: &str, rel: &str) -> Option<std::path::PathBuf> {
    let erofs_root = std::path::Path::new("/run/weft/apps")
        .join(app_id)
        .join("merged");
    if erofs_root.exists() {
        let p = erofs_root.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    app_store_roots()
        .into_iter()
        .map(|r| r.join(app_id).join(rel))
        .find(|p| p.exists())
}

fn resolve_weft_app_url(app_id: &str) -> Option<ServoUrl> {
    let entry = read_ui_entry(app_id).unwrap_or_else(|| "ui/index.html".to_owned());
    let file_path = resolve_app_file_path(app_id, &entry)?;
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

pub fn run(app_id: &str, session_id: u64, ws_port: u16) -> anyhow::Result<()> {
    let url = resolve_weft_app_url(app_id)
        .ok_or_else(|| anyhow::anyhow!("no ui/index.html found for app {app_id}"))?;

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
