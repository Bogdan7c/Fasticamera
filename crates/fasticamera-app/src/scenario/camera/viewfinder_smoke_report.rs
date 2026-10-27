//! Отчёт и вердикт сценария `viewfinder` (сессия 07): продуктовый видоискатель с ISP за минуту.
//!
//! Правила вердикта ([`evaluate_viewfinder`]) — по плану 07 («Тесты», телефон):
//! 1. запуск и поток без отказа камеры/рендера;
//! 2. частота сенсора по `SENSOR_TIMESTAMP` не ниже `min_sensor_fps`;
//! 3. ISP обрабатывает каждый кадр: «кадров через ISP в секунду» = частоте сенсора, а «кадров через
//!    ISP» = «показано новых кадров» (оба — в пределах `frame_rate_tolerance`);
//! 4. время прохода ISP (`isp_dispatch`, p95) в бюджете `isp_dispatch_budget_ms`; окно времени кадра
//!    (`frame_total`) обязано быть (иначе замер не работает);
//! 5. ноль CPU-копий (ISP и поток RAW), ноль ошибок validation (если слой включён);
//! 6. параметры ISP разрешались, отказов нет, кадров без параметров и с неверной геометрией нет,
//!    ошибок записи ISP и компоновки нет;
//! 7. статистика AWB публиковалась;
//!    Потери кадров видоискателя считаются, а не прячутся: вытеснение (`superseded`) и переполнение
//!    джиттер-буфера (`jitter_overflow`) — в отчёте раздельно. Переполнений допустимо не больше
//!    дрейфа часов камеры против экрана ([`max_jitter_overflow_in_window`]), иначе буфер не справляется;
//! 8. задержка «сенсор → экран» замерена;
//! 9. видоискатель продуктовый (ISP, все стадии), цветовое пространство экрана известно (P3 или sRGB —
//!    пишется честно).
//!
//! «Не чёрный» и (по флагу) «нейтральная карта» судит скрипт по скриншотам стадии `viewfinder_live`.

use isp_vulkan::viewfinder::IspStageSet;
use render_loop::viewfinder::{ViewfinderDiagnostics, ViewfinderGpuTimes, ViewfinderPainter, ViewfinderStorage};
use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::raw_stream_samples::LatencyReport;
use super::viewfinder_metrics::{GpuTimeMs, WindowDelta, color_space_name, ratio, round2};
use super::viewfinder_pacing_report::PacingWindowReport;
use super::viewfinder_session::SessionSummary;
use super::viewfinder_tuning_report::{AdpfReport, PresentTargetReport};
use crate::power_measurement::ThermalSnapshot;
use crate::scenario::render_metrics::ValidationReportPart;
use crate::viewfinder_scenario_config::ViewfinderSmokeConfig;

/// Время GPU видоискателя на конец замера (скользящие окна потока рендера).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ViewfinderGpuTimesMs {
    pub isp_dispatch: Option<GpuTimeMs>,
    pub frame_isp_span: Option<GpuTimeMs>,
    pub compose: Option<GpuTimeMs>,
    pub frame_total: Option<GpuTimeMs>,
}

impl ViewfinderGpuTimesMs {
    pub fn of(times: &ViewfinderGpuTimes) -> Self {
        Self {
            isp_dispatch: GpuTimeMs::of_optional(times.isp_dispatch.as_ref()),
            frame_isp_span: GpuTimeMs::of_optional(times.frame_isp_span.as_ref()),
            compose: GpuTimeMs::of_optional(times.compose.as_ref()),
            frame_total: GpuTimeMs::of_optional(times.frame_total.as_ref()),
        }
    }
}

/// Что видоискатель рисует и куда.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewfinderLook {
    /// `isp` / `debug_mosaic`.
    pub painter: &'static str,
    /// Все стадии ISP включены (продукт).
    pub all_stages: bool,
    /// `display_p3` / `srgb` / `unknown`.
    pub display_color_space: &'static str,
    pub storage: Option<&'static str>,
    pub output_extent: Option<[u32; 2]>,
}

impl ViewfinderLook {
    pub fn of(viewfinder: &ViewfinderDiagnostics) -> Self {
        Self {
            painter: match viewfinder.painter {
                ViewfinderPainter::Isp => "isp",
                ViewfinderPainter::DebugMosaic => "debug_mosaic",
            },
            all_stages: viewfinder.stages == IspStageSet::ALL,
            display_color_space: color_space_name(viewfinder.display_color_space),
            storage: viewfinder.storage.map(|storage| match storage {
                ViewfinderStorage::TenBitUnorm => "ten_bit_unorm",
                ViewfinderStorage::HalfFloat => "half_float",
            }),
            output_extent: viewfinder.output_extent.map(|extent| [extent.width, extent.height]),
        }
    }
}

/// Наблюдения прогона `viewfinder`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewfinderSmokeObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    pub session: SessionSummary,
    pub sensor_fps: Option<f64>,
    pub window: WindowDelta,
    /// Подача кадров видоискателя за замер: потери (вытеснение и переполнение буфера отдельно),
    /// повторы, доля кадров, показанных ровно один раз.
    pub pacing: PacingWindowReport,
    pub gpu_time: ViewfinderGpuTimesMs,
    pub look: Option<ViewfinderLook>,
    pub validation: Option<ValidationReportPart>,
    pub latency: LatencyReport,
    /// Выбор линии кадра, оценка стоимости кадра и время кадра на GPU (`None` — снимка рендера не было).
    pub present_target: Option<PresentTargetReport>,
    /// ADPF на конец замера (`None` — снимка рендера не было).
    pub adpf: Option<AdpfReport>,
    /// Ожидание картинки swapchain и CPU «захват → сабмит» (окна линии кадра).
    pub acquire_wait: Option<GpuTimeMs>,
    pub latch_to_submit: Option<GpuTimeMs>,
    pub thermal_start: ThermalSnapshot,
    pub thermal_end: ThermalSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_isp_failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_params_failure: Option<String>,
}

/// Расхождение двух величин относительно опорной (`None` — опорная нулевая).
fn relative_gap(value: f64, reference: f64) -> Option<f64> {
    ratio((value - reference).abs(), reference)
}

/// Вердикт: список нарушений (пустой — прошёл).
pub fn evaluate_viewfinder(config: &ViewfinderSmokeConfig, observation: &ViewfinderSmokeObservation) -> Vec<String> {
    if let Some(failure) = &observation.failure {
        return vec![failure.describe()];
    }
    let mut violations = Vec::new();
    let window = &observation.window;
    match observation.sensor_fps {
        None => violations.push("частота сенсора по SENSOR_TIMESTAMP не измерена".to_owned()),
        Some(fps) if fps < config.min_sensor_fps => violations.push(format!(
            "частота сенсора {fps:.2} к/с ниже {} к/с",
            config.min_sensor_fps
        )),
        Some(fps) => match window.isp_frames_per_second() {
            Some(isp_fps) if relative_gap(isp_fps, fps).is_some_and(|gap| gap <= config.frame_rate_tolerance) => {}
            Some(isp_fps) => violations.push(format!(
                "ISP обработал {isp_fps:.2} кадров/с при частоте сенсора {fps:.2}: расхождение больше {:.0} %",
                config.frame_rate_tolerance * 100.0
            )),
            None => violations.push("окно замера пустое: кадров/с ISP не посчитать".to_owned()),
        },
    }
    let processed = window.isp.frames_processed as f64;
    let presented = window.camera_frames_presented as f64;
    match relative_gap(processed, presented) {
        None => violations.push("на экран не поставлено ни одного нового кадра камеры".to_owned()),
        Some(gap) if gap > config.frame_rate_tolerance => violations.push(format!(
            "через ISP прошло {} кадров, показано новых {}: ISP обязан обработать каждый показанный кадр",
            window.isp.frames_processed, window.camera_frames_presented
        )),
        Some(_) => {}
    }
    match observation.gpu_time.isp_dispatch {
        None => violations.push("нет замеров времени прохода ISP (isp_dispatch)".to_owned()),
        Some(time) if time.p95_ms > config.isp_dispatch_budget_ms => violations.push(format!(
            "проход ISP p95 {:.2} мс больше бюджета {} мс",
            time.p95_ms, config.isp_dispatch_budget_ms
        )),
        Some(_) => {}
    }
    if observation.gpu_time.frame_total.is_none() {
        violations.push("нет замеров времени кадра видоискателя (frame_total)".to_owned());
    }
    let copies = window.isp.cpu_copies + window.stream_cpu_copies;
    if copies != 0 {
        violations.push(format!("CPU-копий кадров: {copies}, ожидалось 0"));
    }
    if let Some(violation) = observation
        .validation
        .as_ref()
        .and_then(ValidationReportPart::violation)
    {
        violations.push(violation);
    }
    violations.extend(isp_counter_violations(window));
    violations.extend(jitter_overflow_violation(config, observation));
    if observation.latency.samples == 0 {
        violations.push("задержка «сенсор → экран» не замерена ни разу".to_owned());
    }
    // ADPF по умолчанию выключен; включён и не создался — провал, `unsupported` только в отчёте.
    if let Some(violation) = observation.adpf.as_ref().and_then(AdpfReport::violation) {
        violations.push(violation);
    }
    match &observation.look {
        None => violations.push("в статусе рендера нет диагностики видоискателя".to_owned()),
        Some(look) => {
            if look.painter != "isp" || !look.all_stages {
                violations.push(format!(
                    "видоискатель не продуктовый: рисовальщик {}, все стадии {}",
                    look.painter, look.all_stages
                ));
            }
            if look.display_color_space == "unknown" {
                violations.push("цветовое пространство экрана не известно (кадров ISP не было?)".to_owned());
            }
        }
    }
    violations
}

/// Сколько переполнений джиттер-буфера допустимо за окно `window_ms`: дрейф часов камеры против
/// экрана (60,03 против 60,00 Гц — кадр за 33 с, ~1,8 в минуту) плюс запас на шум фазы, то есть
/// `max_jitter_overflow_per_minute` на минуту, округлённое вверх, но не меньше одного: короткое окно
/// может законно поймать оборот фазы.
pub fn max_jitter_overflow_in_window(config: &ViewfinderSmokeConfig, window_ms: u64) -> u64 {
    let minutes = window_ms as f64 / 60_000.0;
    ((config.max_jitter_overflow_per_minute * minutes).ceil() as u64).max(1)
}

/// Переполнения джиттер-буфера сверх допустимого дрейфа — потеря кадров, а не дрейф часов.
fn jitter_overflow_violation(
    config: &ViewfinderSmokeConfig,
    observation: &ViewfinderSmokeObservation,
) -> Option<String> {
    let allowed = max_jitter_overflow_in_window(config, observation.window.window_ms);
    let overflow = observation.pacing.jitter_overflow;
    (overflow > allowed).then(|| {
        format!(
            "джиттер-буфер переполнился {overflow} раз за {} мс, допустимо {allowed} (дрейф часов камеры): \
             камера и показ расходятся сильнее дрейфа или рендер не успевает забирать кадры",
            observation.window.window_ms
        )
    })
}

/// Счётчики ISP: параметры, отказы, AWB.
fn isp_counter_violations(window: &WindowDelta) -> Vec<String> {
    let isp = &window.isp;
    let mut violations = Vec::new();
    // `params_resolved` — НЕ правило: параметры разрешаются один раз на поток, при его старте, то есть
    // до окна замера, и приращение за окно законно 0 (так сценарий ложно падал на телефоне в 07).
    // Что параметры были весь замер, доказывают обработанные кадры (ISP без параметров кадр не пишет)
    // плюс `frames_dropped_no_params = 0` ниже.
    for (name, value) in [
        ("params_failed", isp.params_failed),
        ("isp_record_failures", isp.isp_record_failures),
        ("compose_failures", isp.compose_failures),
        ("frames_dropped_no_params", isp.frames_dropped_no_params),
        ("frames_rejected_geometry", isp.frames_rejected_geometry),
    ] {
        if value != 0 {
            violations.push(format!("{name} = {value}, ожидалось 0"));
        }
    }
    if isp.statistics_published == 0 {
        violations.push("статистика AWB ни разу не опубликована".to_owned());
    }
    violations
}

impl ViewfinderSmokeObservation {
    /// Сводка для строки результата (без журнала событий; он — в `detail_viewfinder`).
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "failure": self.failure,
            "configured": self.session.configured,
            "degraded": self.session.degraded,
            "time_to_live_ms": self.session.time_to_live_ms,
            "state_at_end": self.session.state_at_end,
            "sensor_fps": self.sensor_fps.map(round2),
            "isp_fps": self.window.isp_frames_per_second().map(round2),
            "window_ms": self.window.window_ms,
            "frames_processed": self.window.isp.frames_processed,
            "frames_presented": self.window.camera_frames_presented,
            "cpu_copies": self.window.isp.cpu_copies + self.window.stream_cpu_copies,
            "pacing": self.pacing,
            "params_resolved": self.window.isp.params_resolved,
            "params_failed": self.window.isp.params_failed,
            "awb_published": self.window.isp.statistics_published,
            "gpu_time": self.gpu_time,
            "look": self.look,
            "validation": self.validation.as_ref().map(|validation| serde_json::json!({
                "mode": validation.mode, "errors": validation.errors,
            })),
            "latency": {
                "samples": self.latency.samples,
                "p50_ms": self.latency.p50_ms.map(round2),
                "p95_ms": self.latency.p95_ms.map(round2),
            },
            "present_target": {
                "miss_ratio": self.window.present_target.vsync_shift.miss_ratio().map(round2),
                "fallbacks": self.window.present_target.fallbacks_to_preferred,
                "selection": self.present_target,
            },
            "adpf": self.adpf,
            "thermal_end": self.thermal_end,
        })
    }

    /// Подробности: всё наблюдение по частям (каждая часть — элемент `detail_viewfinder`).
    pub fn detail_items(&self) -> Vec<Value> {
        vec![
            serde_json::json!({ "session": to_value(&self.session) }),
            serde_json::json!({ "window": to_value(&self.window) }),
            serde_json::json!({
                "latency": to_value(&self.latency),
                "acquire_wait": self.acquire_wait,
                "latch_to_submit": self.latch_to_submit,
            }),
            serde_json::json!({
                "thermal_start": self.thermal_start,
                "thermal_end": self.thermal_end,
                "last_isp_failure": self.last_isp_failure,
                "last_params_failure": self.last_params_failure,
                "validation": self.validation,
            }),
        ]
    }
}

#[cfg(test)]
#[path = "viewfinder_smoke_report_tests.rs"]
mod tests;
