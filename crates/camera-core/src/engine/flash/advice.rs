//! Совет для Auto и статус вспышки (решение 159).
//!
//! Совет решает наш AE, а не экран: «вспыхнуть», если свету сцены без вспышки для среднего серого
//! нужно не меньше `three_a.flash.auto_fire_min_mid_gray_log2_seconds_iso` (log2 с·ISO).
//! Берётся ПРИНЯТАЯ цель AE при компенсации 0 EV, БЕЗ EV пользователя: компенсация — вкусовая
//! поправка к яркости снимка, а не свойство сцены; иначе «+2 EV» включал бы вспышку при дневном свете.

use camera_contract::flash::FlashUnit;

use super::super::CameraEngine;
use crate::flash_status::{FlashAdvice, FlashAdviceUnavailable, FlashStatus};
use crate::three_a_control::SceneForFlash;

impl CameraEngine {
    /// Вспышка для статуса: блок, фаза и счётчики.
    pub(in crate::engine) fn flash_status(&self) -> FlashStatus {
        FlashStatus {
            unit: self
                .planning_capabilities()
                .map_or(FlashUnit::Absent, |capabilities| capabilities.flash),
            advice: self.flash_advice(),
            phase: self.flash.phase(),
            counters: self.flash.counters,
        }
    }

    /// Совет для Auto по текущему состоянию. Порядок причин: нет камеры, нет вспышки, чужой AE, поток, замер.
    fn flash_advice(&self) -> FlashAdvice {
        let Some(capabilities) = self.planning_capabilities() else {
            return unavailable(FlashAdviceUnavailable::NotStreaming);
        };
        if !capabilities.flash.is_present() {
            return unavailable(FlashAdviceUnavailable::NoFlashUnit);
        }
        let scene = self.three_a.scene_for_flash();
        if scene == SceneForFlash::NotOwnExposure {
            return unavailable(FlashAdviceUnavailable::NotOwnExposure);
        }
        if !self.is_streaming_now() {
            return unavailable(FlashAdviceUnavailable::NotStreaming);
        }
        let SceneForFlash::Measured { mid_gray_product } = scene else {
            return unavailable(FlashAdviceUnavailable::NoMeasurement);
        };
        let threshold = self.config.three_a.flash.auto_fire_min_mid_gray_log2_seconds_iso;
        if mid_gray_product.seconds_iso().log2() >= threshold {
            FlashAdvice::Fire
        } else {
            FlashAdvice::NotNeeded
        }
    }
}

fn unavailable(reason: FlashAdviceUnavailable) -> FlashAdvice {
    FlashAdvice::Unavailable { reason }
}
