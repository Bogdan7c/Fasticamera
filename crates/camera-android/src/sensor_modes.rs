//! Таблица режимов сенсора из вендорской характеристики Qualcomm (`SensorModeTable`).
//!
//! Публичные характеристики Camera2 не различают камеры с родным режимом сенсора на 60 к/с
//! (ширик) и без него (фронт): диапазоны AE у всех обрезаны на 30. Вендорская таблица —
//! единственный источник правды «какой размер сенсора на какой частоте умеет HAL».
//!
//! Зачем это знать: если просить `AE [60,60]` для размера, у которого нет родного режима на
//! 60 к/с, HAL молча берёт полноразмерный режим (фронт: 4608×3456 вместо 2304×1728), а буфер
//! reader-а меньше — провайдер камеры падает по `SIGABRT` в `BindImportedBuffer`. Поэтому
//! бэкенд обещает AE-диапазон выше публичного потолка **только** при подтверждённом режиме.
//!
//! Формат записи (int32): `[число_режимов, полей_на_режим(=3), (ширина, высота, макс_к/с)…]`.
//! Тег ищется по имени ([`SENSOR_MODE_TABLE_TAG_NAME`] через реестр `vendor`): номер зависит от прошивки.
//!
//! Три исхода чтения ([`SensorModeAvailability`]) планировщик не склеивает в один `bool`:
//! * таблица есть и разобрана — [`SensorModeAvailability::Listed`];
//! * устройство такого тега не знает или запись отсутствует — [`SensorModeAvailability::AbsentOnDevice`];
//! * тег есть, но запись нечитаема или битая — [`SensorModeAvailability::Unusable`] (причина — для лога).

use camera_contract::geometry::PixelSize;
use camera_contract::stream::TargetFrameRate;

use crate::entry::{EntryProblem, MetadataSource, read_int32s};
use crate::tags::Tag;
use crate::vendor::{VendorCapabilities, VendorKey, VendorSupport, VendorUnsupportedReason};

/// Имя вендорского тега в `dumpsys media.camera` (раздел «Vendor tags»). Единственное место,
/// где оно записано: реестр `vendor` берёт его отсюда в [`crate::vendor::VENDOR_TAG_SPECS`].
pub const SENSOR_MODE_TABLE_TAG_NAME: &str = "org.quic.camera2.sensormode.info.SensorModeTable";

/// Сколько полей описывает один режим в поддерживаемом формате: ширина, высота, макс. частота.
const FIELDS_PER_MODE: usize = 3;
/// Служебный заголовок записи: число режимов и число полей на режим.
const HEADER_LEN: usize = 2;

/// Один режим сенсора из таблицы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorMode {
    /// Размер кадра сенсора в этом режиме.
    pub size: PixelSize,
    /// Максимальная частота режима, к/с.
    pub max_frames_per_second: u32,
}

/// Подходит ли таблица для запрошенного режима. Два исхода — не `bool`, чтобы место вызова читалось.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeModeSupport {
    /// Есть режим сенсора ровно этого размера с частотой не ниже запрошенной.
    Offered,
    /// Такого режима нет (или таблица недоступна — см. [`SensorModeAvailability`]).
    NotOffered,
}

/// Почему массив не удалось разобрать в таблицу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SensorModeParseError {
    #[error("формат таблицы режимов: {actual} полей на режим вместо {FIELDS_PER_MODE}")]
    UnsupportedFieldCount { actual: i32 },
    #[error("число режимов в заголовке не положительно: {declared}")]
    NonPositiveModeCount { declared: i32 },
    #[error("длина записи {actual} не сходится с заголовком (ждали {expected})")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("режим №{mode_index}: размер или частота не положительны")]
    NonPositiveValue { mode_index: usize },
}

/// Разобранная таблица режимов одной камеры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorModeTable {
    modes: Vec<SensorMode>,
}

impl SensorModeTable {
    /// Разбирает массив int32 строго: любое расхождение с форматом — явная ошибка, а не пропуск.
    pub fn parse(values: &[i32]) -> Result<Self, SensorModeParseError> {
        let [declared_count, declared_fields, rest @ ..] = values else {
            return Err(SensorModeParseError::LengthMismatch {
                expected: HEADER_LEN,
                actual: values.len(),
            });
        };
        if usize::try_from(*declared_fields) != Ok(FIELDS_PER_MODE) {
            return Err(SensorModeParseError::UnsupportedFieldCount {
                actual: *declared_fields,
            });
        }
        let mode_count = usize::try_from(*declared_count).ok().filter(|count| *count > 0).ok_or(
            SensorModeParseError::NonPositiveModeCount {
                declared: *declared_count,
            },
        )?;
        // Проверка длины до нарезки: `mode_count` пришёл от HAL, переполнения умножения быть не должно.
        let expected = mode_count
            .checked_mul(FIELDS_PER_MODE)
            .and_then(|body| body.checked_add(HEADER_LEN))
            .unwrap_or(usize::MAX);
        if values.len() != expected {
            return Err(SensorModeParseError::LengthMismatch {
                expected,
                actual: values.len(),
            });
        }
        let modes = rest
            .chunks_exact(FIELDS_PER_MODE)
            .enumerate()
            .map(|(mode_index, fields)| parse_mode(mode_index, fields))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { modes })
    }

    /// Режимы в порядке таблицы.
    pub fn modes(&self) -> &[SensorMode] {
        &self.modes
    }

    /// Есть ли родной режим сенсора ровно такого размера с частотой не ниже запрошенной.
    ///
    /// Размер сравнивается точно: HAL выбирает режим по размеру буфера/вырезки, а «режим побольше»
    /// и есть тот самый полноразмерный режим, на котором падает провайдер.
    pub fn native_mode_support(&self, readout: PixelSize, requested: TargetFrameRate) -> NativeModeSupport {
        let offered = self
            .modes
            .iter()
            .any(|mode| mode.size == readout && mode.max_frames_per_second >= requested.frames_per_second());
        if offered {
            NativeModeSupport::Offered
        } else {
            NativeModeSupport::NotOffered
        }
    }
}

fn parse_mode(mode_index: usize, fields: &[i32]) -> Result<SensorMode, SensorModeParseError> {
    let positive = |value: i32| u32::try_from(value).ok().filter(|value| *value > 0);
    match fields {
        [width, height, fps] => match (positive(*width), positive(*height), positive(*fps)) {
            (Some(width), Some(height), Some(max_frames_per_second)) => Ok(SensorMode {
                size: PixelSize::new(width, height),
                max_frames_per_second,
            }),
            _ => Err(SensorModeParseError::NonPositiveValue { mode_index }),
        },
        // `chunks_exact(FIELDS_PER_MODE)` другой длины не отдаёт; ветка — защита от рассинхрона констант.
        _ => Err(SensorModeParseError::NonPositiveValue { mode_index }),
    }
}

/// Почему тег таблицы есть, но пользоваться ею нельзя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SensorModeProblem {
    #[error("номер вендорского тега не получен, код {status}")]
    TagResolveFailed { status: i32 },
    #[error("запись таблицы непригодна: {0}")]
    Entry(EntryProblem),
    #[error("таблица битая: {0}")]
    Malformed(SensorModeParseError),
}

/// Что известно о таблице режимов камеры. Хранится в записи каталога рядом с `VendorCapabilities`
/// (тот владеет номерами тегов, а эта таблица — прочитанным **значением** характеристики).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorModeAvailability {
    /// Таблица прочитана и разобрана.
    Listed(SensorModeTable),
    /// Эта прошивка таблицу не отдаёт (тег неизвестен или записи нет).
    AbsentOnDevice,
    /// Тег есть, но читать или разбирать его нельзя; причина нужна для лога и диагностики.
    Unusable(SensorModeProblem),
}

impl SensorModeAvailability {
    /// Читает таблицу камеры. Номер тега берётся из реестра `vendor`, значение — из характеристик.
    pub fn read(source: &dyn MetadataSource, vendor: &VendorCapabilities) -> Self {
        let code = match vendor.support(VendorKey::SensorModeTable) {
            VendorSupport::Available { tag } => tag,
            VendorSupport::Unsupported(VendorUnsupportedReason::ResolveFailed { status }) => {
                return Self::Unusable(SensorModeProblem::TagResolveFailed { status });
            }
            // Для характеристики «на любой камере» `NotAllowedForCamera` не бывает; ветка исчерпывает enum.
            VendorSupport::Unsupported(
                VendorUnsupportedReason::TagNotFound | VendorUnsupportedReason::NotAllowedForCamera,
            ) => {
                return Self::AbsentOnDevice;
            }
        };
        let tag = Tag {
            code,
            name: "vendor:sensor_mode_table",
        };
        match read_int32s(source, tag, HEADER_LEN) {
            Ok(None) => Self::AbsentOnDevice,
            Ok(Some(values)) => match SensorModeTable::parse(&values) {
                Ok(table) => Self::Listed(table),
                Err(error) => Self::Unusable(SensorModeProblem::Malformed(error)),
            },
            Err(problem) => Self::Unusable(SensorModeProblem::Entry(problem)),
        }
    }

    /// Предложен ли родной режим. Нет таблицы или она непригодна — `NotOffered`: цена ошибки в обратную
    /// сторону — падение провайдера камеры, а запасные 30 к/с дёшевы (см. `frame_rate_plan`).
    pub fn native_mode_support(&self, readout: PixelSize, requested: TargetFrameRate) -> NativeModeSupport {
        match self {
            Self::Listed(table) => table.native_mode_support(readout, requested),
            Self::AbsentOnDevice | Self::Unusable(_) => NativeModeSupport::NotOffered,
        }
    }
}

#[cfg(test)]
#[path = "sensor_modes_tests.rs"]
mod tests;
