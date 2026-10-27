//! Создание ОДНОГО потока RAW-кадров в composition root и его наблюдательных сервисов.
//!
//! Поток создаётся до запуска рендера и камеры: камера получает `producer()`, рендер —
//! `viewfinder()`, сценарии — [`RawStreamServices`]. Модуль нейтрален к платформе, поэтому
//! правила («неверный конфиг не роняет приложение», «наблюдатель видит те же счётчики, что пишет
//! производитель») проверяются на ПК с настоящим `RawFrameStream`.

use std::sync::Arc;

use camera_android::CpuAccessProbe;
use raw_frame_stream::{RawFrameStream, RawFrameStreamConfig, RawFrameStreamConfigError};

use crate::clock_offset::SystemClocks;
use crate::raw_stream_services::RawStreamServices;

const LOG_TARGET: &str = "fasticamera::raw_stream::wiring";

/// Созданный поток и (если был) отказ конфига.
pub struct RawStreamCreation {
    pub stream: RawFrameStream,
    /// Конфиг, из которого поток создан на самом деле.
    pub effective_config: RawFrameStreamConfig,
    /// Почему переданный конфиг не принят. Тогда поток собран из значений по умолчанию, а камера
    /// не запустится (её валидация отвергает тот же конфиг) — рендер при этом показывает чёрный экран.
    pub rejected_config: Option<RawFrameStreamConfigError>,
}

/// Создаёт поток. Неверный конфиг не роняет приложение: поток всё равно нужен рендеру (канал
/// видоискателя), поэтому он строится на значениях по умолчанию, а отказ возвращается и логируется.
pub fn create_raw_stream(config: &RawFrameStreamConfig) -> RawStreamCreation {
    match config.validate() {
        Ok(()) => RawStreamCreation {
            stream: RawFrameStream::new(config.clone()),
            effective_config: config.clone(),
            rejected_config: None,
        },
        Err(error) => {
            tracing::error!(
                target: LOG_TARGET,
                error = %error,
                "camera.raw_stream неверен: поток создан на значениях по умолчанию, камера не запустится"
            );
            let fallback = RawFrameStreamConfig::default();
            RawStreamCreation {
                stream: RawFrameStream::new(fallback.clone()),
                effective_config: fallback,
                rejected_config: Some(error),
            }
        }
    }
}

/// Наблюдательные сервисы для сценариев над созданным потоком. `probe` — рубильник пробы CPU,
/// который отдал бэкенд камеры (клоны делят одно состояние).
pub fn observe_stream(creation: &RawStreamCreation, probe: CpuAccessProbe) -> RawStreamServices {
    RawStreamServices::new(
        Arc::new(creation.stream.clone()),
        probe,
        Arc::new(SystemClocks),
        creation.effective_config.clone(),
    )
}

#[cfg(test)]
#[path = "raw_stream_wiring_tests.rs"]
mod tests;
