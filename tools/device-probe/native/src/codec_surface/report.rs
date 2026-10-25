//! Типизированный JSON-отчёт теста. Имена полей — контракт с Kotlin-стороной.

use serde::Serialize;
use serde_json::{Map, Value};

/// Параметры и свойства `ANativeWindow`.
#[derive(Debug, Serialize, Default)]
pub struct WindowReport {
    pub width: i32,
    pub height: i32,
    pub format: i32,
}

/// Пара формат + цветовое пространство в текстовом виде.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct FormatEntry {
    pub format: String,
    pub color_space: String,
}

/// Возможности поверхности.
#[derive(Debug, Serialize, Default, Clone)]
pub struct SurfaceReport {
    pub supported: bool,
    pub capabilities: Value,
    pub formats: Vec<FormatEntry>,
    pub present_modes: Vec<String>,
    /// `true`, если среди форматов есть 10-битный (A2…10G10R10).
    pub has_ten_bit_format: bool,
    /// `true`, если среди цветовых пространств есть HDR10 (PQ или HLG).
    pub has_hdr10_color_space: bool,
}

/// Результат создания swapchain.
#[derive(Debug, Serialize, Default)]
pub struct SwapchainReport {
    pub ok: bool,
    pub format: Option<String>,
    pub color_space: Option<String>,
    pub image_count: u32,
    /// Фактический размер картинок swapchain `[ширина, высота]`.
    pub extent: Option<[u32; 2]>,
    pub usage: Option<String>,
    pub error: Option<String>,
}

/// Результат цикла рендера.
#[derive(Debug, Serialize, Default)]
pub struct RenderReport {
    pub frames_requested: u32,
    pub frames_presented: u32,
    pub suboptimal: u32,
    pub out_of_date: u32,
    pub acquire_us_median: u64,
    pub present_us_median: u64,
    pub acquire_us_max: u64,
    pub present_us_max: u64,
    pub wall_ms: u64,
    pub present_times_used: bool,
    pub error: Option<String>,
}

/// Результат освобождения ресурсов.
#[derive(Debug, Serialize, Default)]
pub struct TeardownReport {
    pub ok: bool,
    pub errors: Vec<String>,
}

/// Итоговый отчёт. Секции, до которых тест не дошёл, остаются `null`.
#[derive(Debug, Serialize, Default)]
pub struct CodecSurfaceReport {
    pub ok: bool,
    pub error: Option<String>,
    pub window: Option<WindowReport>,
    pub extensions: Option<Map<String, Value>>,
    pub surface: Option<SurfaceReport>,
    pub swapchain: Option<SwapchainReport>,
    pub render: Option<RenderReport>,
    pub teardown: Option<TeardownReport>,
}

impl CodecSurfaceReport {
    /// Фиксирует первую фатальную ошибку: `"<этап>: <причина>"`.
    pub fn fail(&mut self, stage: &str, reason: &str) {
        if self.error.is_none() {
            self.error = Some(format!("{stage}: {reason}"));
        }
    }

    /// Сериализует отчёт; `ok` = нет ошибки и все кадры показаны.
    pub fn to_json(mut self) -> Result<String, serde_json::Error> {
        let rendered = self
            .render
            .as_ref()
            .is_some_and(|render| render.error.is_none() && render.frames_presented == render.frames_requested);
        self.ok = self.error.is_none() && rendered;
        serde_json::to_string(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_failure_has_null_sections_and_error_text() {
        let mut report = CodecSurfaceReport::default();
        report.fail("window", "ANativeWindow_fromSurface вернул null");
        report.fail("later", "не должно перезаписать первую");
        let value: Value = serde_json::from_str(&report.to_json().expect("json")).expect("parse");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], "window: ANativeWindow_fromSurface вернул null");
        assert!(value["swapchain"].is_null());
        assert!(value["render"].is_null());
    }

    #[test]
    fn ok_requires_all_frames_presented() {
        let render = |presented| RenderReport {
            frames_requested: 10,
            frames_presented: presented,
            ..RenderReport::default()
        };
        let full = CodecSurfaceReport {
            render: Some(render(10)),
            ..CodecSurfaceReport::default()
        };
        let partial = CodecSurfaceReport {
            render: Some(render(7)),
            ..CodecSurfaceReport::default()
        };
        let parse = |report: CodecSurfaceReport| -> Value {
            serde_json::from_str(&report.to_json().expect("json")).expect("parse")
        };
        assert_eq!(parse(full)["ok"], true);
        assert_eq!(parse(partial)["ok"], false);
    }

    #[test]
    fn full_report_has_expected_keys() {
        let report = CodecSurfaceReport {
            window: Some(WindowReport {
                width: 3840,
                height: 2160,
                format: 4,
            }),
            swapchain: Some(SwapchainReport {
                ok: true,
                image_count: 3,
                ..SwapchainReport::default()
            }),
            teardown: Some(TeardownReport {
                ok: true,
                errors: vec![],
            }),
            ..CodecSurfaceReport::default()
        };
        let value: Value = serde_json::from_str(&report.to_json().expect("json")).expect("parse");
        assert_eq!(value["window"]["width"], 3840);
        assert_eq!(value["swapchain"]["image_count"], 3);
        assert_eq!(value["teardown"]["ok"], true);
    }
}
