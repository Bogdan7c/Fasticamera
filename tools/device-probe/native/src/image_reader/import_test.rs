//! `rawReaderRunImportTest`: цикл захвата кадров из `AImageReader` и прогон каждого
//! через Vulkan-пайплайн (`frame_pipeline`). Блокирует вызывающий поток до `frames`
//! кадров или таймаута; камеру ведёт Kotlin.

use std::time::{Duration, Instant};

use super::acquired_image::{AcquireOutcome, AcquiredImage};
use super::frame_pipeline::process_frame;
use super::registry::find_reader;
use crate::error::ProbeError;
use crate::report::{FrameReport, ImportTestReport, build_summary};
use crate::vulkan::context::shared_context;

/// Как часто опрашиваем ридер, пока кадров нет. 2 мс ≪ периода кадра при 30–60 fps.
const NO_FRAME_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Неизвестный handle — штатный отрицательный ответ, а не сбой пробы.
fn unknown_handle_json() -> String {
    r#"{"ok":false,"error":"unknown_handle"}"#.to_owned()
}

/// Основная точка входа: возвращает JSON-отчёт.
pub fn run_import_test_json(handle: i64, frames: i32, timeout_ms: i32) -> Result<String, ProbeError> {
    let Some(entry) = find_reader(handle) else {
        return Ok(unknown_handle_json());
    };
    // Держим блокировку ридера весь прогон: параллельный destroy дождётся конца.
    let native_guard = entry.lock_native();
    let Some(native) = native_guard.as_ref() else {
        return Ok(unknown_handle_json());
    };

    let wanted_frames = usize::try_from(frames).unwrap_or(0);
    let timeout = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
    let ctx = shared_context()?;

    let deadline = Instant::now() + timeout;
    let mut reports: Vec<FrameReport> = Vec::new();
    while reports.len() < wanted_frames {
        // SAFETY: ридер жив — мы держим его мьютекс (`native_guard`).
        match unsafe { AcquiredImage::acquire_next(native.as_ptr()) } {
            AcquireOutcome::Image(image) => {
                let index = reports.len();
                reports.push(process_frame(&ctx, index, image, &entry));
            }
            AcquireOutcome::NoBufferYet => {
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(NO_FRAME_POLL_INTERVAL);
            }
            AcquireOutcome::Failed(status) => {
                reports.push(FrameReport {
                    index: reports.len(),
                    error: Some(format!("acquireNextImageAsync: {status}")),
                    ..FrameReport::default()
                });
                break;
            }
        }
    }

    let summary = build_summary(&reports);
    let report = ImportTestReport {
        ok: true,
        frames: reports,
        summary,
    };
    Ok(serde_json::to_string(&report)?)
}
