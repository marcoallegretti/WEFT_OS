use std::{collections::HashMap, time::Instant};

use smithay::{
    backend::{
        allocator::gbm::GbmAllocator,
        drm::{
            DrmDeviceFd, DrmNode,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager},
        },
        renderer::{
            gles::GlesRenderer,
            multigpu::{GpuManager, gbm::GbmGlesBackend},
        },
        session::libseat::LibSeatSession,
    },
    desktop::utils::OutputPresentationFeedback,
    output::Output,
    reexports::{
        calloop::RegistrationToken,
        drm::control::crtc,
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
};
use smithay_drm_extras::drm_scanner::DrmScanner;

pub type WeftAllocator = GbmAllocator<DrmDeviceFd>;
pub type WeftExporter = GbmFramebufferExporter<DrmDeviceFd>;
pub type WeftDrmOutput =
    DrmOutput<WeftAllocator, WeftExporter, Option<OutputPresentationFeedback>, DrmDeviceFd>;
pub type WeftDrmOutputManager =
    DrmOutputManager<WeftAllocator, WeftExporter, Option<OutputPresentationFeedback>, DrmDeviceFd>;
pub type WeftGpuManager = GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>;

#[allow(dead_code)]
pub struct WeftOutputSurface {
    pub output: Output,
    pub drm_output: WeftDrmOutput,
    pub device_id: DrmNode,
    pub global: GlobalId,
}

pub struct WeftDrmDevice {
    pub drm_output_manager: WeftDrmOutputManager,
    pub drm_scanner: DrmScanner,
    pub surfaces: HashMap<crtc::Handle, WeftOutputSurface>,
    pub render_node: Option<DrmNode>,
    pub registration_token: RegistrationToken,
}

pub struct WeftDrmData {
    pub session: LibSeatSession,
    pub primary_gpu: DrmNode,
    pub gpu_manager: WeftGpuManager,
    pub devices: HashMap<DrmNode, WeftDrmDevice>,
    pub keyboards: Vec<smithay::reexports::input::Device>,
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
}
