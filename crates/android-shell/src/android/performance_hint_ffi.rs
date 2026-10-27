//! Safety-модуль: подсказки производительности ADPF (`android/performance_hint.h`).
//!
//! Уровни API: `APerformanceHint_getManager/createSession/updateTargetWorkDuration/closeSession` —
//! API 33; `AWorkDuration_*` и `APerformanceHint_reportActualWorkDuration2` (раздельные CPU/GPU) —
//! API 35. Наш `minSdk` — 36, поэтому символы есть всегда; объявления берём из `ndk-sys` 0.6 (он же
//! линкует `libandroid.so`). Устройство без поддержки ADPF отвечает `null` из `getManager` или
//! `createSession` — это честное `Unsupported`, рендер работает без подсказок.
//!
//! Потоки: сессия создаётся и используется одним потоком-владельцем (поток рендера). Вызовы —
//! односторонний binder к системному сервису (не блокируют). `AWorkDuration` создаётся один раз на
//! сессию и переиспользуется: выделение памяти на каждом кадре не нужно.

use std::ptr::NonNull;
use std::time::Duration;

use ndk_sys::{APerformanceHintSession, AWorkDuration};
use shell_contract::performance_hint::{
    FrameWork, FrameWorkSession, HintThreadId, PerformanceHintError, PerformanceHintService,
};

use crate::performance_hint_codes::{PlatformWorkDuration, duration_to_platform_nanos, platform_result};

/// Служба ADPF телефона. Состояния не держит: менеджер — системный синглтон, берётся при создании сессии.
#[derive(Debug, Default, Clone, Copy)]
pub struct AndroidPerformanceHints;

impl AndroidPerformanceHints {
    pub fn new() -> Self {
        Self
    }
}

impl PerformanceHintService for AndroidPerformanceHints {
    fn current_thread(&self) -> Result<HintThreadId, PerformanceHintError> {
        // SAFETY: gettid без аргументов, всегда успешен (Linux), возвращает tid вызывающего потока.
        let tid = unsafe { libc::gettid() };
        Ok(HintThreadId::from_raw(tid))
    }

    fn create_session(
        &self,
        threads: &[HintThreadId],
        target: Duration,
    ) -> Result<Box<dyn FrameWorkSession>, PerformanceHintError> {
        // SAFETY: функция без аргументов; null — ADPF на устройстве нет.
        let manager = unsafe { ndk_sys::APerformanceHint_getManager() };
        if manager.is_null() {
            return Err(PerformanceHintError::Unsupported {
                reason: "APerformanceHint_getManager вернул null".to_owned(),
            });
        }
        let tids: Vec<i32> = threads.iter().map(|thread| thread.as_raw()).collect();
        // SAFETY: менеджер не null; указатель и длина — живой срез `tids` на время вызова (NDK копирует).
        let session = unsafe {
            ndk_sys::APerformanceHint_createSession(
                manager,
                tids.as_ptr(),
                tids.len(),
                duration_to_platform_nanos(target),
            )
        };
        let Some(session) = NonNull::new(session) else {
            return Err(PerformanceHintError::Unsupported {
                reason: "APerformanceHint_createSession вернул null (ADPF не поддерживается или потоки чужие)"
                    .to_owned(),
            });
        };
        // SAFETY: функция без аргументов; null — нехватка памяти.
        let work = unsafe { ndk_sys::AWorkDuration_create() };
        let Some(work) = NonNull::new(work) else {
            // SAFETY: сессия только что создана и больше нигде не используется.
            unsafe { ndk_sys::APerformanceHint_closeSession(session.as_ptr()) };
            return Err(PerformanceHintError::Failed {
                operation: "AWorkDuration_create",
                code: 0,
            });
        };
        Ok(Box::new(AndroidHintSession { session, work }))
    }
}

/// Сессия ADPF и переиспользуемый `AWorkDuration`; оба освобождаются в `Drop` ровно один раз.
struct AndroidHintSession {
    session: NonNull<APerformanceHintSession>,
    work: NonNull<AWorkDuration>,
}

// SAFETY: объекты NDK не привязаны к создавшему потоку; доступ — только через `&mut self` владельца
// (один поток за раз), так что перенос между потоками безопасен.
unsafe impl Send for AndroidHintSession {}

impl FrameWorkSession for AndroidHintSession {
    fn update_target(&mut self, target: Duration) -> Result<(), PerformanceHintError> {
        // SAFETY: сессия жива до Drop.
        let code = unsafe {
            ndk_sys::APerformanceHint_updateTargetWorkDuration(
                self.session.as_ptr(),
                duration_to_platform_nanos(target),
            )
        };
        platform_result("APerformanceHint_updateTargetWorkDuration", code)
    }

    fn report_frame(&mut self, work: &FrameWork) -> Result<(), PerformanceHintError> {
        let numbers = PlatformWorkDuration::of(work);
        let duration = self.work.as_ptr();
        // SAFETY: `AWorkDuration` жив до Drop; сеттеры только пишут числа в объект. Правила чисел
        // (начало и общая > 0, CPU/GPU ≥ 0 и хотя бы одно > 0) проверены `FrameWork::new`.
        let code = unsafe {
            ndk_sys::AWorkDuration_setWorkPeriodStartTimestampNanos(duration, numbers.period_start_nanos);
            ndk_sys::AWorkDuration_setActualTotalDurationNanos(duration, numbers.total_nanos);
            ndk_sys::AWorkDuration_setActualCpuDurationNanos(duration, numbers.cpu_nanos);
            ndk_sys::AWorkDuration_setActualGpuDurationNanos(duration, numbers.gpu_nanos);
            ndk_sys::APerformanceHint_reportActualWorkDuration2(self.session.as_ptr(), duration)
        };
        platform_result("APerformanceHint_reportActualWorkDuration2", code)
    }
}

impl Drop for AndroidHintSession {
    fn drop(&mut self) {
        // SAFETY: оба объекта созданы в `create_session`, освобождаются ровно один раз здесь.
        unsafe {
            ndk_sys::AWorkDuration_release(self.work.as_ptr());
            ndk_sys::APerformanceHint_closeSession(self.session.as_ptr());
        }
    }
}
