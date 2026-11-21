//! Порядок обмена ролей основной и тёплой камер (13b) — нейтральная часть: проверяется на ПК
//! с поддельными портами, настоящие порты (`android/backend/standby_api`) только исполняют шаги.
//!
//! # Порядок шагов (нельзя менять)
//! 1. **Проверки** без побочных эффектов: у тёплой есть идущий поток (сессия с запросом, эпоха известна), у основной
//!    тоже. Не прошли — [`StandbySwapFailure`], ничего не изменено.
//! 2. **Поток кадров** (`promote_standby`) — единственный шаг, который может отказать после проверок
//!    (хаб не знает эпохи). Он идёт первым именно поэтому: отказ не требует отката. Пока роли
//!    ещё не переключены, результаты прежней основной идут движку как раньше, а кадры нового
//!    показа поток уже берёт у новой эпохи — это согласовано внутри хаба под одним замком.
//! 3. **Роли результатов** (атомарные флаги в контекстах колбэков): бывшая тёплая начинает
//!    доставлять результаты движку, бывшая основная перестаёт.
//! 4. **Ресурсы** (устройства, сессии) меняются местами в бэкенде: бесшумно и без отказов,
//!    NDK-вызовов нет.

use camera_core::backend_standby::{StandbySlotId, StandbySwapFailure};
use raw_frame_stream::{StandbyPromotion, StreamEpoch};

/// Операции обмена. Реализация на Android — над настоящими ресурсами, в тестах — запись вызовов.
pub trait SwapPorts {
    /// Эпоха потока кадров у сессии тёплой ячейки; `None` — у ячейки нет идущего потока (нет сессии или запроса).
    fn standby_epoch(&self, slot: StandbySlotId) -> Option<StreamEpoch>;
    /// Идёт ли у основной камеры поток (сессия с повторяющимся запросом).
    fn primary_is_streaming(&self) -> bool;
    /// Обмен в потоке кадров (атомарно под замком хаба).
    fn promote_in_frame_stream(&mut self, standby: StreamEpoch) -> StandbyPromotion;
    /// Переключить роли результатов: тёплая ячейки → основная, основная → тёплая той же ячейки.
    fn flip_result_roles(&mut self, slot: StandbySlotId);
    /// Поменять местами ресурсы основной и ячейки.
    fn exchange_resources(&mut self, slot: StandbySlotId);
}

/// Выполняет обмен по порядку из доки модуля.
pub fn swap_with_primary(ports: &mut dyn SwapPorts, slot: StandbySlotId) -> Result<(), StandbySwapFailure> {
    let Some(standby_epoch) = ports.standby_epoch(slot) else {
        return Err(StandbySwapFailure::StandbyNotStreaming);
    };
    if !ports.primary_is_streaming() {
        return Err(StandbySwapFailure::PrimaryNotStreaming);
    }
    match ports.promote_in_frame_stream(standby_epoch) {
        StandbyPromotion::Promoted { .. } => {}
        // Хаб не знает такой тёплой эпохи (её закрыли или она уже показываемая): для движка
        // это «у тёплой нет идущего потока»; ничего не менялось, откат не нужен.
        StandbyPromotion::NoSuchStandby => return Err(StandbySwapFailure::StandbyNotStreaming),
    }
    ports.flip_result_roles(slot);
    ports.exchange_resources(slot);
    Ok(())
}

#[cfg(test)]
#[path = "standby_swap_tests.rs"]
mod tests;
