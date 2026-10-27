//! Решение об откате на запасную частоту кадров (решение 23).
//!
//! Видоискатель основной камеры просит 4:3@60 через вендорские ключи CamX. Если ключей нет,
//! бэкенд отвечает `Unsupported(FrameRateNotAchievable)`. Вместо отказа движок один раз
//! понижает частоту до запасной из конфига и сообщает об этом событием
//! `CameraEvent::StreamDegraded` — молчаливых откатов нет.
//!
//! Модуль — чистая функция без доступа к движку: само решение тестируется отдельно,
//! а исполняет его `reconcile::configure_session`.

use camera_contract::stream::{StreamMode, TargetFrameRate};

use crate::backend::{ConfigureFailure, UnsupportedStream};
use crate::config::FrameRateFallback;

/// Режим, на который надо откатиться после неудачной настройки `mode`, либо `None`,
/// если откат не положен.
///
/// Откат возможен только когда выполнены все условия сразу:
/// * сбой именно «частота недостижима» (отказ HAL, нет вырезки и т. п. частотой не лечатся);
/// * откат включён в конфиге, а его частота валидна;
/// * запрошенная частота строго выше запасной (иначе это не откат, а повтор того же или
///   повышение) — это же гарантирует, что повторный откат с запасного режима невозможен,
///   то есть цикла не будет.
pub(super) fn fallback_for(
    mode: StreamMode,
    failure: &ConfigureFailure,
    policy: FrameRateFallback,
) -> Option<StreamMode> {
    let ConfigureFailure::Unsupported(UnsupportedStream::FrameRateNotAchievable { .. }) = failure else {
        return None;
    };
    let FrameRateFallback::To { fps } = policy else {
        return None;
    };
    // Невалидную частоту ловит `CameraEngineConfig::validate`; здесь это просто «отката нет».
    let fallback_rate = TargetFrameRate::new(fps).ok()?;
    (mode.frame_rate > fallback_rate).then_some(StreamMode {
        frame_rate: fallback_rate,
        ..mode
    })
}

#[cfg(test)]
mod tests {
    use camera_contract::raw_format::RawFormat;
    use camera_contract::stream::ReadoutAspect;

    use super::*;

    fn mode_at(fps: u32) -> StreamMode {
        StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::FourByThree,
            frame_rate: TargetFrameRate::new(fps).expect("валидная частота"),
        }
    }

    fn not_achievable(fps: u32) -> ConfigureFailure {
        ConfigureFailure::Unsupported(UnsupportedStream::FrameRateNotAchievable {
            requested: TargetFrameRate::new(fps).expect("валидная частота"),
        })
    }

    #[test]
    fn lowers_only_the_frame_rate_and_keeps_format_and_readout() {
        let fallback = fallback_for(mode_at(60), &not_achievable(60), FrameRateFallback::To { fps: 30 });
        assert_eq!(fallback, Some(mode_at(30)));
    }

    #[test]
    fn other_failures_do_not_trigger_fallback() {
        let policy = FrameRateFallback::To { fps: 30 };
        let rejected = ConfigureFailure::RejectedByHal { detail: "x".to_owned() };
        let readout = ConfigureFailure::Unsupported(UnsupportedStream::ReadoutNotAvailable {
            readout: ReadoutAspect::FourByThree,
        });
        assert_eq!(fallback_for(mode_at(60), &rejected, policy), None);
        assert_eq!(fallback_for(mode_at(60), &readout, policy), None);
        assert_eq!(
            fallback_for(mode_at(60), &ConfigureFailure::DeviceDisconnected, policy),
            None
        );
    }

    #[test]
    fn no_fallback_when_requested_rate_is_not_above_fallback() {
        let policy = FrameRateFallback::To { fps: 30 };
        assert_eq!(fallback_for(mode_at(30), &not_achievable(30), policy), None);
        assert_eq!(fallback_for(mode_at(24), &not_achievable(24), policy), None);
    }

    #[test]
    fn disabled_policy_never_falls_back() {
        assert_eq!(
            fallback_for(mode_at(60), &not_achievable(60), FrameRateFallback::Disabled),
            None
        );
    }

    #[test]
    fn invalid_fallback_rate_means_no_fallback_instead_of_panic() {
        let policy = FrameRateFallback::To { fps: 0 };
        assert_eq!(fallback_for(mode_at(60), &not_achievable(60), policy), None);
    }
}
