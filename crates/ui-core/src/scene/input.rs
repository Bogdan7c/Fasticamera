//! Вход сцены: касания оболочки, дыры в потоке, единый поток [`UiInput`].

use shell_contract::touch::{TouchPhase, TouchSample};

use super::frame::FrameReport;
use super::{SceneError, UiScene};
use crate::gesture::{GestureError, GestureEvent, GestureState, PointerDisposition, RecognizerId, RecognizerSpec};
use crate::id::NodeId;
use crate::input::{PointerSample, UiInput};

/// Итог [`UiScene::handle_input`]: что произошло с входом.
#[derive(Debug, Clone, PartialEq)]
pub enum InputOutcome {
    Pointer(PointerDisposition),
    /// Незавершённые жесты сброшены как отменённые системой.
    StreamGapHandled,
    Frame(FrameReport),
}

impl<I: Clone> UiScene<I> {
    /// Касание оболочки в пикселях surface. Переводится в pt по текущему масштабу.
    pub fn handle_touch(&mut self, touch: &TouchSample) -> PointerDisposition {
        self.handle_pointer(PointerSample::from_touch(touch, self.scale))
    }

    /// Касание, уже переведённое в pt. На `Began` путь хит-теста берётся из дерева по его
    /// последней вёрстке (экран вызывает [`UiScene::layout`] после изменений структуры).
    pub fn handle_pointer(&mut self, sample: PointerSample) -> PointerDisposition {
        let hit_path = if sample.phase == TouchPhase::Began {
            self.tree.hit_path(sample.position)
        } else {
            // Арена использует путь только на `Began` первого пальца.
            Vec::new()
        };
        self.gestures.handle_pointer(sample, &hit_path)
    }

    /// Дыра в потоке касаний (`ShellEvent::TouchGap`): идущие жесты отменяются.
    pub fn handle_pointer_gap(&mut self) {
        self.gestures.handle_stream_gap();
    }

    /// Единый вход: касание в pt, дыра в потоке или кадр.
    pub fn handle_input(&mut self, input: UiInput) -> InputOutcome {
        match input {
            UiInput::Pointer(sample) => InputOutcome::Pointer(self.handle_pointer(sample)),
            UiInput::PointerStreamGap => {
                self.handle_pointer_gap();
                InputOutcome::StreamGapHandled
            }
            UiInput::Frame(frame_time) => InputOutcome::Frame(self.advance_frame(frame_time)),
        }
    }

    /// Добавляет распознаватель элементу. Мёртвый элемент — ошибка (иначе распознаватель висел бы
    /// на идентификаторе, который уже ничего не значит).
    pub fn add_recognizer(
        &mut self,
        node: NodeId,
        spec: RecognizerSpec,
        intent: I,
    ) -> Result<RecognizerId, SceneError> {
        if !self.tree.contains(node) {
            return Err(crate::tree::TreeError::UnknownNode(node).into());
        }
        Ok(self.gestures.add_recognizer(node, spec, intent)?)
    }

    /// `waiting` не сработает, пока `must_fail` не провалится.
    pub fn require_to_fail(&mut self, waiting: RecognizerId, must_fail: RecognizerId) -> Result<(), GestureError> {
        self.gestures.require_to_fail(waiting, must_fail)
    }

    /// Разрешает двум распознавателям идти одновременно.
    pub fn allow_simultaneous(&mut self, first: RecognizerId, second: RecognizerId) -> Result<(), GestureError> {
        self.gestures.allow_simultaneous(first, second)
    }

    pub fn recognizer_state(&self, id: RecognizerId) -> Result<GestureState, GestureError> {
        self.gestures.recognizer_state(id)
    }

    /// Забрать накопленные события жестов в порядке возникновения.
    pub fn drain_gesture_events(&mut self) -> Vec<GestureEvent<I>> {
        self.gestures.drain_events()
    }
}
