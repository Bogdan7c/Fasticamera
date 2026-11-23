//! Таблица вида (LUT) на GPU: байты для `shaders/look_stage.glsl` и host-буфер, который их хранит. Общий для
//! видоискателя, качественного конвейера и проявления снимка (по образцу `lens_shading_map`).
//!
//! # Раскладка буфера (std430, зеркало `look_stage.glsl`, менять только вместе)
//! | Смещение | Тип | Содержимое |
//! |---|---|---|
//! | 0 | `uvec4` | x — узлов на ось `N` (0 — заглушка «таблицы нет»), y, z, w — 0 |
//! | 16 | `float[]` | узлы `isp_model::look::lut::LookLut::nodes` подряд по три числа (R, G, B): узел `(r, g, b)` лежит в `3·(r + N·(g + N·b))` |
//!
//! Узлы — по 12 байт (а не `vec4` по 16): таблица 65³ занимает 3,3 МиБ вместо 4,4.
//!
//! # Кто за что отвечает
//! * [`LookLutLimits`] — наибольшая таблица, под которую выделен буфер (конфиг владельца конвейера; умолчание
//!   берётся из модели вида `isp-model`, чтобы предел параметров и предел буфера совпадали). Таблица больше —
//!   явная ошибка [`LookLutError::TooLarge`], а не молчаливый пропуск (пропуск тихо сменил бы вид картинки).
//! * [`LookLutBuffer`] — один host-буфер (слот кадра или одна проявка): пишет таблицу только когда она сменилась
//!   (тот же `Arc` — ничего не пишется), отсутствие таблицы пишет заглушку.
//!
//! Шейдер не проверяет границы таблицы: все проверки — здесь, до записи команд.

use std::sync::Arc;

use gpu_vulkan::compute::DescriptorResource;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use isp_model::look::lut::LookLut;
use isp_model::look::model::{LookModelSettings, MAX_LUT_SIZE_LIMIT};

/// Байт в заголовке буфера (`uvec4`).
const HEADER_BYTES: usize = 16;
/// Байт на узел: три `f32`.
const NODE_BYTES: usize = 12;

/// Наибольшая таблица, помещающаяся в буфер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookLutLimits {
    /// Наибольшее число узлов на ось.
    pub max_size: u32,
}

impl Default for LookLutLimits {
    /// Предел модели вида по умолчанию (65 узлов на ось, `[estimate]` там): 65³·12 байт = 3,3 МиБ на буфер.
    fn default() -> Self {
        Self {
            max_size: LookModelSettings::default().max_lut_size,
        }
    }
}

impl LookLutLimits {
    /// Размер буфера под наибольшую таблицу, байт.
    pub fn buffer_bytes(self) -> u64 {
        HEADER_BYTES as u64 + u64::from(self.max_size).pow(3) * NODE_BYTES as u64
    }

    /// Лимит имеет смысл: от 2 узлов до предела конфига модели ([`MAX_LUT_SIZE_LIMIT`]).
    pub fn validate(self) -> Result<(), LookLutError> {
        if self.max_size < 2 || self.max_size > MAX_LUT_SIZE_LIMIT {
            return Err(LookLutError::InvalidLimits { limits: self });
        }
        Ok(())
    }
}

/// Почему таблицу нельзя положить в буфер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LookLutError {
    /// Таблица больше буфера: она НЕ применяется молча, вызывающий обязан узнать.
    #[error("таблица вида {size}³ больше буфера {max}³")]
    TooLarge {
        /// Узлов на ось у таблицы.
        size: u32,
        /// Лимит буфера.
        max: u32,
    },
    /// Лимит таблицы не годится для буфера.
    #[error("лимит таблицы вида {limits:?} не годится: нужно от 2 до {MAX_LUT_SIZE_LIMIT} узлов на ось")]
    InvalidLimits {
        /// Лимит из конфига.
        limits: LookLutLimits,
    },
}

/// Байты буфера: таблица `lut` или заглушка («таблицы нет»), с проверкой по `limits`.
pub(crate) fn pack_look_lut(lut: Option<&LookLut>, limits: LookLutLimits) -> Result<Vec<u8>, LookLutError> {
    let Some(lut) = lut else {
        return Ok(pack_header(0));
    };
    check_lut_size(lut, limits)?;
    let mut bytes = pack_header(lut.size());
    bytes.reserve(lut.nodes().len() * NODE_BYTES);
    for node in lut.nodes() {
        bytes.extend(node.iter().flat_map(|value| value.to_ne_bytes()));
    }
    Ok(bytes)
}

/// Помещается ли таблица в буфер с лимитом `limits` (без записи).
pub(crate) fn check_lut_size(lut: &LookLut, limits: LookLutLimits) -> Result<(), LookLutError> {
    if lut.size() > limits.max_size {
        return Err(LookLutError::TooLarge {
            size: lut.size(),
            max: limits.max_size,
        });
    }
    Ok(())
}

fn pack_header(size: u32) -> Vec<u8> {
    [size, 0, 0, 0].iter().flat_map(|word| word.to_ne_bytes()).collect()
}

/// Что сейчас лежит в буфере (чтобы не писать то же самое каждый кадр).
enum Contents {
    /// Ничего не писали: содержимое не определено.
    Unwritten,
    /// Заглушка «таблицы нет».
    Absent,
    /// Таблица. `Arc` держим, чтобы адрес не мог быть переиспользован другой таблицей.
    Lut(Arc<LookLut>),
}

impl Contents {
    fn matches(&self, wanted: Option<&Arc<LookLut>>) -> bool {
        match (self, wanted) {
            (Self::Absent, None) => true,
            (Self::Lut(stored), Some(wanted)) => Arc::ptr_eq(stored, wanted),
            _ => false,
        }
    }
}

/// Host-буфер таблицы одного владельца (слот кадра видоискателя, слот качественного конвейера, одна проявка).
///
/// Контракт записи тот же, что у uniform-буфера слота: писать, только когда GPU закончил читать прошлое
/// содержимое (fence слота сигнален).
pub(crate) struct LookLutBuffer {
    buffer: HostBuffer,
    limits: LookLutLimits,
    contents: Contents,
}

impl LookLutBuffer {
    /// Выделяет буфер под наибольшую таблицу `limits`. Содержимое — заглушка (буфер всегда пригоден для привязки).
    pub(crate) fn new(context: &GpuContext, limits: LookLutLimits) -> Result<Self, LookLutBufferError> {
        limits.validate()?;
        let mut buffer = HostBuffer::new(context, limits.buffer_bytes(), HostBufferUsage::ReadOnlyStorage)?;
        buffer.write_bytes(0, &pack_header(0))?;
        Ok(Self {
            buffer,
            limits,
            contents: Contents::Absent,
        })
    }

    /// Приводит содержимое к таблице `lut`. Пишет в буфер только если оно изменилось; таблица больше лимита —
    /// ошибка и буфер НЕ тронут.
    pub(crate) fn prepare(&mut self, lut: Option<&Arc<LookLut>>) -> Result<(), LookLutBufferError> {
        if self.contents.matches(lut) {
            return Ok(());
        }
        let bytes = pack_look_lut(lut.map(Arc::as_ref), self.limits)?;
        // Пока пишем, содержимое неопределено: ошибка записи не оставит ложного «совпадает».
        self.contents = Contents::Unwritten;
        self.buffer.write_bytes(0, &bytes)?;
        self.contents = match lut {
            Some(lut) => Contents::Lut(Arc::clone(lut)),
            None => Contents::Absent,
        };
        Ok(())
    }

    /// Ресурс для привязки (`BindingKind::ReadOnlyStorageBuffer`).
    pub(crate) fn binding(&self) -> DescriptorResource {
        self.buffer.binding()
    }
}

/// Ошибка ресурсов вида: таблица не годится или сломался GPU — вызывающие переводят каждую в свою ошибку.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LookLutBufferError {
    /// Таблица не помещается или лимит не годится.
    #[error(transparent)]
    Lut(#[from] LookLutError),
    /// Ошибка Vulkan-набора.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

#[cfg(test)]
mod tests;
