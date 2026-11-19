//! Запись о распознавателе внутри арены.

use super::super::kinds::Kind;
use super::super::spec::RecognizerSpec;
use super::super::state::{GestureData, GestureState, RecognizerId};
use crate::geometry::Point;
use crate::id::NodeId;

pub(super) struct Recognizer<I> {
    pub id: RecognizerId,
    pub node: NodeId,
    pub intent: I,
    pub spec: RecognizerSpec,
    pub kind: Kind,
    pub state: GestureState,
    /// Условия начала выполнены, но арена ещё не разрешила (ждёт чужого провала).
    pub ready: Option<GestureData>,
    /// Последние опубликованные данные: отдаются при `Cancelled`.
    pub last_data: Option<GestureData>,
    /// Положение пальца на `Began`: от него считается смещение `from_recognition`.
    pub recognition_origin: Option<Point>,
}

impl<I> Recognizer<I> {
    /// Переживает ли распознаватель полный отрыв пальцев: он в `Possible` и либо уже готов (ждёт
    /// провала другого), либо ждёт следующего тапа. У таких есть тикающий таймер или конечное
    /// условие, поэтому арена не залипает.
    pub fn survives_all_fingers_up(&self) -> bool {
        self.state == GestureState::Possible && (self.ready.is_some() || self.kind.awaits_more_input())
    }

    /// Молча вернуть в `Possible` (событие не публикуется).
    pub fn reset_to_possible(&mut self) {
        self.state = GestureState::Possible;
        self.ready = None;
        self.last_data = None;
        self.recognition_origin = None;
        self.kind.reset();
    }
}
