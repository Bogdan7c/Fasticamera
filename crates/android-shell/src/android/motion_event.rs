//! Safety-модуль: копия Java `MotionEvent` через NDK (`AMotionEvent_fromJava`, API 31).
//!
//! `AMotionEvent_fromJava` создаёт нативную **копию** события, которую мы обязаны отпустить
//! (`AInputEvent_release`) — это делает [`InputEventCopy`] в `Drop`. Поля копируются в
//! нейтральный [`RawMotionEvent`] сразу, разбор смысла — в `touch_decode` (тестируется на ПК).

use std::ptr::NonNull;

use jni::Env;
use jni::objects::JObject;
use ndk_sys::{
    AInputEvent, AInputEvent_release, AMotionEvent_fromJava, AMotionEvent_getAction, AMotionEvent_getEventTime,
    AMotionEvent_getHistoricalEventTime, AMotionEvent_getHistoricalX, AMotionEvent_getHistoricalY,
    AMotionEvent_getHistorySize, AMotionEvent_getPointerCount, AMotionEvent_getPointerId, AMotionEvent_getX,
    AMotionEvent_getY,
};

use crate::touch_decode::{RawHistoricalFrame, RawMotionEvent, RawPointer};

/// Не удалось скопировать событие.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("AMotionEvent_fromJava вернул null: объект не MotionEvent или событие уже переработано")]
pub struct MotionEventCopyError;

/// Нативная копия события, которой мы владеем.
struct InputEventCopy(NonNull<AInputEvent>);

impl Drop for InputEventCopy {
    fn drop(&mut self) {
        // SAFETY: копия создана AMotionEvent_fromJava и отпускается ровно один раз.
        unsafe { AInputEvent_release(self.0.as_ptr()) };
    }
}

/// Копирует поля Java `MotionEvent` (живая локальная ссылка текущего нативного метода).
pub fn copy_motion_event(env: &Env<'_>, motion_event: &JObject<'_>) -> Result<RawMotionEvent, MotionEventCopyError> {
    // SAFETY: env — JNIEnv текущего потока внутри нативного метода, motion_event — живая
    // ссылка на android.view.MotionEvent из сигнатуры метода (её тип проверяет JNI).
    let raw = unsafe { AMotionEvent_fromJava(env.get_raw().cast(), motion_event.as_raw().cast()) };
    let copy = InputEventCopy(NonNull::new(raw.cast_mut()).ok_or(MotionEventCopyError)?);
    let event = copy.0.as_ptr().cast_const();
    // SAFETY (для всех вызовов ниже): event — действующая копия, живущая до конца функции
    // (copy отпустит её в Drop); индексы пальцев и истории берутся строго из диапазонов,
    // которые вернули getPointerCount и getHistorySize этой же копии.
    let pointer_count = unsafe { AMotionEvent_getPointerCount(event) };
    let history_size = unsafe { AMotionEvent_getHistorySize(event) };
    let pointers = (0..pointer_count)
        .map(|index| RawPointer {
            pointer_id: unsafe { AMotionEvent_getPointerId(event, index) },
            x_px: unsafe { AMotionEvent_getX(event, index) },
            y_px: unsafe { AMotionEvent_getY(event, index) },
        })
        .collect();
    let history = (0..history_size)
        .map(|history_index| RawHistoricalFrame {
            time_ns: unsafe { AMotionEvent_getHistoricalEventTime(event, history_index) },
            positions: (0..pointer_count)
                .map(|index| unsafe {
                    (
                        AMotionEvent_getHistoricalX(event, index, history_index),
                        AMotionEvent_getHistoricalY(event, index, history_index),
                    )
                })
                .collect(),
        })
        .collect();
    Ok(RawMotionEvent {
        action: unsafe { AMotionEvent_getAction(event) },
        // Наносекунды в шкале System.nanoTime (CLOCK_MONOTONIC), NDK input.h.
        event_time_ns: unsafe { AMotionEvent_getEventTime(event) },
        pointers,
        history,
    })
}
