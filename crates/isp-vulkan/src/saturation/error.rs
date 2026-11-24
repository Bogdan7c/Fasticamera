//! Типизированная ошибка адаптеров стенда насыщения.
//!
//! Контракт стенда (`SaturationWorkload`) плоский: `record_frame` отдаёт только `GpuError`, а
//! `release_slot_after_fence` не возвращает ничего. Поэтому адаптеры держат ПЕРВУЮ типизированную
//! ошибку у себя (`take_failure`), а стенду отдают текст в `GpuError::InvalidState`.

use gpu_vulkan::error::GpuError;

use crate::photo::PhotoError;
use crate::quality::QualityError;
use crate::scopes::ScopeError;
use crate::viewfinder::ViewfinderError;

/// Почему адаптер работы не создан или не записал кадр.
#[derive(Debug, thiserror::Error)]
pub enum SaturationWorkError {
    /// Настройки адаптера неверны (нулевой размер, каденция 0, размер не кратен ячейке CFA...). GPU не трогали.
    #[error("неверные настройки работы стенда насыщения: {what}")]
    InvalidConfig {
        /// Что именно неверно.
        what: String,
    },
    /// Параметры обработки (`IspParams::resolve`) не посчитаны.
    #[error("параметры ISP не посчитаны: {what}")]
    Params {
        /// Текст ошибки `isp-model`.
        what: String,
    },
    /// Слот стенда вне числа слотов работы.
    #[error("слот стенда {slot} вне 0..{slots}: у работы столько слотов кадров")]
    SlotOutOfRange {
        /// Запрошенный слот.
        slot: usize,
        /// Сколько слотов у работы.
        slots: usize,
    },
    /// Работа записала «пустой» кадр (приёмник занят и т. п.): считать такой кадр пропускной способностью нельзя.
    #[error("кадр работы пуст: {what}")]
    EmptyFrame {
        /// Что именно не записано.
        what: String,
    },
    /// Ошибка ISP видоискателя.
    #[error("видоискатель: {0}")]
    Viewfinder(#[from] ViewfinderError),
    /// Ошибка цветовых приборов.
    #[error("приборы: {0}")]
    Scopes(#[from] ScopeError),
    /// Ошибка стенда V4.
    #[error("стенд V4: {0}")]
    Quality(#[from] QualityError),
    /// Ошибка проявления снимка.
    #[error("снимок: {0}")]
    Photo(#[from] PhotoError),
    /// Ошибка GPU.
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
    /// Дайджест выхода не прочитан (блокирующее чтение не удалось).
    #[error("дайджест выхода: {0}")]
    Digest(#[from] gpu_vulkan::saturation::DigestError),
    /// Дайджест этой работы в данной сборке получить нельзя (нужного доступа к выходу нет).
    #[error("дайджест недоступен: {what}")]
    DigestUnavailable {
        /// Что именно недоступно и что нужно, чтобы стало доступно.
        what: String,
    },
}

impl SaturationWorkError {
    /// Сокращение для `InvalidConfig`.
    pub(crate) fn invalid_config(what: impl Into<String>) -> Self {
        Self::InvalidConfig { what: what.into() }
    }

    /// Ошибка в виде, который принимает контракт стенда (`GpuError`).
    pub(crate) fn to_gpu_error(&self) -> GpuError {
        GpuError::InvalidState {
            what: format!("работа стенда насыщения: {self}"),
        }
    }
}

/// Хранит ПЕРВУЮ ошибку работы: стенд видит плоский `GpuError`, а тесты и сценарий читают типизированную.
#[derive(Debug, Default)]
pub(crate) struct FirstFailure {
    first: Option<SaturationWorkError>,
}

impl FirstFailure {
    /// Запоминает ошибку, если прежней нет. Возвращает ошибку в виде для стенда.
    pub(crate) fn note(&mut self, error: SaturationWorkError) -> GpuError {
        let for_bench = error.to_gpu_error();
        self.first.get_or_insert(error);
        for_bench
    }

    /// Забирает первую ошибку (после этого журнал пуст).
    pub(crate) fn take(&mut self) -> Option<SaturationWorkError> {
        self.first.take()
    }
}
