//! Белая точка по подсказкам HAL — ТОЛЬКО для сравнения с нашей (диагностика).
//!
//! Решение 62 оживило вендорский AWB пустым вспомогательным потоком, и `COLOR_CORRECTION_GAINS` снова
//! что-то значат. Но ISP они не питают никогда (критерий выхода 08): здесь они превращаются в белую точку
//! `white_for_neutral(1/gains)` и сравниваются с нашей белой в `RenderStatus`. Пересчёт — только при
//! изменении подсказок (итерация DNG не бесплатна).

use camera_contract::frame_passport::CaptureColorHints;
use isp_model::color::dng::{CameraNeutral, DngColorProfile};
use isp_model::color::white_point::{CctDuv, cct_duv_from_chromaticity};

use crate::viewfinder::{WhiteDelta, WhiteReading};

/// Кэш белой HAL: пересчитывается, когда подсказки кадра изменились.
#[derive(Debug, Default)]
pub struct HalWhiteTracker {
    last_hints: Option<CaptureColorHints>,
    last_white: Option<CctDuv>,
}

impl HalWhiteTracker {
    /// Сбрасывает память (новый поток: профиль цвета другой).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Учитывает подсказки очередного кадра и возвращает последнюю белую HAL (если вычислима).
    pub fn observe(&mut self, hints: &CaptureColorHints, profile: &DngColorProfile) -> Option<CctDuv> {
        if self.last_hints != Some(*hints) {
            self.last_white = hal_white(hints, profile);
            self.last_hints = Some(*hints);
        }
        self.last_white
    }
}

/// Нейтраль HAL: сначала коэффициенты баланса (`N_c = 1/g_c`, зелёный — среднее двух), затем нейтральная точка.
fn hal_neutral(hints: &CaptureColorHints) -> Option<CameraNeutral> {
    let from_gains = hints.color_gains.and_then(|gains| {
        let [red, green_even, green_odd, blue] = gains.0.map(f64::from);
        CameraNeutral::new([1.0 / red, 2.0 / (green_even + green_odd), 1.0 / blue]).ok()
    });
    from_gains.or_else(|| {
        let point = hints.neutral_color_point?;
        let values = [point[0].to_f64()?, point[1].to_f64()?, point[2].to_f64()?];
        CameraNeutral::new(values).ok()
    })
}

fn hal_white(hints: &CaptureColorHints, profile: &DngColorProfile) -> Option<CctDuv> {
    let neutral = hal_neutral(hints)?;
    let estimate = profile.white_for_neutral(neutral).ok()?;
    cct_duv_from_chromaticity(estimate.white).ok()
}

/// Белая точка в виде показаний диагностики.
pub fn reading_of(white: CctDuv) -> WhiteReading {
    WhiteReading {
        temperature_kelvin: white.temperature.0,
        duv: white.duv.0,
    }
}

/// Наша белая минус белая HAL.
pub fn delta_between(ours: CctDuv, hal: CctDuv) -> WhiteDelta {
    WhiteDelta {
        mired: ours.temperature.mired() - hal.temperature.mired(),
        duv: ours.duv.0 - hal.duv.0,
    }
}

#[cfg(test)]
mod tests {
    use camera_contract::capabilities::RawSupport;
    use camera_contract::fixtures::main_wide_camera;
    use camera_contract::frame_passport::ColorCorrectionGains;
    use isp_model::color::temperature::Kelvin;
    use isp_model::color::white_point::{Duv, chromaticity_from_cct_duv};

    use super::*;

    fn profile() -> DngColorProfile {
        let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
            panic!("в образце основной камеры есть RAW");
        };
        DngColorProfile::from_calibration_sets(&info.color_calibration).expect("профиль цвета")
    }

    /// Коэффициенты HAL для света `kelvin`: обратные нейтрали камеры.
    fn gains_for(profile: &DngColorProfile, kelvin: f64) -> CaptureColorHints {
        let white = chromaticity_from_cct_duv(CctDuv {
            temperature: Kelvin(kelvin),
            duv: Duv(0.0),
        })
        .expect("белая");
        let neutral = profile.neutral_for_white(white).expect("нейтраль").values();
        let gain = |channel: f64| (1.0 / channel) as f32;
        CaptureColorHints {
            color_gains: Some(ColorCorrectionGains([gain(neutral[0]), 1.0, 1.0, gain(neutral[2])])),
            ..CaptureColorHints::default()
        }
    }

    #[test]
    fn gains_of_a_known_light_give_that_light() {
        let profile = profile();
        let mut tracker = HalWhiteTracker::default();
        let white = tracker
            .observe(&gains_for(&profile, 4000.0), &profile)
            .expect("белая HAL");
        assert!((white.temperature.0 - 4000.0).abs() < 60.0, "{white:?}");
    }

    #[test]
    fn no_hints_means_no_hal_white_and_unit_gains_mean_a_greenish_sensor_white() {
        let profile = profile();
        let mut tracker = HalWhiteTracker::default();
        assert_eq!(tracker.observe(&CaptureColorHints::default(), &profile), None);
        let unit = CaptureColorHints {
            color_gains: Some(ColorCorrectionGains([1.0; 4])),
            ..CaptureColorHints::default()
        };
        // Единичные коэффициенты (RAW-only сессия без вендорской автоматики) — валидная нейтраль [1,1,1].
        assert!(tracker.observe(&unit, &profile).is_some());
    }

    #[test]
    fn delta_is_ours_minus_hal() {
        let ours = CctDuv {
            temperature: Kelvin(2800.0),
            duv: Duv(0.004),
        };
        let hal = CctDuv {
            temperature: Kelvin(5000.0),
            duv: Duv(0.001),
        };
        let delta = delta_between(ours, hal);
        assert!((delta.mired - (357.142857 - 200.0)).abs() < 1e-3);
        assert!((delta.duv - 0.003).abs() < 1e-12);
    }
}
