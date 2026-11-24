//! Связка нашего AE между камерой и рендером на уровне композиции (сессия 08a).
//!
//! Корень композиции создаёт [`AutoExposureLink`] один раз на процесс и раздаёт две стороны:
//! * **сторона камеры** остаётся внутри [`crate::camera_wiring`]: движок читает ящик предложений
//!   и пишет указания, а после запуска получает «толчок» рендера ([`EngineBinding`]);
//! * **сторона рендера** ([`AutoExposureRenderEnd`]) уходит туда, где строится поток рендера:
//!   рендер пишет предложения в ящик и читает указания движка.
//!
//! Ни камера, ни рендер не знают друг о друге: между ними только нейтральные ящики из
//! `camera-contract`. Какой исполнитель экспозиции стоит у камеры, решает [`ExposureControllerKind`]
//! (конфиг `camera.exposure_controller`): с вендорским AE рендеру отдавать нечего.

use std::sync::Arc;

use camera_contract::auto_exposure::{
    ExposureDirective, ExposureProposalChannel, LateBoundRefreshSignal, RefreshSignal,
};
use camera_contract::three_a::DirectiveSlot;
use serde::{Deserialize, Serialize};

/// Чья автоэкспозиция ведёт камеру. Переключатель для сравнения на телефоне (A/B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureControllerKind {
    /// Наш AE по статистике RAW: рендер меряет, движок строит ручной запрос (`AE_MODE_OFF`).
    #[default]
    Own,
    /// Вендорский AE через HAL (`AE_MODE_ON`): поведение сессии 08, рендер не участвует.
    Vendor,
}

/// Есть ли у нашего AE рендер, который мерит сцену. Корень композиции знает это после запуска рендера.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RenderAutoExposure {
    /// Рендер поднят (или его роль играет тест) и подключён к ящикам.
    #[default]
    Connected,
    /// Рендер не поднялся: предложений не будет никогда.
    Unavailable { reason: String },
}

/// Какой исполнитель экспозиции **реально** у камеры и почему. Выбор делает композиция: наш AE без
/// рендера навсегда застыл бы на стартовом значении, поэтому при недоступном рендере камера получает
/// вендорский AE, и это видно в журнале, сервисе камеры и отчёте сценария `3a`, а не молча.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExposureControllerChoice {
    /// Что просил конфиг.
    pub configured: ExposureControllerKind,
    /// Что поставлено у камеры.
    pub effective: ExposureControllerKind,
    /// Почему `effective` отличается от `configured` (`None` — не отличается).
    pub fallback_reason: Option<String>,
}

impl ExposureControllerChoice {
    pub fn resolve(configured: ExposureControllerKind, render: &RenderAutoExposure) -> Self {
        match (configured, render) {
            (ExposureControllerKind::Own, RenderAutoExposure::Unavailable { reason }) => Self {
                configured,
                effective: ExposureControllerKind::Vendor,
                fallback_reason: Some(format!("рендер недоступен, предложений нашему AE не будет: {reason}")),
            },
            _ => Self {
                configured,
                effective: configured,
                fallback_reason: None,
            },
        }
    }
}

impl Default for ExposureControllerChoice {
    fn default() -> Self {
        Self::resolve(ExposureControllerKind::default(), &RenderAutoExposure::Connected)
    }
}

/// Ящики нашего AE, общие для камеры и рендера. Клоны делят одно состояние.
#[derive(Debug, Clone, Default)]
pub struct AutoExposureLink {
    proposals: ExposureProposalChannel,
    directives: DirectiveSlot<ExposureDirective>,
}

/// Сторона рендера: куда писать предложения и откуда читать указания движка.
///
/// Передаётся туда, где строится поток рендера. Поля публичны намеренно: это пара ящиков для
/// передачи, а не объект с инвариантами (инварианты — у владельцев ящиков в `camera-contract`).
#[derive(Debug, Clone)]
pub struct AutoExposureRenderEnd {
    /// Сюда рендер кладёт предложения «сколько света нужно» и толкает движок.
    pub proposals: ExposureProposalChannel,
    /// Отсюда рендер читает, где мерить, блокировку и что реально стоит в запросе.
    pub directives: DirectiveSlot<ExposureDirective>,
}

impl AutoExposureLink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Сторона рендера; `None`, если у камеры вендорский AE (рендеру нечего измерять и некому
    /// отдавать предложения).
    pub fn render_end(&self, kind: ExposureControllerKind) -> Option<AutoExposureRenderEnd> {
        match kind {
            ExposureControllerKind::Own => Some(AutoExposureRenderEnd {
                proposals: self.proposals.clone(),
                directives: self.directives.clone(),
            }),
            ExposureControllerKind::Vendor => None,
        }
    }

    /// Ящик предложений для исполнителя экспозиции движка (сторона камеры).
    pub(crate) fn engine_proposals(&self) -> ExposureProposalChannel {
        self.proposals.clone()
    }

    /// Ящик указаний для движка (он единственный писатель).
    pub(crate) fn engine_directive_slot(&self) -> DirectiveSlot<ExposureDirective> {
        self.directives.clone()
    }

    /// Привязывает запущенный движок к толчку рендера. Пока привязка жива, предложения будят
    /// движок; когда её роняют (движок остановлен), толчки возвращаются в «ещё нет получателя».
    pub(crate) fn bind_engine(&self, receiver: Arc<dyn RefreshSignal>) -> EngineBinding {
        EngineBinding::bind(self.proposals.refresh_signal(), receiver)
    }
}

/// Живая привязка движка к толчку рендера; при падении отвязывает получателя.
///
/// Привязка одна на процесс (камера стартует один раз): если бы движков стало два, отвязка
/// старого сняла бы получателя нового — тогда владелец связки обязан сопоставлять поколения.
pub struct EngineBinding {
    signal: LateBoundRefreshSignal,
}

impl EngineBinding {
    /// Привязывает получателя к толчку (общий путь AE и белой).
    pub(crate) fn bind(signal: &LateBoundRefreshSignal, receiver: Arc<dyn RefreshSignal>) -> Self {
        let signal = signal.clone();
        signal.bind(receiver);
        Self { signal }
    }
}

impl Drop for EngineBinding {
    fn drop(&mut self) {
        self.signal.unbind();
    }
}

impl std::fmt::Debug for EngineBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EngineBinding")
    }
}

#[cfg(test)]
#[path = "auto_exposure_link_tests.rs"]
mod tests;
