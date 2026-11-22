//! Сборка `StillFrameFacts`: что снимок знает о выбранном кадре (кадрирование, поворот, нейтраль, подтяжка).
//!
//! Всё считается ДО захвата (кадр ещё в кольце, паспорт у лизинга): отказ на этом этапе даёт «кадра нет»
//! ([`FactsRefusal`]), а не провал уже после того, как затвор отпущен.
//!
//! * Кадрирование — из ПЛАНА снимка (`plan.framing`: соотношение, зум, шкала камер на момент нажатия), а не из
//!   текущих настроек видоискателя: зум мог измениться, пока снимок ждал очереди. Формула та же
//!   (`ZoomFraming::crop_for_camera`), камера — ТОГО кадра, который снимается.
//! * Нейтраль DNG — по нашей белой точке и DNG-профилю потока (`neutral_for_white`), НЕ подсказка HAL (решение 9).
//! * `DigitalGainEv` — `log2 k` (подтяжка видоискателя этого кадра).

use camera_contract::frame_passport::RawFramePassport;
use camera_contract::geometry::PixelRect;
use isp_model::sensor::SensorDescription;
use still_contract::frame_choice::FrameChoice;
use still_contract::plan::StillPlan;
use still_contract::shot::{AsShotNeutral, DigitalGainEv, StillFrameFacts};

use super::feed::FrameStillParams;
use super::rotation::still_rotation;

/// Почему факты кадра собрать нельзя (кадр снимать не будем).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FactsRefusal {
    /// У кадра нет метаданных результата (кольцо такие не выдаёт, но проверка дешёвая и честная).
    #[error("у кадра нет метаданных результата")]
    NoCaptureDetails,
    /// Камеры кадра нет в каталоге зума плана: направление (поворот) неизвестно.
    #[error("камеры кадра нет в каталоге зума плана")]
    CameraNotInCatalog,
    /// Кадрирование не построилось по полезной области.
    #[error("кадрирование: {0}")]
    Crop(String),
    /// Описание сенсора или нейтраль не построились.
    #[error("цвет кадра: {0}")]
    Color(String),
}

/// Собирает факты кадра. `passport` — паспорт кадра выбранного лизинга.
pub fn build_facts(
    plan: &StillPlan,
    passport: &RawFramePassport,
    choice: FrameChoice,
    params: &FrameStillParams,
) -> Result<StillFrameFacts, FactsRefusal> {
    let capture = passport
        .capture
        .details()
        .cloned()
        .ok_or(FactsRefusal::NoCaptureDetails)?;
    let stream = &passport.stream;
    let (group, _) = plan
        .framing
        .catalog
        .lens_for_camera(&stream.camera)
        .ok_or(FactsRefusal::CameraNotInCatalog)?;
    let framed = plan.framing.crop_for_camera(&stream.camera);
    let crop: PixelRect = framed
        .crop
        .resolve(stream.valid_area)
        .map_err(|error| FactsRefusal::Crop(error.to_string()))?;
    let sensor = SensorDescription::from_stream_passport(stream, &stream.color_calibration)
        .map_err(|error| FactsRefusal::Color(error.to_string()))?;
    let neutral = sensor
        .color_profile()
        .neutral_for_white(params.white)
        .map_err(|error| FactsRefusal::Color(error.to_string()))?;
    Ok(StillFrameFacts {
        id: plan.id,
        outputs: plan.outputs,
        stream: stream.clone(),
        sensor_timestamp: passport.sensor_timestamp,
        capture,
        choice,
        crop,
        rotation: still_rotation(stream.sensor_orientation, group.facing, plan.orientation),
        neutral: AsShotNeutral(normalized_to_maximum(neutral.values())),
        digital_gain: DigitalGainEv(params.gain.ev()),
    })
}

/// Нейтраль DNG нормируется так, что максимум равен 1 (контракт `AsShotNeutral`). Профиль отдаёт нейтраль с
/// `G = 1`: у сенсоров, где красный или синий ярче зелёного, максимум не 1.
fn normalized_to_maximum(neutral: [f64; 3]) -> [f64; 3] {
    let maximum = neutral.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    neutral.map(|channel| channel / maximum)
}

#[cfg(test)]
#[path = "facts_tests.rs"]
mod tests;
