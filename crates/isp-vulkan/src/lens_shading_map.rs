//! Карта коррекции виньетирования на GPU: пакет байтов для `shaders/lens_shading.glsl` и буфер, который
//! её хранит. Общий для видоискателя, качественного конвейера и проявления снимка.
//!
//! # Раскладка буфера (std430, зеркало `lens_shading.glsl`, менять только вместе)
//! | Смещение | Тип | Содержимое |
//! |---|---|---|
//! | 0 | `uvec4` | x — коррекция включена (1) или заглушка «пропуск» (0), y — столбцов, z — строк, w — 0 |
//! | 16 | `uvec4` | x, y — ширина и высота ПОЛЕЗНОЙ области (пиксели; нужны, чтобы последний пиксель попал ровно в последний узел), z, w — 0 |
//! | 32 | `vec4` | x, y — масштабы «пиксель → узел» `(узлов − 1)/(пикселей − 1)` по осям (f32, считает CPU один раз: `LensShadingGainMap::grid_scales`), z, w — 0 |
//! | 48 | `vec4[]` | точки карты по строкам, в каждой `[R, G_even, G_odd, B]` |
//!
//! # Кто за что отвечает
//! * [`LensShadingMapLimits`] — наибольшая карта, под которую выделен буфер (конфиг владельца конвейера).
//!   Карта больше лимита — явная ошибка [`LensShadingMapError::TooLarge`], а не молчаливый пропуск
//!   коррекции (пропуск оставил бы углы тёмными без всякого сигнала).
//! * [`LensShadingMapBuffer`] — один host-буфер (слот кадра или одна проявка): пишет карту только когда она
//!   изменилась (тот же `Arc` карты и та же полезная область — ничего не пишется), решение «пропуск»
//!   пишет заглушку.
//!
//! Шейдер не проверяет границы карты: все проверки (размер карты, полезная область не меньше 2×2) — здесь,
//! до записи команд.

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::DescriptorResource;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use isp_model::lens_shading::{LensShadingGainMap, LensShadingStep};

/// Байт в заголовке буфера: `uvec4`, `uvec4`, `vec4`.
const HEADER_BYTES: usize = 48;
/// Байт на точку карты: `vec4` из четырёх каналов.
const POINT_BYTES: usize = 16;

/// Наибольшая карта, помещающаяся в буфер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensShadingMapLimits {
    /// Наибольшее число столбцов карты.
    pub max_columns: u32,
    /// Наибольшее число строк карты.
    pub max_rows: u32,
}

impl Default for LensShadingMapLimits {
    /// 64×64 точек: калибровка OnePlus 11 — сетка 17×13, реальные карты Camera2 — порядка 17×13…64×48;
    /// 64×64×16 байт = 64 КиБ на буфер — пренебрежимо.
    fn default() -> Self {
        Self {
            max_columns: 64,
            max_rows: 64,
        }
    }
}

impl LensShadingMapLimits {
    /// Размер буфера под наибольшую карту, байт.
    pub fn buffer_bytes(self) -> u64 {
        (HEADER_BYTES as u64) + u64::from(self.max_columns) * u64::from(self.max_rows) * (POINT_BYTES as u64)
    }

    /// Лимиты имеют смысл: карта не меньше 2×2 (иначе билинейной сетки нет).
    pub fn validate(self) -> Result<(), LensShadingMapError> {
        if self.max_columns < 2 || self.max_rows < 2 {
            return Err(LensShadingMapError::InvalidLimits { limits: self });
        }
        Ok(())
    }
}

/// Почему карту нельзя положить в буфер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LensShadingMapError {
    /// Карта больше буфера: коррекция НЕ применяется молча — вызывающий обязан узнать.
    #[error("карта виньетирования {columns}×{rows} больше буфера {max_columns}×{max_rows}")]
    TooLarge {
        /// Столбцов в карте.
        columns: u32,
        /// Строк в карте.
        rows: u32,
        /// Лимит по столбцам.
        max_columns: u32,
        /// Лимит по строкам.
        max_rows: u32,
    },
    /// Полезная область меньше 2×2: у неё нет «последнего пикселя» для нормировки координаты.
    #[error("полезная область {size:?} слишком мала для карты виньетирования (нужно хотя бы 2×2)")]
    ValidAreaTooSmall {
        /// Размер полезной области.
        size: PixelSize,
    },
    /// Лимиты карты не годятся для буфера.
    #[error("лимиты карты виньетирования {limits:?} не годятся: нужно не меньше 2×2")]
    InvalidLimits {
        /// Лимиты из конфига.
        limits: LensShadingMapLimits,
    },
}

/// Байты буфера для решения стадии `step` над полезной областью `valid_area`.
///
/// `Skip(_)` — заглушка: заголовок с нулевым флагом «включено» (шейдер её не читает дальше заголовка).
/// `Apply(map)` — заголовок и точки карты; карта больше `limits` или область меньше 2×2 — ошибка.
pub(crate) fn pack_lens_shading_map(
    step: &LensShadingStep,
    valid_area: PixelSize,
    limits: LensShadingMapLimits,
) -> Result<Vec<u8>, LensShadingMapError> {
    check_lens_shading_step(step, valid_area, limits)?;
    match step {
        LensShadingStep::Skip(_) => Ok(pack_header(false, 0, 0, PixelSize::new(0, 0), [0.0, 0.0])),
        LensShadingStep::Apply(map) => {
            let scales = map.grid_scales(valid_area);
            let mut bytes = pack_header(true, map.columns(), map.rows(), valid_area, [scales.x, scales.y]);
            bytes.reserve(map.points().len() * POINT_BYTES);
            for point in map.points() {
                bytes.extend(point.iter().flat_map(|gain| gain.to_ne_bytes()));
            }
            Ok(bytes)
        }
    }
}

/// Годится ли решение стадии для буфера с лимитами `limits` (проверка без GPU и без записи). «Пропуск»
/// годится всегда; карта — если помещается и область не меньше 2×2.
pub(crate) fn check_lens_shading_step(
    step: &LensShadingStep,
    valid_area: PixelSize,
    limits: LensShadingMapLimits,
) -> Result<(), LensShadingMapError> {
    let LensShadingStep::Apply(map) = step else {
        return Ok(());
    };
    if map.columns() > limits.max_columns || map.rows() > limits.max_rows {
        return Err(LensShadingMapError::TooLarge {
            columns: map.columns(),
            rows: map.rows(),
            max_columns: limits.max_columns,
            max_rows: limits.max_rows,
        });
    }
    if valid_area.width < 2 || valid_area.height < 2 {
        return Err(LensShadingMapError::ValidAreaTooSmall { size: valid_area });
    }
    Ok(())
}

fn pack_header(enabled: bool, columns: u32, rows: u32, valid_area: PixelSize, scales: [f32; 2]) -> Vec<u8> {
    let words = [
        [u32::from(enabled), columns, rows, 0],
        [valid_area.width, valid_area.height, 0, 0],
        [scales[0].to_bits(), scales[1].to_bits(), 0, 0],
    ];
    words.iter().flatten().flat_map(|word| word.to_ne_bytes()).collect()
}

/// Что сейчас лежит в буфере (чтобы не писать то же самое каждый кадр).
enum Contents {
    /// Ничего не писали: содержимое не определено.
    Unwritten,
    /// Заглушка «пропуск».
    Disabled,
    /// Карта над полезной областью. `Arc` держим, чтобы адрес не мог быть переиспользован другой картой.
    Map {
        map: Arc<LensShadingGainMap>,
        valid_area: PixelSize,
    },
}

impl Contents {
    /// Совпадает ли содержимое с решением `step` над `valid_area` (тогда писать нечего).
    fn matches(&self, step: &LensShadingStep, valid_area: PixelSize) -> bool {
        match (self, step) {
            (Self::Disabled, LensShadingStep::Skip(_)) => true,
            (
                Self::Map {
                    map,
                    valid_area: stored,
                },
                LensShadingStep::Apply(wanted),
            ) => Arc::ptr_eq(map, wanted) && *stored == valid_area,
            _ => false,
        }
    }
}

/// Host-буфер карты одного владельца (слот кадра видоискателя, слот качественного конвейера, одна проявка).
///
/// Контракт записи тот же, что у uniform-буфера слота: писать, только когда GPU закончил читать прошлое
/// содержимое (fence слота сигнален).
pub(crate) struct LensShadingMapBuffer {
    buffer: HostBuffer,
    limits: LensShadingMapLimits,
    contents: Contents,
}

impl LensShadingMapBuffer {
    /// Выделяет буфер под наибольшую карту `limits`. Содержимое — заглушка «пропуск» (буфер всегда пригоден
    /// для привязки, даже если карту не пришлось писать).
    pub(crate) fn new(context: &GpuContext, limits: LensShadingMapLimits) -> Result<Self, LensShadingMapBufferError> {
        limits.validate()?;
        let mut buffer = HostBuffer::new(context, limits.buffer_bytes(), HostBufferUsage::ReadOnlyStorage)?;
        buffer.write_bytes(0, &pack_header(false, 0, 0, PixelSize::new(0, 0), [0.0, 0.0]))?;
        Ok(Self {
            buffer,
            limits,
            contents: Contents::Disabled,
        })
    }

    /// Приводит содержимое к решению `step` над полезной областью `valid_area`. Пишет в буфер только если
    /// оно изменилось; карта больше лимита — ошибка и буфер НЕ тронут.
    pub(crate) fn prepare(
        &mut self,
        step: &LensShadingStep,
        valid_area: PixelSize,
    ) -> Result<(), LensShadingMapBufferError> {
        if self.contents.matches(step, valid_area) {
            return Ok(());
        }
        let bytes = pack_lens_shading_map(step, valid_area, self.limits)?;
        // Пока пишем, содержимое неопределено: ошибка записи не оставит ложного «совпадает».
        self.contents = Contents::Unwritten;
        self.buffer.write_bytes(0, &bytes)?;
        self.contents = match step {
            LensShadingStep::Skip(_) => Contents::Disabled,
            LensShadingStep::Apply(map) => Contents::Map {
                map: Arc::clone(map),
                valid_area,
            },
        };
        Ok(())
    }

    /// Ресурс для привязки (`BindingKind::ReadOnlyStorageBuffer`).
    pub(crate) fn binding(&self) -> DescriptorResource {
        self.buffer.binding()
    }
}

/// Ошибка буфера карты: карта не годится или сломался GPU — вызывающие переводят каждую в свою ошибку.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LensShadingMapBufferError {
    /// Карта не помещается или область слишком мала.
    #[error(transparent)]
    Map(#[from] LensShadingMapError),
    /// Ошибка Vulkan-набора.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

#[cfg(test)]
#[path = "lens_shading_map_tests.rs"]
mod tests;
