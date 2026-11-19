//! Кадр сцены: часы → таймеры жестов → аниматор.

use super::UiScene;
use crate::animator::AdvanceReport;
use crate::time::{ClockAdvance, MonotonicNanos};

/// Итог [`UiScene::advance_frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReport {
    /// Как часы приняли время кадра (вперёд, повтор, назад).
    pub clock: ClockAdvance,
    /// Что сделал аниматор.
    pub animations: AdvanceReport,
}

impl<I: Clone> UiScene<I> {
    /// Первая половина кадра: часы и таймеры жестов. События таймеров (долгий тап) появятся в
    /// [`UiScene::drain_gesture_events`]. Время «назад» часы игнорируют и считают.
    pub fn begin_frame(&mut self, frame_time: MonotonicNanos) -> ClockAdvance {
        let advance = self.clock.advance_to(frame_time);
        // Арена ведёт свои часы и сама игнорирует повтор и ход назад; её ответ дублирует
        // ответ сцены, поэтому здесь он не нужен.
        let _arena_view = self.gestures.advance_time(frame_time);
        advance
    }

    /// Вторая половина кадра: значения анимаций записываются в дерево на момент последнего кадра.
    /// До первого кадра времени нет — пустой отчёт (анимации ждут первого кадра).
    pub fn apply_animations(&mut self) -> AdvanceReport {
        match self.clock.now() {
            Some(now) => self.animator.advance(now, &mut self.tree),
            None => AdvanceReport::default(),
        }
    }

    /// Целый кадр: [`UiScene::begin_frame`] и [`UiScene::apply_animations`] подряд.
    pub fn advance_frame(&mut self, frame_time: MonotonicNanos) -> FrameReport {
        let clock = self.begin_frame(frame_time);
        let animations = self.apply_animations();
        FrameReport { clock, animations }
    }
}
