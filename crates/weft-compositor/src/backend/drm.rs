// Non-Linux: DRM/KMS backend is unavailable; callers must use --winit.
#[cfg(not(target_os = "linux"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("DRM/KMS backend requires Linux; pass --winit for development on other platforms")
}

#[cfg(target_os = "linux")]
use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use anyhow::Context;

#[cfg(target_os = "linux")]
use smithay::{
    backend::{
        allocator::{
            Fourcc, Modifier,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{
                Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
            multigpu::{GpuManager, MultiRenderer, gbm::GbmGlesBackend},
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::space::SpaceRenderElements,
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::{Mode as WlMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        drm::control::{ModeTypeFlags, connector, crtc},
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::{DeviceFd, Scale, Transform},
    wayland::{compositor::with_states, socket::ListeningSocketSource},
};

#[cfg(target_os = "linux")]
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

#[cfg(target_os = "linux")]
use crate::{
    appd_ipc::{self, WeftAppdIpc},
    input,
    state::{WeftClientState, WeftCompositorState},
};

#[cfg(target_os = "linux")]
use super::drm_device::{WeftDrmData, WeftDrmDevice, WeftOutputSurface};

#[cfg(target_os = "linux")]
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

#[cfg(target_os = "linux")]
const SUPPORTED_FORMATS_8BIT_ONLY: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

#[cfg(target_os = "linux")]
#[allow(dead_code)]
type WeftMultiRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

#[cfg(target_os = "linux")]
pub fn run() -> anyhow::Result<()> {
    let mut event_loop: EventLoop<'static, WeftCompositorState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let display = Display::<WeftCompositorState>::new()?;
    let display_handle = display.handle();

    let (session, session_notifier) =
        LibSeatSession::new().context("failed to create libseat session")?;

    let seat_name = session.seat().to_owned();
    let loop_signal = event_loop.get_signal();

    let primary_gpu_node = if let Ok(var) = std::env::var("WEFT_DRM_DEVICE") {
        DrmNode::from_path(var).context("invalid WEFT_DRM_DEVICE path")?
    } else {
        primary_gpu(&seat_name)
            .ok()
            .flatten()
            .and_then(|p| {
                DrmNode::from_path(p)
                    .ok()?
                    .node_with_type(NodeType::Render)?
                    .ok()
            })
            .or_else(|| {
                all_gpus(&seat_name)
                    .unwrap_or_default()
                    .into_iter()
                    .find_map(|p| DrmNode::from_path(p).ok())
            })
            .context("no GPU found")?
    };
    tracing::info!(?primary_gpu_node, "primary GPU");

    let gpu_manager: super::drm_device::WeftGpuManager =
        GpuManager::new(Default::default()).context("failed to create GPU manager")?;

    let listening_socket =
        ListeningSocketSource::new_auto().context("failed to create Wayland socket")?;
    let socket_name = listening_socket.socket_name().to_os_string();
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };
    tracing::info!(?socket_name, "Wayland socket open");

    loop_handle
        .insert_source(listening_socket, |stream, _, state| {
            state
                .display_handle
                .insert_client(stream, Arc::new(WeftClientState::default()))
                .unwrap();
        })
        .map_err(|e| anyhow::anyhow!("socket source: {e}"))?;

    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: Display is owned by this Generic source and outlives the event loop.
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("display source: {e}"))?;

    let udev_backend = UdevBackend::new(&seat_name).context("failed to create udev backend")?;

    let mut libinput_ctx =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_ctx
        .udev_assign_seat(&seat_name)
        .map_err(|_| anyhow::anyhow!("libinput seat assignment failed"))?;
    let libinput_backend = LibinputInputBackend::new(libinput_ctx);

    loop_handle
        .insert_source(
            libinput_backend,
            move |mut event, _, state: &mut WeftCompositorState| {
                if let InputEvent::DeviceAdded { device } = &mut event
                    && device.has_capability(DeviceCapability::Keyboard)
                {
                    if let Some(led) = state.seat.get_keyboard().map(|k| k.led_state()) {
                        device.led_update(led.into());
                    }
                    if let Some(drm) = state.drm.as_mut() {
                        drm.keyboards.push(device.clone());
                    }
                }
                input::process_input_event(state, event);
            },
        )
        .map_err(|e| anyhow::anyhow!("libinput source: {e}"))?;

    loop_handle
        .insert_source(
            session_notifier,
            move |event, &mut (), state: &mut WeftCompositorState| match event {
                SessionEvent::PauseSession => {
                    if let Some(drm) = state.drm.as_mut() {
                        for dev in drm.devices.values_mut() {
                            dev.drm_output_manager.pause();
                        }
                    }
                }
                SessionEvent::ActivateSession => {
                    tracing::info!("session activated");
                }
            },
        )
        .map_err(|e| anyhow::anyhow!("session notifier: {e}"))?;

    loop_handle
        .insert_source(
            udev_backend,
            move |event, _, state: &mut WeftCompositorState| match event {
                UdevEvent::Added { device_id: _, path } => {
                    let node = match DrmNode::from_path(&path) {
                        Ok(n) => n,
                        Err(e) => {
                            tracing::warn!(?e, "failed to build DRM node");
                            return;
                        }
                    };
                    if let Err(e) = device_added(state, node, &path) {
                        tracing::warn!(?e, "failed to add DRM device");
                    }
                }
                UdevEvent::Changed { device_id } => {
                    let node = state
                        .drm
                        .as_ref()
                        .and_then(|d| d.devices.keys().find(|n| n.dev_id() == device_id).copied());
                    if let Some(node) = node {
                        device_changed(state, node);
                    }
                }
                UdevEvent::Removed { device_id } => {
                    let node = state
                        .drm
                        .as_ref()
                        .and_then(|d| d.devices.keys().find(|n| n.dev_id() == device_id).copied());
                    if let Some(node) = node {
                        device_removed(state, node);
                    }
                }
            },
        )
        .map_err(|e| anyhow::anyhow!("udev source: {e}"))?;

    let mut state =
        WeftCompositorState::new(display_handle.clone(), loop_signal, loop_handle, seat_name);

    state.appd_ipc = Some(WeftAppdIpc::new(appd_ipc::compositor_socket_path()));
    if let Err(e) = appd_ipc::setup(&mut state) {
        tracing::warn!(?e, "compositor IPC setup failed");
    }

    state.drm = Some(WeftDrmData {
        session,
        primary_gpu: primary_gpu_node,
        gpu_manager,
        devices: HashMap::new(),
        keyboards: Vec::new(),
        display_handle,
        start_time: Instant::now(),
    });

    let existing: Vec<(DrmNode, std::path::PathBuf)> = state
        .drm
        .as_ref()
        .map(|d| {
            all_gpus(d.session.seat())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| DrmNode::from_path(&p).ok().map(|n| (n, p)))
                .collect()
        })
        .unwrap_or_default();

    for (node, path) in existing {
        if let Err(e) = device_added(&mut state, node, &path) {
            tracing::warn!(?e, ?node, "startup device_added failed");
        }
    }

    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);

    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn device_added(state: &mut WeftCompositorState, node: DrmNode, path: &Path) -> anyhow::Result<()> {
    let drm_data = state.drm.as_mut().context("DRM data not initialised")?;

    let fd = drm_data
        .session
        .open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .context("failed to open DRM device")?;

    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true).context("DrmDevice::new failed")?;
    let gbm = GbmDevice::new(fd).context("GbmDevice::new failed")?;

    let render_node = (|| -> anyhow::Result<DrmNode> {
        // Safety: EGLDisplay requires the GBM device to outlive it; gbm lives in WeftDrmDevice.
        let egl_display =
            unsafe { EGLDisplay::new(gbm.clone()).context("EGLDisplay::new failed")? };
        let egl_device = EGLDevice::device_for_display(&egl_display).context("no EGL device")?;
        if egl_device.is_software() {
            anyhow::bail!("software renderer");
        }
        let rn = egl_device
            .try_get_render_node()
            .ok()
            .flatten()
            .unwrap_or(node);
        drm_data
            .gpu_manager
            .as_mut()
            .add_node(rn, gbm.clone())
            .map_err(|e| anyhow::anyhow!("add_node: {e:?}"))?;
        Ok(rn)
    })()
    .map_err(|e| {
        tracing::warn!(?e, "EGL init failed; output may render black");
        e
    })
    .ok();

    let effective_gpu = render_node.unwrap_or(drm_data.primary_gpu);

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );

    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);

    let color_formats = if std::env::var("WEFT_DISABLE_10BIT").is_ok() {
        SUPPORTED_FORMATS_8BIT_ONLY
    } else {
        SUPPORTED_FORMATS
    };

    let render_formats: FormatSet = drm_data
        .gpu_manager
        .single_renderer(&effective_gpu)
        .map(|r| {
            r.as_ref()
                .egl_context()
                .dmabuf_render_formats()
                .iter()
                .filter(|f| render_node.is_some() || f.modifier == Modifier::Linear)
                .copied()
                .collect::<FormatSet>()
        })
        .unwrap_or_default();

    let drm_output_manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        color_formats.iter().copied(),
        render_formats,
    );

    let registration_token = state
        .loop_handle
        .insert_source(
            notifier,
            move |event, _metadata, data: &mut WeftCompositorState| match event {
                DrmEvent::VBlank(crtc) => render_output(data, node, crtc),
                DrmEvent::Error(e) => tracing::error!(?e, "DRM error"),
            },
        )
        .map_err(|e| anyhow::anyhow!("DRM notifier: {e}"))?;

    state.drm.as_mut().unwrap().devices.insert(
        node,
        WeftDrmDevice {
            drm_output_manager,
            drm_scanner: DrmScanner::new(),
            surfaces: HashMap::new(),
            render_node,
            registration_token,
        },
    );

    device_changed(state, node);
    Ok(())
}

#[cfg(target_os = "linux")]
fn device_changed(state: &mut WeftCompositorState, node: DrmNode) {
    let drm_data = match state.drm.as_mut() {
        Some(d) => d,
        None => return,
    };
    let device = match drm_data.devices.get_mut(&node) {
        Some(d) => d,
        None => return,
    };

    let events: Vec<DrmScanEvent> = match device
        .drm_scanner
        .scan_connectors(device.drm_output_manager.device())
    {
        Ok(r) => r.into_iter().collect(),
        Err(e) => {
            tracing::warn!(?e, "connector scan failed");
            return;
        }
    };

    for event in events {
        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => connector_connected(state, node, connector, crtc),
            DrmScanEvent::Disconnected {
                connector,
                crtc: Some(crtc),
            } => connector_disconnected(state, node, connector, crtc),
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn connector_connected(
    state: &mut WeftCompositorState,
    node: DrmNode,
    connector: connector::Info,
    crtc: crtc::Handle,
) {
    let name = format!("{:?}-{}", connector.interface(), connector.interface_id());

    let mode = match connector
        .modes()
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .copied()
        .or_else(|| connector.modes().first().copied())
    {
        Some(m) => m,
        None => {
            tracing::warn!(?name, "connector has no modes");
            return;
        }
    };

    let wl_mode = WlMode {
        size: (mode.size().0 as i32, mode.size().1 as i32).into(),
        refresh: mode.vrefresh() as i32 * 1000,
    };

    let output = Output::new(
        name.clone(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Unknown".to_string(),
            model: name.clone(),
        },
    );

    let drm_data = match state.drm.as_mut() {
        Some(d) => d,
        None => return,
    };

    let global = output.create_global::<WeftCompositorState>(&drm_data.display_handle);
    output.change_current_state(
        Some(wl_mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(wl_mode);

    let render_node = drm_data
        .devices
        .get(&node)
        .and_then(|d| d.render_node)
        .unwrap_or(drm_data.primary_gpu);

    let planes = drm_data
        .devices
        .get_mut(&node)
        .and_then(|d| d.drm_output_manager.device().planes(&crtc).ok());

    let WeftDrmData {
        ref mut gpu_manager,
        ref mut devices,
        ..
    } = *drm_data;

    let device = match devices.get_mut(&node) {
        Some(d) => d,
        None => return,
    };

    let mut renderer = match gpu_manager.single_renderer(&render_node) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(?e, "no renderer for output init");
            return;
        }
    };

    let drm_output = match device
        .drm_output_manager
        .initialize_output::<_, WaylandSurfaceRenderElement<_>>(
            crtc,
            mode,
            &[connector.handle()],
            &output,
            planes,
            &mut renderer,
            &DrmOutputRenderElements::default(),
        ) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(?e, ?name, "initialize_output failed");
            return;
        }
    };

    device.surfaces.insert(
        crtc,
        WeftOutputSurface {
            output: output.clone(),
            drm_output,
            device_id: node,
            global,
        },
    );

    state.space.map_output(&output, (0, 0));
    tracing::info!(?name, "output connected");
    render_output(state, node, crtc);
}

#[cfg(target_os = "linux")]
fn connector_disconnected(
    state: &mut WeftCompositorState,
    node: DrmNode,
    _connector: connector::Info,
    crtc: crtc::Handle,
) {
    let drm_data = match state.drm.as_mut() {
        Some(d) => d,
        None => return,
    };
    if let Some(device) = drm_data.devices.get_mut(&node)
        && let Some(surface) = device.surfaces.remove(&crtc)
    {
        state.space.unmap_output(&surface.output);
    }
}

#[cfg(target_os = "linux")]
fn device_removed(state: &mut WeftCompositorState, node: DrmNode) {
    let drm_data = match state.drm.as_mut() {
        Some(d) => d,
        None => return,
    };
    if let Some(device) = drm_data.devices.remove(&node) {
        state.loop_handle.remove(device.registration_token);
        for surface in device.surfaces.into_values() {
            state.space.unmap_output(&surface.output);
        }
    }
}

#[cfg(target_os = "linux")]
fn render_output(state: &mut WeftCompositorState, node: DrmNode, crtc: crtc::Handle) {
    let output = {
        let drm_data = match state.drm.as_ref() {
            Some(d) => d,
            None => return,
        };
        match drm_data
            .devices
            .get(&node)
            .and_then(|d| d.surfaces.get(&crtc))
        {
            Some(s) => s.output.clone(),
            None => return,
        }
    };

    let render_node = {
        let d = state.drm.as_ref().unwrap();
        d.devices
            .get(&node)
            .and_then(|d| d.render_node)
            .unwrap_or(d.primary_gpu)
    };

    let elapsed = state
        .drm
        .as_ref()
        .map(|d| d.start_time.elapsed())
        .unwrap_or_default();

    let output_geo = state.space.output_geometry(&output).unwrap_or_default();
    let pointer_location = state.pointer_location;
    let cursor_status = state.cursor_image_status.clone();

    {
        let WeftCompositorState {
            ref mut drm,
            ref space,
            ..
        } = *state;

        let drm_data = match drm.as_mut() {
            Some(d) => d,
            None => return,
        };

        let WeftDrmData {
            ref mut gpu_manager,
            ref mut devices,
            ..
        } = *drm_data;

        let device = match devices.get_mut(&node) {
            Some(d) => d,
            None => return,
        };
        let surface = match device.surfaces.get_mut(&crtc) {
            Some(s) => s,
            None => return,
        };

        let mut renderer = match gpu_manager.single_renderer(&render_node) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(?e, "renderer unavailable");
                return;
            }
        };

        let output_scale = output.current_scale().fractional_scale();

        let mut elements: Vec<SpaceRenderElements<_, WaylandSurfaceRenderElement<_>>> =
            if let CursorImageStatus::Surface(ref cursor_surface) = cursor_status {
                let hotspot = with_states(cursor_surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .and_then(|d| d.lock().ok().map(|g| g.hotspot))
                        .unwrap_or_default()
                });
                let cursor_pos = (pointer_location - output_geo.loc.to_f64() - hotspot.to_f64())
                    .to_physical_precise_round(output_scale);
                render_elements_from_surface_tree(
                    &mut renderer,
                    cursor_surface,
                    cursor_pos,
                    Scale::from(output_scale),
                    1.0,
                    Kind::Cursor,
                )
            } else {
                Vec::new()
            };

        elements.extend(
            space
                .render_elements_for_output(&mut renderer, &output, 1.0)
                .unwrap_or_default(),
        );

        match surface.drm_output.render_frame(
            &mut renderer,
            &elements,
            [0.08_f32, 0.08, 0.08, 1.0],
            FrameFlags::DEFAULT,
        ) {
            Ok(result) if !result.is_empty => {
                if let Err(e) = surface.drm_output.queue_frame(None) {
                    tracing::warn!(?e, "queue_frame failed");
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(?e, "render_frame failed"),
        }
    }

    state.space.elements().for_each(|window| {
        window.send_frame(&output, elapsed, Some(Duration::from_millis(16)), |_, _| {
            Some(output.clone())
        });
    });
    state.space.refresh();
    state.popups.cleanup();
    let _ = state.display_handle.flush_clients();
}
