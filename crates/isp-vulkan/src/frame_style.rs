//! Ресурсы «стиля кадра» одного владельца (слот кадра видоискателя, слот качественного конвейера, одна проявка):
//! блок основы тона ([`crate::tone_base`]), блок цвета вида и таблица вида ([`crate::look`]).
//!
//! Все три нужны трём путям одинаково: один владелец готовит их одним вызовом [`FrameStyleBuffers::prepare`]
//! и отдаёт привязки [`FrameStyleBindings`], поэтому путь не знает, сколько буферов за стилем стоит, а новая
//! модель основы или стадия вида не добавляет полей в каждый из трёх путей.
//!
//! Контракт записи — как у uniform-буфера слота: `prepare` вызывать, только когда GPU закончил читать прошлое
//! содержимое (fence слота сигнален).

use gpu_vulkan::compute::DescriptorResource;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use isp_model::params::IspParams;

use crate::look::lut_buffer::{LookLutBuffer, LookLutBufferError, check_lut_size};
use crate::look::{LOOK_UNIFORM_BYTES, LookLutError, LookLutLimits, pack_look_uniforms};
use crate::tone_base::{TONE_BASE_UNIFORM_BYTES, pack_tone_base_uniforms};

/// Стиль кадра не подготовлен: что именно помешало, различимо (таблица не влезла / сбой GPU). Пути переводят её
/// в свою ошибку вариант в вариант.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameStyleError {
    /// Таблица вида не помещается в буфер или сбой GPU при подготовке таблицы.
    #[error(transparent)]
    Look(#[from] LookLutBufferError),
    /// Сбой GPU-набора при записи блоков стиля.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Привязки ресурсов стиля для набора дескрипторов (порядок привязок — на стороне пути).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameStyleBindings {
    /// Блок основы тона (`ToneBaseParameters`, uniform).
    pub tone_base: DescriptorResource,
    /// Блок цвета вида (`LookParameters`, uniform).
    pub look: DescriptorResource,
    /// Таблица вида (`LookLutNodes`, read-only storage; заглушка, если таблицы нет).
    pub look_lut: DescriptorResource,
}

/// Помещается ли стиль кадра в буферы с лимитом таблицы `lut_limits` (проверка без GPU и без записи): владелец
/// зовёт её в `check_request` до выделений, чтобы отказать запросу заранее.
pub(crate) fn check_style_fits(params: &IspParams, lut_limits: LookLutLimits) -> Result<(), LookLutError> {
    match &params.look.lut {
        Some(lut) => check_lut_size(lut, lut_limits),
        None => Ok(()),
    }
}

/// Буферы стиля одного владельца.
pub(crate) struct FrameStyleBuffers {
    tone_base: HostBuffer,
    look: HostBuffer,
    look_lut: LookLutBuffer,
}

impl FrameStyleBuffers {
    /// Выделяет буферы; таблица — под наибольший размер `lut_limits`.
    pub(crate) fn new(context: &GpuContext, lut_limits: LookLutLimits) -> Result<Self, LookLutBufferError> {
        Ok(Self {
            tone_base: HostBuffer::new(
                context,
                TONE_BASE_UNIFORM_BYTES as u64,
                HostBufferUsage::UniformParameters,
            )?,
            look: HostBuffer::new(context, LOOK_UNIFORM_BYTES as u64, HostBufferUsage::UniformParameters)?,
            look_lut: LookLutBuffer::new(context, lut_limits)?,
        })
    }

    /// Приводит буферы к параметрам кадра: блоки пишутся каждый кадр (малы, меняются с подтяжкой AE и жестом
    /// ручки), таблица — только когда сменилась. Таблица больше лимита — ошибка ДО записи чего-либо (буферы
    /// остаются как были).
    pub(crate) fn prepare(&mut self, params: &IspParams) -> Result<(), FrameStyleError> {
        let tone_base_bytes = pack_tone_base_uniforms(params);
        // Таблица второй: её ошибка (слишком большая) не должна оставить блоки наполовину обновлёнными.
        self.look_lut.prepare(params.look.lut.as_ref())?;
        self.tone_base.write_bytes(0, &tone_base_bytes)?;
        self.look.write_bytes(0, &pack_look_uniforms(params))?;
        Ok(())
    }

    /// Привязки для набора дескрипторов.
    pub(crate) fn bindings(&self) -> FrameStyleBindings {
        FrameStyleBindings {
            tone_base: self.tone_base.binding(),
            look: self.look.binding(),
            look_lut: self.look_lut.binding(),
        }
    }
}
