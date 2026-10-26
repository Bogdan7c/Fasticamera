//! Разбор `MotionEvent` Android в нейтральные [`TouchSample`].
//!
//! Kotlin передаёт объект `MotionEvent` как есть (решение владельца, сессия 02), safety-модуль
//! `android::motion_event` копирует его поля в [`RawMotionEvent`], а смысл действий разбирается
//! здесь — без Android, поэтому тестируется на ПК.
//!
//! Семантика действий (`MotionEvent.ACTION_*`, значения сверены с `android.jar` SDK 36):
//! * `DOWN` — первый палец коснулся; `POINTER_DOWN` — ещё один палец (его индекс в старших битах);
//! * `MOVE` — сдвинулись пальцы; до текущих положений идут накопленные промежуточные
//!   («исторические») точки, от старых к новым;
//! * `POINTER_UP` — один из пальцев оторвался; `UP` — оторвался последний;
//! * `CANCEL` — система забрала жест: все пальцы отменены.
//!
//! Прочие действия (наведение, колесо, `OUTSIDE`) экрану камеры не нужны и пропускаются явно.

use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};

/// Коды действий `MotionEvent` (android.jar SDK 36).
mod action {
    pub const MASK: i32 = 0xff;
    pub const POINTER_INDEX_MASK: i32 = 0xff00;
    pub const POINTER_INDEX_SHIFT: u32 = 8;
    pub const DOWN: i32 = 0;
    pub const UP: i32 = 1;
    pub const MOVE: i32 = 2;
    pub const CANCEL: i32 = 3;
    pub const POINTER_DOWN: i32 = 5;
    pub const POINTER_UP: i32 = 6;
}

/// Положение одного пальца в текущем (последнем) наблюдении события.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPointer {
    pub pointer_id: i32,
    pub x_px: f32,
    pub y_px: f32,
}

/// Промежуточное наблюдение `MOVE`: время и положения в том же порядке, что [`RawMotionEvent::pointers`].
#[derive(Debug, Clone, PartialEq)]
pub struct RawHistoricalFrame {
    pub time_ns: i64,
    pub positions: Vec<(f32, f32)>,
}

/// Копия полей `MotionEvent`, нужных экрану камеры.
#[derive(Debug, Clone, PartialEq)]
pub struct RawMotionEvent {
    /// `getAction()`: код действия и индекс пальца в старших битах.
    pub action: i32,
    /// `getEventTime()` в наносекундах (`CLOCK_MONOTONIC`).
    pub event_time_ns: i64,
    pub pointers: Vec<RawPointer>,
    /// Промежуточные наблюдения от старых к новым (у `MOVE`; у остальных обычно пусто).
    pub history: Vec<RawHistoricalFrame>,
}

/// Итог разбора события.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedMotion {
    /// Касания в порядке времени.
    Samples(Vec<TouchSample>),
    /// Действие не относится к касаниям экрана (наведение, колесо и т.п.).
    Ignored { masked_action: i32 },
}

/// Событие противоречит само себе — это ошибка платформы или копирования, не жест.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TouchDecodeError {
    #[error("в событии касания нет ни одного пальца (действие {action})")]
    NoPointers { action: i32 },
    #[error("индекс пальца {index} вне диапазона: в событии {pointer_count} пальцев (действие {action})")]
    PointerIndexOutOfRange {
        action: i32,
        index: usize,
        pointer_count: usize,
    },
    #[error("промежуточная точка {frame} содержит {positions} положений вместо {pointer_count}")]
    HistoryShapeMismatch {
        frame: usize,
        positions: usize,
        pointer_count: usize,
    },
}

/// Разбирает одно событие в касания.
pub fn decode_motion_event(event: &RawMotionEvent) -> Result<DecodedMotion, TouchDecodeError> {
    let masked_action = event.action & action::MASK;
    let phase_for_index = match masked_action {
        action::DOWN => Some(TouchPhase::Began),
        action::UP => Some(TouchPhase::Ended),
        action::POINTER_DOWN => Some(TouchPhase::Began),
        action::POINTER_UP => Some(TouchPhase::Ended),
        action::MOVE | action::CANCEL => None,
        _ => return Ok(DecodedMotion::Ignored { masked_action }),
    };
    if event.pointers.is_empty() {
        return Err(TouchDecodeError::NoPointers { action: event.action });
    }
    let samples = match (masked_action, phase_for_index) {
        (action::MOVE, _) => decode_move(event)?,
        (action::CANCEL, _) => all_pointers(event, TouchPhase::Cancelled),
        (_, Some(phase)) => vec![single_pointer(event, action_pointer_index(event)?, phase)],
        // Сочетания выше покрывают все коды, для которых phase_for_index вернул значение.
        (_, None) => Vec::new(),
    };
    Ok(DecodedMotion::Samples(samples))
}

/// Индекс пальца, к которому относится действие (`DOWN`/`UP` — всегда 0).
fn action_pointer_index(event: &RawMotionEvent) -> Result<usize, TouchDecodeError> {
    let index = ((event.action & action::POINTER_INDEX_MASK) >> action::POINTER_INDEX_SHIFT) as usize;
    if index < event.pointers.len() {
        Ok(index)
    } else {
        Err(TouchDecodeError::PointerIndexOutOfRange {
            action: event.action,
            index,
            pointer_count: event.pointers.len(),
        })
    }
}

fn decode_move(event: &RawMotionEvent) -> Result<Vec<TouchSample>, TouchDecodeError> {
    let pointer_count = event.pointers.len();
    let mut samples = Vec::with_capacity(pointer_count * (event.history.len() + 1));
    for (frame_index, frame) in event.history.iter().enumerate() {
        if frame.positions.len() != pointer_count {
            return Err(TouchDecodeError::HistoryShapeMismatch {
                frame: frame_index,
                positions: frame.positions.len(),
                pointer_count,
            });
        }
        for (pointer, &(x_px, y_px)) in event.pointers.iter().zip(&frame.positions) {
            samples.push(TouchSample {
                pointer: PointerId::new(pointer.pointer_id),
                phase: TouchPhase::Moved,
                position: SurfacePoint { x_px, y_px },
                time: MonotonicNanos::from_nanos(frame.time_ns),
            });
        }
    }
    samples.extend(all_pointers(event, TouchPhase::Moved));
    Ok(samples)
}

fn all_pointers(event: &RawMotionEvent, phase: TouchPhase) -> Vec<TouchSample> {
    (0..event.pointers.len())
        .map(|index| single_pointer(event, index, phase))
        .collect()
}

/// Касание пальца `index`; вызывающий гарантирует, что индекс в диапазоне.
fn single_pointer(event: &RawMotionEvent, index: usize, phase: TouchPhase) -> TouchSample {
    let pointer = event.pointers[index];
    TouchSample {
        pointer: PointerId::new(pointer.pointer_id),
        phase,
        position: SurfacePoint {
            x_px: pointer.x_px,
            y_px: pointer.y_px,
        },
        time: MonotonicNanos::from_nanos(event.event_time_ns),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer(pointer_id: i32, x_px: f32, y_px: f32) -> RawPointer {
        RawPointer { pointer_id, x_px, y_px }
    }

    fn event(action: i32, time_ns: i64, pointers: Vec<RawPointer>) -> RawMotionEvent {
        RawMotionEvent {
            action,
            event_time_ns: time_ns,
            pointers,
            history: Vec::new(),
        }
    }

    /// Код действия с индексом пальца, как его кодирует Android.
    fn indexed(code: i32, index: i32) -> i32 {
        code | (index << action::POINTER_INDEX_SHIFT)
    }

    /// Запись ожидаемого касания: (id, фаза, x, y, время).
    type Expected = (i32, TouchPhase, f32, f32, i64);

    fn as_tuples(samples: &[TouchSample]) -> Vec<Expected> {
        samples
            .iter()
            .map(|sample| {
                (
                    sample.pointer.value(),
                    sample.phase,
                    sample.position.x_px,
                    sample.position.y_px,
                    sample.time.as_nanos(),
                )
            })
            .collect()
    }

    fn decode_all(events: &[RawMotionEvent]) -> Vec<Expected> {
        events
            .iter()
            .flat_map(|event| match decode_motion_event(event).expect("разбор") {
                DecodedMotion::Samples(samples) => as_tuples(&samples),
                DecodedMotion::Ignored { .. } => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn two_finger_gesture_decodes_to_exact_ordered_sequence() {
        use TouchPhase::*;
        let mut pinch_move = event(
            action::MOVE,
            40,
            vec![pointer(7, 110.0, 210.0), pointer(9, 290.0, 390.0)],
        );
        pinch_move.history = vec![
            RawHistoricalFrame {
                time_ns: 30,
                positions: vec![(105.0, 205.0), (295.0, 395.0)],
            },
            RawHistoricalFrame {
                time_ns: 35,
                positions: vec![(108.0, 208.0), (292.0, 392.0)],
            },
        ];
        let gesture = [
            event(action::DOWN, 10, vec![pointer(7, 100.0, 200.0)]),
            event(
                indexed(action::POINTER_DOWN, 1),
                20,
                vec![pointer(7, 100.0, 200.0), pointer(9, 300.0, 400.0)],
            ),
            pinch_move,
            event(
                indexed(action::POINTER_UP, 0),
                50,
                vec![pointer(7, 111.0, 211.0), pointer(9, 289.0, 389.0)],
            ),
            event(action::UP, 60, vec![pointer(9, 288.0, 388.0)]),
        ];
        let expected: Vec<Expected> = vec![
            (7, Began, 100.0, 200.0, 10),
            (9, Began, 300.0, 400.0, 20),
            (7, Moved, 105.0, 205.0, 30),
            (9, Moved, 295.0, 395.0, 30),
            (7, Moved, 108.0, 208.0, 35),
            (9, Moved, 292.0, 392.0, 35),
            (7, Moved, 110.0, 210.0, 40),
            (9, Moved, 290.0, 390.0, 40),
            (7, Ended, 111.0, 211.0, 50),
            (9, Ended, 288.0, 388.0, 60),
        ];
        assert_eq!(decode_all(&gesture), expected);
    }

    #[test]
    fn broken_event_order_changes_decoded_sequence() {
        // Защита от «тест проходит при любом порядке»: переставленные события дают другой поток.
        let down = event(action::DOWN, 10, vec![pointer(1, 0.0, 0.0)]);
        let up = event(action::UP, 20, vec![pointer(1, 5.0, 5.0)]);
        let in_order = decode_all(&[down.clone(), up.clone()]);
        let swapped = decode_all(&[up, down]);
        assert_ne!(in_order, swapped);
        assert_eq!(in_order[0].1, TouchPhase::Began);
    }

    #[test]
    fn cancel_cancels_every_pointer() {
        let cancel = event(action::CANCEL, 70, vec![pointer(1, 1.0, 1.0), pointer(2, 2.0, 2.0)]);
        let decoded = decode_all(&[cancel]);
        assert_eq!(
            decoded,
            vec![
                (1, TouchPhase::Cancelled, 1.0, 1.0, 70),
                (2, TouchPhase::Cancelled, 2.0, 2.0, 70)
            ]
        );
    }

    #[test]
    fn pointer_up_reports_only_lifted_finger() {
        let lift_second = event(
            indexed(action::POINTER_UP, 1),
            5,
            vec![pointer(3, 1.0, 1.0), pointer(4, 9.0, 9.0)],
        );
        assert_eq!(decode_all(&[lift_second]), vec![(4, TouchPhase::Ended, 9.0, 9.0, 5)]);
    }

    #[test]
    fn hover_and_other_actions_are_ignored_explicitly() {
        const ACTION_HOVER_MOVE: i32 = 7;
        let hover = event(ACTION_HOVER_MOVE, 1, vec![pointer(0, 1.0, 1.0)]);
        assert_eq!(
            decode_motion_event(&hover),
            Ok(DecodedMotion::Ignored {
                masked_action: ACTION_HOVER_MOVE
            })
        );
    }

    #[test]
    fn inconsistent_events_are_errors_not_guesses() {
        let no_pointers = event(action::DOWN, 1, Vec::new());
        assert_eq!(
            decode_motion_event(&no_pointers),
            Err(TouchDecodeError::NoPointers { action: action::DOWN })
        );
        let bad_index = event(indexed(action::POINTER_DOWN, 3), 1, vec![pointer(0, 1.0, 1.0)]);
        assert!(matches!(
            decode_motion_event(&bad_index),
            Err(TouchDecodeError::PointerIndexOutOfRange {
                index: 3,
                pointer_count: 1,
                ..
            })
        ));
        let mut bad_history = event(action::MOVE, 2, vec![pointer(0, 1.0, 1.0)]);
        bad_history.history.push(RawHistoricalFrame {
            time_ns: 1,
            positions: Vec::new(),
        });
        assert!(matches!(
            decode_motion_event(&bad_history),
            Err(TouchDecodeError::HistoryShapeMismatch { frame: 0, .. })
        ));
    }
}
