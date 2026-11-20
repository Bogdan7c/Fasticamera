//! Сборка рисовальщика камеры при присоединении surface: объекты GPU под формат цели и сессия показа.
//! Вынесено из `session` без изменения поведения (владение цепочкой показа остаётся там).

use std::sync::Arc;

use gpu_vulkan::context::GpuContext;

use super::Painter;
use crate::backend::{AttachError, BackendError};
use crate::vulkan_backend::camera::{
    CameraPipelines, CameraSession, CameraSessionFailure, CameraSessionParts, HeadroomBenchParts, ViewfinderStats,
};
use crate::vulkan_backend::config::VulkanBackendConfig;

use super::CameraAttach;

/// Рисовальщик камеры: объекты GPU под формат цели и сессия показа. При отказе статистика
/// видоискателя возвращается владельцу (`camera.stats`), а цель уничтожит вызывающий.
pub(super) fn create_camera_painter(
    context: &GpuContext,
    config: &VulkanBackendConfig,
    info: &gpu_vulkan::swapchain::DisplayTargetInfo,
    camera: CameraAttach<'_>,
) -> Result<Painter, AttachError> {
    let pipelines =
        CameraPipelines::new(context, info, &config.viewfinder.isp, &config.ui_overlay).map_err(|error| {
            AttachError::SwapchainFailed(BackendError {
                operation: "create_camera_pipelines".to_owned(),
                reason: error.to_string(),
            })
        })?;
    let stats = camera
        .stats
        .take()
        .unwrap_or_else(|| ViewfinderStats::new(camera.gpu_time_window_frames));
    let headroom_mode = camera.settings.headroom;
    let session = CameraSession::new(
        context,
        CameraSessionParts {
            frames: Arc::clone(&camera.feed.frames),
            make_importer: &camera.feed.make_importer,
            history_capacity: config.camera_present_history_capacity,
            settings: camera.settings,
            viewfinder: stats,
            awb_feed: camera.awb_feed.clone(),
            white_balance: camera.white_balance.clone(),
            auto_exposure: camera.auto_exposure.clone(),
            headroom: HeadroomBenchParts {
                config: config.viewfinder.headroom_bench,
                frame_slots: info.frames_in_flight,
                mode: headroom_mode,
            },
            tone: config.viewfinder.tone,
            continuity: config.viewfinder.continuity,
        },
    )
    .map_err(|failure| {
        let CameraSessionFailure { error, viewfinder } = *failure;
        *camera.stats = Some(viewfinder);
        AttachError::SwapchainFailed(BackendError {
            operation: "create_camera_session".to_owned(),
            reason: error.to_string(),
        })
    })?;
    Ok(Painter::Camera {
        pipelines: Box::new(pipelines),
        session: Box::new(session),
    })
}
