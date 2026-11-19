//! Помощники тестов жестов: стенд с ареной и сценариями касаний.

use shell_contract::touch::{PointerId, TouchPhase};

use super::*;
use crate::geometry::Point;
use crate::id::NodeId;
use crate::input::PointerSample;
use crate::time::MonotonicNanos;

/// Элемент с номером места `slot`.
pub(super) fn node(slot: u32) -> NodeId {
    NodeId::new(slot, 0)
}

/// Момент в миллисекундах.
pub(super) fn at(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(millis * 1_000_000)
}

/// Стенд: арена с намерениями-строками и короткие методы для потоков касаний.
pub(super) struct Rig {
    pub arena: GestureArena<&'static str>,
}

impl Rig {
    pub fn new() -> Self {
        Self {
            arena: GestureArena::new(GestureConfig::default()).expect("конфиг по умолчанию верен"),
        }
    }

    pub fn add(&mut self, element: NodeId, spec: RecognizerSpec, intent: &'static str) -> RecognizerId {
        self.arena
            .add_recognizer(element, spec, intent)
            .expect("спецификация верна")
    }

    fn sample(pointer: i32, phase: TouchPhase, x: f64, y: f64, millis: i64) -> PointerSample {
        PointerSample {
            pointer: PointerId::new(pointer),
            phase,
            position: Point::new(x, y),
            time: at(millis),
        }
    }

    pub fn touch(
        &mut self,
        pointer: i32,
        phase: TouchPhase,
        x: f64,
        y: f64,
        millis: i64,
        path: &[NodeId],
    ) -> PointerDisposition {
        self.arena
            .handle_pointer(Self::sample(pointer, phase, x, y, millis), path)
    }

    pub fn down(&mut self, pointer: i32, x: f64, y: f64, millis: i64, path: &[NodeId]) {
        assert_eq!(
            self.touch(pointer, TouchPhase::Began, x, y, millis, path),
            PointerDisposition::Applied
        );
    }

    pub fn moved(&mut self, pointer: i32, x: f64, y: f64, millis: i64) {
        assert_eq!(
            self.touch(pointer, TouchPhase::Moved, x, y, millis, &[]),
            PointerDisposition::Applied
        );
    }

    pub fn up(&mut self, pointer: i32, x: f64, y: f64, millis: i64) {
        assert_eq!(
            self.touch(pointer, TouchPhase::Ended, x, y, millis, &[]),
            PointerDisposition::Applied
        );
    }

    pub fn system_cancel(&mut self, pointer: i32, x: f64, y: f64, millis: i64) {
        assert_eq!(
            self.touch(pointer, TouchPhase::Cancelled, x, y, millis, &[]),
            PointerDisposition::Applied
        );
    }

    pub fn frame(&mut self, millis: i64) {
        self.arena.advance_time(at(millis));
    }

    /// События в виде «намерение:состояние».
    pub fn trace(&mut self) -> Vec<String> {
        self.arena
            .drain_events()
            .iter()
            .map(|event| format!("{}:{:?}", event.intent, event.state))
            .collect()
    }

    /// Все события с данными.
    pub fn events(&mut self) -> Vec<GestureEvent<&'static str>> {
        self.arena.drain_events()
    }

    pub fn state(&self, id: RecognizerId) -> GestureState {
        self.arena.recognizer_state(id).expect("распознаватель известен")
    }
}

pub(super) fn tap() -> RecognizerSpec {
    RecognizerSpec::Tap(TapSpec::default())
}

pub(super) fn double_tap() -> RecognizerSpec {
    RecognizerSpec::Tap(TapSpec {
        taps: TapCount::Double,
        ..TapSpec::default()
    })
}

pub(super) fn long_press() -> RecognizerSpec {
    RecognizerSpec::LongPress(LongPressSpec::default())
}

pub(super) fn pan(direction: PanDirection) -> RecognizerSpec {
    RecognizerSpec::Pan(PanSpec {
        direction,
        ..PanSpec::default()
    })
}

pub(super) fn pinch() -> RecognizerSpec {
    RecognizerSpec::Pinch(PinchSpec::default())
}

pub(super) fn press() -> RecognizerSpec {
    RecognizerSpec::Press(PressSpec::default())
}

pub(super) fn swipe(directions: &[SwipeDirection]) -> RecognizerSpec {
    RecognizerSpec::Swipe(SwipeSpec {
        directions: directions.to_vec(),
        ..SwipeSpec::default()
    })
}

/// Строка трассы для побитового сравнения: числа данных — через `to_bits`.
pub(super) fn bitwise_trace(events: &[GestureEvent<&'static str>]) -> Vec<(u32, String, i64, Vec<u64>)> {
    events
        .iter()
        .map(|event| {
            let bits = |value: f64| value.to_bits();
            let numbers = match event.data {
                GestureData::None => vec![],
                GestureData::Tap { position, tap_count } => {
                    vec![bits(position.x), bits(position.y), u64::from(tap_count)]
                }
                GestureData::LongPress { position, translation } => vec![
                    bits(position.x),
                    bits(position.y),
                    bits(translation.from_touch_down.dx),
                    bits(translation.from_touch_down.dy),
                    bits(translation.from_recognition.dx),
                    bits(translation.from_recognition.dy),
                ],
                GestureData::Pan {
                    position,
                    translation,
                    velocity,
                } => vec![
                    bits(position.x),
                    bits(position.y),
                    bits(translation.from_touch_down.dx),
                    bits(translation.from_touch_down.dy),
                    bits(translation.from_recognition.dx),
                    bits(translation.from_recognition.dy),
                    bits(velocity.dx),
                    bits(velocity.dy),
                ],
                GestureData::Pinch {
                    scale,
                    scale_velocity,
                    focus,
                } => {
                    vec![bits(scale), bits(scale_velocity), bits(focus.x), bits(focus.y)]
                }
                GestureData::Swipe {
                    direction,
                    position,
                    velocity,
                } => {
                    vec![
                        direction as u64,
                        bits(position.x),
                        bits(position.y),
                        bits(velocity.dx),
                        bits(velocity.dy),
                    ]
                }
                GestureData::Press { position } => vec![bits(position.x), bits(position.y)],
            };
            (
                event.recognizer.ordinal(),
                format!("{:?}", event.state),
                event.time.as_nanos(),
                numbers,
            )
        })
        .collect()
}
