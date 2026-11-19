//! Источник кадра UI: кто строит список рисования к каждому показу.
//!
//! Поток рендера (`render-loop`) на каждом тике vsync спрашивает источник «что рисовать к моменту
//! показа» и рисует ответ поверх видоискателя. Источник живёт НА потоке рендера: анимации считаются
//! от времени показа этого же кадра, без пересылки списков между потоками и без отставания на кадр.
//! Кто источник — решает сборка (сценарий `glass` в 11, экран камеры в 12); рендер о содержимом не
//! знает (решение 86).

use shell_contract::clock::MonotonicNanos;

use crate::draw_list::{DrawList, DrawListError};
use crate::geometry::PxSize;

/// Что рендер сообщает источнику о кадре.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiFrameRequest {
    /// Ожидаемое время показа кадра (шкала vsync, `MONOTONIC`): к нему строятся анимации.
    pub present_time: MonotonicNanos,
    /// Логический размер экрана в пикселях (портрет, до поворота цели показа).
    pub logical_size: PxSize,
    /// Пикселей на pt (`ui_core::input::PointScale`): ширина surface / 440 pt (решение 82).
    pub px_per_pt: f32,
}

/// Почему источник не дал кадр. Рендер не рисует UI этого кадра и считает отказ (видоискатель
/// показывается как обычно): UI не может уронить показ камеры.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum UiFrameError {
    /// Список построен, но не прошёл проверку словаря.
    #[error("список рисования отклонён: {0}")]
    InvalidDrawList(#[from] DrawListError),
    /// Источник не смог построить кадр по своей причине.
    #[error("источник кадра UI: {reason}")]
    SourceFailed { reason: String },
}

/// Источник списка рисования для потока рендера.
pub trait UiFrameSource: Send {
    /// Короткое имя для диагностики и логов.
    fn name(&self) -> &'static str;

    /// Построить список рисования к моменту `request.present_time`.
    ///
    /// Вызывается один раз на показанный кадр, на потоке рендера; не должен блокироваться.
    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError>;
}
