//! Оркестрация теста: шаги по порядку, отчёт по каждому шагу, гарантированный teardown.

use super::instance::{PresentDevice, SurfaceInstance};
use super::policy::RenderParams;
use super::render_loop::run_render_loop;
use super::report::{CodecSurfaceReport, SwapchainReport, TeardownReport};
use super::surface_info::query_surface;
use super::swapchain::Rig;
use super::window::NativeWindow;
use crate::error::ProbeError;
use serde_json::{Map, Value};

/// Все живые ресурсы теста. Порядок полей = порядок уничтожения при `Drop`
/// (в т.ч. при панике): swapchain → устройство → instance/surface → окно.
#[derive(Default)]
struct Session {
    rig: Option<Rig>,
    device: Option<PresentDevice>,
    host: Option<SurfaceInstance>,
    window: Option<NativeWindow>,
}

impl Session {
    /// Явное освобождение в безопасном порядке; ошибки ожидания простоя собираются в отчёт.
    fn teardown(mut self) -> TeardownReport {
        let mut errors = Vec::new();
        if let Some(rig) = self.rig.take() {
            errors.extend(rig.shutdown().err());
        }
        if let Some(device) = self.device.take() {
            errors.extend(device.shutdown().err());
        }
        self.host = None;
        self.window = None;
        TeardownReport {
            ok: errors.is_empty(),
            errors,
        }
    }
}

/// Отчёт о расширениях: что нашли и включили.
fn extensions_report(host: &SurfaceInstance, device: &PresentDevice) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("VK_KHR_surface".into(), true.into());
    map.insert("VK_KHR_android_surface".into(), true.into());
    map.insert("VK_EXT_swapchain_colorspace".into(), host.has_colorspace_ext.into());
    map.insert("VK_KHR_swapchain".into(), true.into());
    map.insert("VK_GOOGLE_display_timing".into(), device.has_display_timing.into());
    map.insert("dedicated_instance_and_device".into(), true.into());
    map.insert("device_name".into(), device.device_name.clone().into());
    map.insert("device_extension_count".into(), device.extensions.len().into());
    map
}

/// Шаги 2–5. Первая фатальная ошибка записывается в отчёт и прерывает остальное.
fn run_steps(session: &mut Session, report: &mut CodecSurfaceReport, params: &RenderParams) {
    let Some(window) = session.window.as_ref() else { return };
    report.window = Some(window.report());

    let host = match SurfaceInstance::create(window) {
        Ok(host) => session.host.insert(host),
        Err(error) => return report.fail("instance/surface", &error),
    };
    let device = match host.create_device() {
        Ok(device) => session.device.insert(device),
        Err(error) => return report.fail("device", &error),
    };
    report.extensions = Some(extensions_report(host, device));
    let query = match query_surface(host, device) {
        Ok(query) => query,
        Err(error) => return report.fail("surface_query", &error),
    };
    report.surface = Some(query.report.clone());
    if !query.report.supported {
        return report.fail("surface_query", "очередь не поддерживает показ на эту surface");
    }
    match Rig::create(host, device, &query, params) {
        Ok((rig, swapchain_report)) => {
            report.swapchain = Some(swapchain_report);
            let rig = session.rig.insert(rig);
            report.render = Some(run_render_loop(rig, device, params));
        }
        Err(error) => {
            report.swapchain = Some(SwapchainReport {
                ok: false,
                error: Some(error.clone()),
                ..SwapchainReport::default()
            });
            report.fail("swapchain", &error);
        }
    }
}

/// Точка входа теста: окно уже получено (или `None`, если `ANativeWindow_fromSurface` вернул null).
pub fn run_codec_surface_test_json(
    window: Option<NativeWindow>,
    params: Result<RenderParams, String>,
) -> Result<String, ProbeError> {
    let mut report = CodecSurfaceReport::default();
    let mut session = Session {
        window,
        ..Session::default()
    };
    match params {
        Err(error) => report.fail("params", &error),
        Ok(_) if session.window.is_none() => report.fail("window", "ANativeWindow_fromSurface вернул null"),
        Ok(params) => run_steps(&mut session, &mut report, &params),
    }
    report.teardown = Some(session.teardown());
    Ok(report.to_json()?)
}
