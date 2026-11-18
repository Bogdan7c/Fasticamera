//! Камера → CIE XYZ D50 по матрицам DNG.
//!
//! Источник: спецификация DNG 1.7, глава 6 «Mapping Camera Color Space to CIE XYZ Space»
//! (разделы «Translating White Balance Xy Coordinates to Camera Neutral Coordinates»,
//! «Camera Calibration Matrices and Analog Balance», «Translating Camera Neutral Coordinates
//! to White Balance Xy Coordinates» и «Camera to XYZ Matrix»). Порядок действий повторяет
//! Adobe DNG SDK (`dng_color_spec.cpp`: `NeutralToXY`, `FindXYZtoCamera`, `SetWhiteXY`);
//! код написан заново, SDK читался ради семантики. Сверка с colour-hdri 0.2.6 — в тестах.
//!
//! # Обозначения
//! - `CM(T)` — ColorMatrix (XYZ → камера), `CC(T)` — CameraCalibration, `AB` — AnalogBalance
//!   (диагональ), `FM(T)` — ForwardMatrix (камера, сбалансированная по белому → XYZ D50);
//!   `(T)` — интерполяция между двумя калибровками линейно по майредам.
//! - `XYZtoCamera = AB · CC(T) · CM(T)`.
//! - Нейтраль камеры (`AsShotNeutral`) — отклик камеры на нейтральный объект, обычно G = 1.

use camera_contract::capabilities::{ColorCalibrationSet, Matrix3x3};

use super::chromaticity::{Chromaticity, bradford_adaptation};
use super::matrix::{BrokenRational, Matrix3, SingularMatrix, Vector3};
use super::temperature::{
    Kelvin, UnknownCalibrationIlluminant, calibration_illuminant_temperature, temperature_from_chromaticity,
};

/// Предел проходов поиска белой точки (`kMaxPasses` в `dng_color_spec::NeutralToXY`).
const MAX_WHITE_SEARCH_PASSES: u32 = 30;

/// Порог сходимости `|Δx| + |Δy|` между проходами (как в SDK: `0.0000001`).
const WHITE_CONVERGENCE_THRESHOLD: f64 = 1e-7;

/// Нейтраль камеры задана неверно: нужны три конечных положительных числа.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "нейтраль камеры {values:?} некорректна: нужны три конечных числа больше нуля, \
     отношение которых конечно в f32 (иначе усиления баланса бесконечны)"
)]
pub struct InvalidCameraNeutral {
    pub values: [f64; 3],
}

/// Нейтраль камеры (`AsShotNeutral` в DNG): сырой RGB-отклик камеры на нейтральный (серый)
/// объект в сцене, обычно нормированный на G = 1. Источники: `neutral_color_point` из HAL
/// (Camera2) или собственный автобаланс белого (сессия 08). Для баланса белого
/// множители каналов — обратные величины нейтрали.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraNeutral([f64; 3]);

impl CameraNeutral {
    pub fn new(values: [f64; 3]) -> Result<Self, InvalidCameraNeutral> {
        let positive = values.iter().all(|value| value.is_finite() && *value > 0.0);
        let brightest = values.iter().copied().fold(f64::MIN, f64::max);
        let dimmest = values.iter().copied().fold(f64::MAX, f64::min);
        // Усиление баланса max/min применяется в f32 (как в шейдере): оно обязано быть конечным.
        let gains_fit_f32 = ((brightest / dimmest) as f32).is_finite();
        if positive && gains_fit_f32 {
            Ok(Self(values))
        } else {
            Err(InvalidCameraNeutral { values })
        }
    }

    pub fn values(self) -> [f64; 3] {
        self.0
    }
}

/// Калибровка камеры под один эталонный свет (набор тегов DNG с суффиксом 1 или 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DngCalibration {
    /// Температура эталонного света (из `CalibrationIlluminant`).
    pub temperature: Kelvin,
    /// ColorMatrix: XYZ → камера.
    pub color_matrix: Matrix3,
    /// ForwardMatrix: камера → XYZ D50; `None`, если профиль её не содержит.
    pub forward_matrix: Option<Matrix3>,
    /// CameraCalibration: поправка конкретного экземпляра камеры.
    pub camera_calibration: Matrix3,
}

/// Что пошло не так при сборке цветового профиля или расчёте матрицы.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ColorProfileError {
    #[error("в метаданных нет ни одной калибровки цвета")]
    NoCalibration,
    #[error("калибровок цвета {count}, DNG допускает одну или две")]
    TooManyCalibrations { count: usize },
    #[error("калибровка {set_index}: матрица {matrix} содержит нулевой знаменатель")]
    BrokenMatrixElement {
        set_index: usize,
        matrix: &'static str,
        #[source]
        source: BrokenRational,
    },
    #[error("калибровка {set_index}: {source}")]
    UnknownIlluminant {
        set_index: usize,
        #[source]
        source: UnknownCalibrationIlluminant,
    },
    #[error("ForwardMatrix есть не у всех калибровок: DNG требует обе или ни одной")]
    MixedForwardMatrices,
    #[error("AnalogBalance {values:?} некорректен: нужны три конечных числа больше нуля")]
    InvalidAnalogBalance { values: Vector3 },
    #[error("не удалось обратить матрицу ({context}): {source}")]
    Singular {
        context: &'static str,
        #[source]
        source: SingularMatrix,
    },
    #[error("для нейтрали не определена белая точка (цветность вне допустимой области)")]
    UndefinedWhitePoint,
    #[error("нейтраль камеры отображается в нулевую яркость, нормировать матрицу нельзя")]
    ZeroLuminanceNeutral,
    #[error("белая точка дала недопустимую нейтраль камеры: {0}")]
    InvalidNeutralForWhite(#[source] InvalidCameraNeutral),
}

/// Чем закончился поиск белой точки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convergence {
    /// Итерации сошлись за `passes` проходов (`|Δx| + |Δy| < 1e-7`).
    Converged { passes: u32 },
    /// За 30 проходов сходимости нет — почти всегда двузначное колебание между соседними
    /// оценками; взято среднее двух последних (как в SDK).
    AveragedAfterOscillation,
}

/// Белая точка сцены, найденная по нейтрали камеры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteEstimate {
    pub white: Chromaticity,
    /// Температура этой белой точки по Робертсону — по ней интерполируются матрицы.
    pub temperature: Kelvin,
    pub convergence: Convergence,
}

/// Матрица камера → XYZ D50 и то, из чего она получена.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraToXyz {
    /// Камера (линейный RGB до баланса белого) → XYZ D50.
    pub matrix: Matrix3,
    pub white: WhiteEstimate,
    /// `ReferenceNeutral = (AB · CC)⁻¹ · нейтраль` (спецификация DNG, «Camera to XYZ Matrix»).
    pub reference_neutral: Vector3,
}

/// Матрицы профиля, интерполированные для одной температуры.
#[derive(Debug, Clone, Copy)]
struct InterpolatedMatrices {
    color_matrix: Matrix3,
    camera_calibration: Matrix3,
    forward_matrix: Option<Matrix3>,
}

/// Цветовой профиль камеры по DNG: одна или две калибровки и AnalogBalance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DngColorProfile {
    first: DngCalibration,
    second: Option<DngCalibration>,
    analog_balance: Vector3,
}

impl DngColorProfile {
    /// Собирает профиль. Калибровки можно давать в любом порядке температур: вес считается
    /// симметрично. `ForwardMatrix` должна быть у всех калибровок или ни у одной.
    pub fn new(
        first: DngCalibration,
        second: Option<DngCalibration>,
        analog_balance: Vector3,
    ) -> Result<Self, ColorProfileError> {
        if !analog_balance.iter().all(|value| value.is_finite() && *value > 0.0) {
            return Err(ColorProfileError::InvalidAnalogBalance { values: analog_balance });
        }
        if let Some(second) = &second
            && first.forward_matrix.is_some() != second.forward_matrix.is_some()
        {
            return Err(ColorProfileError::MixedForwardMatrices);
        }
        Ok(Self {
            first: normalize_forward_matrix(first)?,
            second: second.map(normalize_forward_matrix).transpose()?,
            analog_balance,
        })
    }

    /// Профиль из наборов калибровки HAL (`RawSensorInfo::color_calibration`).
    ///
    /// ForwardMatrix из одних нулей означает «не задана» (так Camera2 помечает отсутствие).
    /// AnalogBalance у Camera2 нет, поэтому `[1, 1, 1]`.
    pub fn from_calibration_sets(sets: &[ColorCalibrationSet]) -> Result<Self, ColorProfileError> {
        match sets {
            [] => Err(ColorProfileError::NoCalibration),
            [only] => Self::new(calibration_from_set(0, only)?, None, NEUTRAL_ANALOG_BALANCE),
            [first, second] => Self::new(
                calibration_from_set(0, first)?,
                Some(calibration_from_set(1, second)?),
                NEUTRAL_ANALOG_BALANCE,
            ),
            more => Err(ColorProfileError::TooManyCalibrations { count: more.len() }),
        }
    }

    /// Белая точка сцены по нейтрали камеры — итерация `NeutralToXY` из SDK.
    ///
    /// Нейтраль и белая точка зависят друг от друга (матрица зависит от температуры белой),
    /// поэтому: старт в D50; на каждом проходе берём `XYZtoCamera(xy)`, переводим нейтраль в XYZ
    /// обратной матрицей и берём её цветность как следующее приближение. Стоп при
    /// `|Δx| + |Δy| < 1e-7`; после 30 проходов без сходимости — среднее двух последних оценок.
    pub fn white_for_neutral(&self, neutral: CameraNeutral) -> Result<WhiteEstimate, ColorProfileError> {
        let (white, convergence) = find_fixed_point_white(Chromaticity::D50, |guess| {
            let xyz_to_camera = self.xyz_to_camera(guess)?;
            let xyz = xyz_to_camera
                .inverse()
                .map_err(|source| ColorProfileError::Singular {
                    context: "XYZtoCamera",
                    source,
                })?
                .apply(neutral.values());
            Chromaticity::from_xyz(xyz).ok_or(ColorProfileError::UndefinedWhitePoint)
        })?;
        Ok(WhiteEstimate {
            white,
            temperature: self.temperature_of(white)?,
            convergence,
        })
    }

    /// Нейтраль камеры для белой точки сцены — обратная к [`Self::white_for_neutral`] операция.
    ///
    /// Спецификация DNG 1.6, «Translating White Balance xy Coordinates to Camera Neutral
    /// Coordinates»: температура белой по Робертсону → матрицы интерполируются по майредам
    /// (как в `white_for_neutral`) → `нейтраль = XYZtoCamera(T) · XYZ(xy, Y = 1)`.
    /// Нормировка — на канал G (`G = 1`): так нейтраль хранят HAL и colour-hdri, а множители
    /// баланса белого получаются как `1 / нейтраль`. Неинтерполируемая (вне таблицы) температура
    /// прижимается к краю, как в SDK: интерполяция матриц и так прижата к крайним калибровкам.
    pub fn neutral_for_white(&self, white: Chromaticity) -> Result<CameraNeutral, ColorProfileError> {
        // `y > 0` нужно для XYZ с Y = 1; `temperature_of` отсеивает остальные вырожденные случаи.
        if !white.x.is_finite() || !white.y.is_finite() || white.y <= 0.0 {
            return Err(ColorProfileError::UndefinedWhitePoint);
        }
        let camera_response = self.xyz_to_camera(white)?.apply(white.to_xyz());
        let green = camera_response[1];
        if !green.is_finite() || green <= 0.0 {
            return Err(ColorProfileError::ZeroLuminanceNeutral);
        }
        CameraNeutral::new(camera_response.map(|channel| channel / green))
            .map_err(ColorProfileError::InvalidNeutralForWhite)
    }

    /// Матрица камера → XYZ D50 для данной нейтрали (спецификация DNG, «Camera to XYZ Matrix»).
    ///
    /// С ForwardMatrix: `ReferenceNeutral = (AB·CC)⁻¹·нейтраль`, `D = diag(ReferenceNeutral)⁻¹`,
    /// `матрица = FM(T) · D · (AB·CC)⁻¹`. FM нормированы при сборке профиля
    /// ([`normalize_forward_matrix`]), поэтому сбалансированный белый (1, 1, 1) уходит ровно
    /// в XYZ D50 — серый остаётся серым даже при округлённых матрицах HAL.
    ///
    /// Без ForwardMatrix: `Брэдфорд(белая → D50) · XYZtoCamera(белая)⁻¹`, затем делим матрицу
    /// на яркость `Y` образа нейтрали, чтобы нейтраль давала ровно D50 с `Y = 1`. SDK нормирует
    /// иначе (по максимуму компоненты белого камеры), но это разница в одном скаляре
    /// (экспозиции); общая яркость кадра на стыке с тоном всё равно задаётся отдельно.
    pub fn camera_to_xyz_d50(&self, neutral: CameraNeutral) -> Result<CameraToXyz, ColorProfileError> {
        let white = self.white_for_neutral(neutral)?;
        let matrices = self.interpolate(white.temperature);
        let analog_and_calibration = Matrix3::diagonal(self.analog_balance).mul(&matrices.camera_calibration);
        let reference_to_camera_inverse =
            analog_and_calibration
                .inverse()
                .map_err(|source| ColorProfileError::Singular {
                    context: "AnalogBalance · CameraCalibration",
                    source,
                })?;
        let reference_neutral = reference_to_camera_inverse.apply(neutral.values());

        let matrix = match matrices.forward_matrix {
            Some(forward_matrix) => {
                let balance =
                    Matrix3::diagonal(reference_neutral)
                        .inverse()
                        .map_err(|source| ColorProfileError::Singular {
                            context: "ReferenceNeutral",
                            source,
                        })?;
                forward_matrix.mul(&balance).mul(&reference_to_camera_inverse)
            }
            None => self.matrix_without_forward(white, neutral)?,
        };
        Ok(CameraToXyz {
            matrix,
            white,
            reference_neutral,
        })
    }

    /// Путь без ForwardMatrix: обращённая `XYZtoCamera`, Брэдфорд к D50 и нормировка по яркости.
    fn matrix_without_forward(
        &self,
        white: WhiteEstimate,
        neutral: CameraNeutral,
    ) -> Result<Matrix3, ColorProfileError> {
        let camera_to_scene_xyz =
            self.xyz_to_camera(white.white)?
                .inverse()
                .map_err(|source| ColorProfileError::Singular {
                    context: "XYZtoCamera",
                    source,
                })?;
        // Адаптируем от цветности, которую нейтраль имеет в этой матрице на самом деле: она
        // отличается от `white.white` на порог сходимости (1e-7), а так нейтраль уходит в D50 точно.
        let neutral_xyz = camera_to_scene_xyz.apply(neutral.values());
        let source_white = Chromaticity::from_xyz(neutral_xyz).ok_or(ColorProfileError::UndefinedWhitePoint)?;
        let adaptation =
            bradford_adaptation(source_white, Chromaticity::D50).map_err(|source| ColorProfileError::Singular {
                context: "Брэдфорд к D50",
                source,
            })?;
        let unnormalized = adaptation.mul(&camera_to_scene_xyz);
        // Яркость образа нейтрали — вторая строка матрицы (Y), применённая к нейтрали.
        let neutral_luminance = unnormalized.apply(neutral.values())[1];
        if !neutral_luminance.is_finite() || neutral_luminance.abs() < f64::EPSILON {
            return Err(ColorProfileError::ZeroLuminanceNeutral);
        }
        let mut rows = unnormalized.rows;
        for value in rows.iter_mut().flatten() {
            *value /= neutral_luminance;
        }
        Ok(Matrix3 { rows })
    }

    /// `XYZtoCamera = AB · CC(T) · CM(T)` для белой точки `white`.
    fn xyz_to_camera(&self, white: Chromaticity) -> Result<Matrix3, ColorProfileError> {
        let matrices = self.interpolate(self.temperature_of(white)?);
        Ok(Matrix3::diagonal(self.analog_balance)
            .mul(&matrices.camera_calibration)
            .mul(&matrices.color_matrix))
    }

    fn temperature_of(&self, white: Chromaticity) -> Result<Kelvin, ColorProfileError> {
        let temperature = temperature_from_chromaticity(white);
        if temperature.0.is_finite() {
            Ok(temperature)
        } else {
            Err(ColorProfileError::UndefinedWhitePoint)
        }
    }

    /// Интерполяция матриц по температуре `T`, линейно по майредам (`1/T`), с прижимом
    /// к крайним калибровкам. Вес первой калибровки
    /// `clamp((1/T − 1/T₂) / (1/T₁ − 1/T₂), 0, 1)` не зависит от порядка калибровок.
    fn interpolate(&self, temperature: Kelvin) -> InterpolatedMatrices {
        let first = &self.first;
        let Some(second) = &self.second else {
            return InterpolatedMatrices {
                color_matrix: first.color_matrix,
                camera_calibration: first.camera_calibration,
                forward_matrix: first.forward_matrix,
            };
        };
        let span = first.temperature.mired() - second.temperature.mired();
        let first_weight = if span.abs() < f64::EPSILON {
            // Обе калибровки на одной температуре: смешивать не из чего.
            1.0
        } else {
            ((temperature.mired() - second.temperature.mired()) / span).clamp(0.0, 1.0)
        };
        InterpolatedMatrices {
            color_matrix: first.color_matrix.blend(&second.color_matrix, first_weight),
            camera_calibration: first.camera_calibration.blend(&second.camera_calibration, first_weight),
            forward_matrix: match (first.forward_matrix, second.forward_matrix) {
                (Some(first_forward), Some(second_forward)) => Some(first_forward.blend(&second_forward, first_weight)),
                // `new` гарантирует обе или ни одной; запасной вариант — калибровка с матрицей.
                (single, None) | (None, single) => single,
            },
        }
    }
}

/// У Camera2 нет AnalogBalance — нейтральные множители.
const NEUTRAL_ANALOG_BALANCE: Vector3 = [1.0, 1.0, 1.0];

/// Поиск неподвижной точки отображения «белая → белая»: сердцевина `NeutralToXY`.
/// Вынесено отдельно, чтобы ветка колебания проверялась тестом без подбора реального профиля.
fn find_fixed_point_white(
    start: Chromaticity,
    mut next_estimate: impl FnMut(Chromaticity) -> Result<Chromaticity, ColorProfileError>,
) -> Result<(Chromaticity, Convergence), ColorProfileError> {
    let mut last = start;
    for pass in 0..MAX_WHITE_SEARCH_PASSES {
        let mut next = next_estimate(last)?;
        if (next.x - last.x).abs() + (next.y - last.y).abs() < WHITE_CONVERGENCE_THRESHOLD {
            return Ok((next, Convergence::Converged { passes: pass + 1 }));
        }
        if pass == MAX_WHITE_SEARCH_PASSES - 1 {
            // Скорее всего двузначное колебание: берём середину двух последних оценок и сдаёмся.
            next = Chromaticity {
                x: (last.x + next.x) * 0.5,
                y: (last.y + next.y) * 0.5,
            };
            return Ok((next, Convergence::AveragedAfterOscillation));
        }
        last = next;
    }
    // Недостижимо: последний проход всегда возвращается внутри цикла.
    Ok((last, Convergence::AveragedAfterOscillation))
}

fn calibration_from_set(set_index: usize, set: &ColorCalibrationSet) -> Result<DngCalibration, ColorProfileError> {
    let to_matrix = |matrix: &Matrix3x3, name: &'static str| {
        Matrix3::from_rationals(matrix).map_err(|source| ColorProfileError::BrokenMatrixElement {
            set_index,
            matrix: name,
            source,
        })
    };
    let temperature = calibration_illuminant_temperature(set.illuminant)
        .map_err(|source| ColorProfileError::UnknownIlluminant { set_index, source })?;
    let forward_matrix = to_matrix(&set.forward_matrix, "ForwardMatrix")?;
    // Нулевая ForwardMatrix — признак «не задана»: у настоящей определитель не нулевой.
    let forward_matrix = (forward_matrix != Matrix3::from_rows([[0.0; 3]; 3])).then_some(forward_matrix);
    Ok(DngCalibration {
        temperature,
        color_matrix: to_matrix(&set.color_matrix, "ColorMatrix")?,
        forward_matrix,
        camera_calibration: to_matrix(&set.calibration_transform, "CameraCalibration")?,
    })
}

/// Нормирует ForwardMatrix калибровки так, чтобы `FM · (1, 1, 1) = XYZ(D50)`:
/// `FM' = diag(XYZ_D50) · diag(FM·1)⁻¹ · FM` — как `dng_camera_profile::NormalizeForwardMatrix`,
/// который Adobe DNG SDK вызывает при построении `dng_color_spec` для обеих FM.
///
/// Зачем: HAL отдаёт FM, округлённую до 1/128 (OnePlus 11: `FM·1 = (0,9609; 1; 0,8203)` против
/// D50 `(0,9643; 1; 0,8251)`). Без нормировки серый получил бы лёгкий цветной оттенок.
fn normalize_forward_matrix(calibration: DngCalibration) -> Result<DngCalibration, ColorProfileError> {
    let Some(forward_matrix) = calibration.forward_matrix else {
        return Ok(calibration);
    };
    let balanced_white_xyz = forward_matrix.apply([1.0, 1.0, 1.0]);
    let undo_current_white =
        Matrix3::diagonal(balanced_white_xyz)
            .inverse()
            .map_err(|source| ColorProfileError::Singular {
                context: "ForwardMatrix · (1, 1, 1)",
                source,
            })?;
    let normalized = Matrix3::diagonal(Chromaticity::D50.to_xyz())
        .mul(&undo_current_white)
        .mul(&forward_matrix);
    Ok(DngCalibration {
        forward_matrix: Some(normalized),
        ..calibration
    })
}

#[cfg(test)]
#[path = "dng_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "dng_neutral_tests.rs"]
mod neutral_tests;
