//! Перенос произведения «выдержка × ISO» с камеры A на камеру B (решения 118 и 126).
//!
//! Одна функция на два места: (а) повторяющийся запрос тёплой камеры (экспозиция основной,
//! пересчитанная на тёплую), (б) стартовая экспозиция новой камеры при обычной смене объектива
//! (память прошлого потока исполнителя экспозиции).
//!
//! # Модель
//! Свет на выходной пиксель пропорционален `pixel_pitch² / f_number²`, где `pixel_pitch` —
//! размер пикселя RAW-буфера: `sensor_physical_size.width / pixel_array.width`. Чувствительность
//! камеры `s = pitch² / N²`; чтобы кадр B был так же ярок, как кадр A, произведение переносится
//! так: `P_B = P_A × s_A / s_B`. Замер на телефоне (основная против ширика, одно поле зрения):
//! +1,18 EV; модель даёт +1,22 EV (диафрагма +0,58, пиксель +0,64).
//!
//! Нет f-числа или размера пикселя у какой-либо из камер — **этот** член даёт множитель 1, а
//! причина возвращается явно ([`TransferTerm::Skipped`]): молча подставлять догадку нельзя.
//! Источник поправки владелец может сменить (замеры зонда); тогда меняется только эта функция.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::capabilities::CameraCapabilities;

/// У какой из двух камер не хватило данных.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraSide {
    /// Камера A: откуда переносим.
    Source,
    /// Камера B: куда переносим.
    Target,
}

/// Чего не хватило члену модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSensitivityData {
    /// Нет правдоподобного f-числа (список пуст, ноль, NaN).
    Aperture(CameraSide),
    /// Нет правдоподобного размера пикселя (нулевой массив или физический размер).
    PixelPitch(CameraSide),
}

/// Как учтён один член модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferTerm {
    /// Член посчитан по данным обеих камер.
    Applied,
    /// Данных не хватило: множитель этого члена равен 1.
    Skipped(MissingSensitivityData),
}

/// Множитель переноса произведения A → B и то, как он получен.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureTransfer {
    /// Во сколько раз умножить произведение A, чтобы получить произведение B.
    pub factor: f64,
    pub aperture: TransferTerm,
    pub pixel_pitch: TransferTerm,
}

impl ExposureTransfer {
    /// Перенос не меняет произведение (одна и та же камера).
    pub const IDENTITY: Self = Self {
        factor: 1.0,
        aperture: TransferTerm::Applied,
        pixel_pitch: TransferTerm::Applied,
    };

    /// Произведение камеры B, дающее ту же яркость, что `product` на камере A.
    /// Если множитель вывел бы за конечные числа, произведение остаётся прежним.
    pub fn apply(&self, product: ExposureProduct) -> ExposureProduct {
        ExposureProduct::from_seconds_iso(product.seconds_iso() * self.factor).unwrap_or(product)
    }

    /// Все ли члены посчитаны по данным (нет ни одной подстановки «1»).
    pub fn is_fully_measured(&self) -> bool {
        self.aperture == TransferTerm::Applied && self.pixel_pitch == TransferTerm::Applied
    }
}

/// Множитель переноса произведения с камеры `from` на камеру `to`.
pub fn exposure_transfer(from: &CameraCapabilities, to: &CameraCapabilities) -> ExposureTransfer {
    let (aperture_factor, aperture) = match (aperture_f_number(from), aperture_f_number(to)) {
        // Диафрагма: света тем больше, чем меньше f-число; `s ∝ 1 / N²`, `s_A / s_B = N_B² / N_A²`.
        (Some(source), Some(target)) => ((target / source).powi(2), TransferTerm::Applied),
        (None, _) => (
            1.0,
            TransferTerm::Skipped(MissingSensitivityData::Aperture(CameraSide::Source)),
        ),
        (_, None) => (
            1.0,
            TransferTerm::Skipped(MissingSensitivityData::Aperture(CameraSide::Target)),
        ),
    };
    let (pitch_factor, pixel_pitch) = match (pixel_pitch_mm(from), pixel_pitch_mm(to)) {
        // Пиксель: `s ∝ pitch²`, `s_A / s_B = pitch_A² / pitch_B²`.
        (Some(source), Some(target)) => ((source / target).powi(2), TransferTerm::Applied),
        (None, _) => (
            1.0,
            TransferTerm::Skipped(MissingSensitivityData::PixelPitch(CameraSide::Source)),
        ),
        (_, None) => (
            1.0,
            TransferTerm::Skipped(MissingSensitivityData::PixelPitch(CameraSide::Target)),
        ),
    };
    ExposureTransfer {
        factor: aperture_factor * pitch_factor,
        aperture,
        pixel_pitch,
    }
}

/// F-число камеры: первое правдоподобное из списка (у телефонных объективов диафрагма одна).
fn aperture_f_number(capabilities: &CameraCapabilities) -> Option<f64> {
    capabilities
        .optics
        .apertures_f_number
        .iter()
        .copied()
        .map(f64::from)
        .find(|f_number| f_number.is_finite() && *f_number > 0.0)
}

/// Размер пикселя RAW-буфера, мм: физическая ширина сенсора на ширину массива пикселей.
fn pixel_pitch_mm(capabilities: &CameraCapabilities) -> Option<f64> {
    let width_mm = f64::from(capabilities.optics.sensor_physical_size.width_mm);
    let width_pixels = f64::from(capabilities.pixel_array.width);
    let pitch = width_mm / width_pixels;
    (width_mm.is_finite() && width_mm > 0.0 && width_pixels > 0.0 && pitch.is_finite()).then_some(pitch)
}

#[cfg(test)]
#[path = "exposure_transfer_tests.rs"]
mod tests;
