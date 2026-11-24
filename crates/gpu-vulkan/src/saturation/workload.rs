//! Контракт «работа под замером»: единственное, что стенд знает о том, что он гоняет.

use ash::vk;

use crate::error::GpuError;

/// Номер слота стенда (0..`frames_in_flight`). Работа заводит свои ресурсы «на слот» по этому номеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotId(pub(crate) usize);

impl SlotId {
    /// Номер слота для индексации собственных массивов работы.
    pub fn index(self) -> usize {
        self.0
    }
}

/// Выданное работе место для записи одного кадра.
#[derive(Debug, Clone, Copy)]
pub struct FrameRecording {
    pub(crate) command_buffer: vk::CommandBuffer,
    pub(crate) slot: SlotId,
    pub(crate) frame_index: u64,
}

impl FrameRecording {
    /// Командный буфер в состоянии записи: работа записывает в него свой кадр, но НЕ завершает
    /// (`vkEndCommandBuffer` и отправку делает стенд).
    pub fn command_buffer(&self) -> vk::CommandBuffer {
        self.command_buffer
    }

    /// Слот, под который записывается кадр.
    pub fn slot(&self) -> SlotId {
        self.slot
    }

    /// Сквозной номер кадра в прогоне, с нуля (прогрев считается).
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }
}

/// Работа, пропускную способность которой меряет [`super::SaturationBench`].
///
/// Стенд не знает, что внутри (ISP, UI, снимок): зависимость только от этого контракта.
///
/// # Порядок вызовов (гарантии стенда)
/// * `record_frame(слот S)` вызывается только когда слот свободен: GPU закончил прошлый кадр этого слота и
///   работа уже получила для него [`Self::release_slot_after_fence`] (а для самого первого кадра слота
///   прошлого кадра нет);
/// * `release_slot_after_fence(S)` вызывается ровно один раз на каждый ОТПРАВЛЕННЫЙ кадр и только когда
///   fence кадра сигнален: можно читать результаты и тайминги слота и переиспользовать его ресурсы;
/// * ни один отправленный кадр не остаётся без `release_slot_after_fence`, пока стенд жив и GPU отвечает
///   (после ошибки или отмены стенд дожидается и освобождает все кадры в полёте).
pub trait SaturationWorkload {
    /// Записывает ОДИН кадр работы (свои барьеры, переходы раскладки, timestamp-точки — внутри).
    ///
    /// Ошибка останавливает прогон: кадр НЕ отправляется, стенд дожидается уже отправленных.
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError>;

    /// GPU закончил кадр слота (fence сигнален). Имя совпадает с принятым в проекте у `ColorScopes`/`UiRenderer`.
    fn release_slot_after_fence(&mut self, slot: SlotId);
}
