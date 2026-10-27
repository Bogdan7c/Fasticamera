//! Поток опроса батареи `fasticamera-power`.
//!
//! Почему отдельный поток, а не `fasticamera-platform`: поток команд обслуживает хаптику и
//! окно из ограниченной очереди и спит в `recv`; периодическое чтение там задерживало бы
//! вибрацию на длительность binder-вызова и требовало бы переделать его цикл под таймер.
//! Этот поток живёт только пока жива подписка [`PowerSampling`], на всё время жизни подключён
//! к JVM (`attach_current_thread` вокруг всего цикла) и делает ровно одно — читает снимок
//! `ShellPlatform.readBatterySnapshot()` по сетке периода и кладёт показание в кольцо.
//!
//! Никто не ждёт этот поток: уничтожение подписки только шлёт сигнал остановки и отсоединяет
//! поток (`crate::detached_worker`). Чтение binder в этот момент может длиться сотни миллисекунд —
//! маршрутизатор, который уничтожает подписку, ждать его не должен. Флаг «опрос занят» поток
//! освобождает сам, когда действительно вышел: быстрый перезапуск до этого честно получает
//! `AlreadySampling`, а не второй поток рядом с первым.
//! Сбой чтения не останавливает опрос: он учитывается в кольце по виду и пишется в лог при
//! смене вида сбоя (а не на каждом тике, чтобы не забить буфер logcat).
//!
//! [`PowerSampling`]: shell_contract::power::PowerSampling

use std::sync::Arc;
use std::time::{Duration, Instant};

use jni::objects::{JLongArray, JObject};
use jni::{Env, JavaVM, jni_sig, jni_str};
use shell_contract::power::{CurrentNowUnit, PowerReadFailure, PowerSampleRing};

use super::platform_calls::describe_failure;
use super::runtime::ShellRuntime;
use crate::battery_snapshot::{BatterySnapshotError, decode_battery_snapshot, slot};
use crate::detached_worker::{ClaimRelease, StopSignal, StopWait, StopWaiter, spawn_detached_worker};
use crate::power_pacing::next_read;

/// Ёмкость локального кадра JNI на одно чтение (возвращённый массив + исключение).
const LOCAL_FRAME_CAPACITY: usize = 4;
const THREAD_NAME: &str = "fasticamera-power";

/// Запущенный поток опроса. Уничтожение только просит поток остановиться и не ждёт его
/// (см. модуль); флаг занятости поток освободит сам при выходе.
pub struct PowerSamplerThread {
    _stop: StopSignal,
}

/// Запускает поток опроса. Сама функция не делает вызовов платформы: только создаёт поток.
/// `claim` — освобождение флага «опрос занят»: оно уходит в поток и срабатывает при его выходе
/// (или сразу, если поток не создан).
pub fn start_power_sampler(
    vm: JavaVM,
    runtime: Arc<ShellRuntime>,
    ring: Arc<PowerSampleRing>,
    period: Duration,
    current_now_unit: CurrentNowUnit,
    claim: ClaimRelease<impl FnOnce() + Send + 'static>,
) -> std::io::Result<PowerSamplerThread> {
    let stop = spawn_detached_worker(THREAD_NAME, claim, move |stop_waiter| {
        let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            run_sampling_loop(env, &runtime, &ring, period, current_now_unit, stop_waiter);
            Ok(())
        });
        if let Err(error) = outcome {
            tracing::error!(target: "fasticamera::power", %error, "поток опроса батареи не подключился к JVM");
            ring.record_failed_read(PowerReadFailure::PlatformCallFailed {
                reason: format!("поток не подключился к JVM: {error}"),
            });
        }
        tracing::debug!(target: "fasticamera::power", "поток опроса батареи вышел");
    })?;
    Ok(PowerSamplerThread { _stop: stop })
}

/// Цикл: чтение → ожидание следующего узла сетки или сигнала остановки.
fn run_sampling_loop(
    env: &mut Env<'_>,
    runtime: &ShellRuntime,
    ring: &PowerSampleRing,
    period: Duration,
    current_now_unit: CurrentNowUnit,
    stop_waiter: &StopWaiter,
) {
    let mut scheduled = Instant::now();
    let mut failure_log = FailureLog::default();
    loop {
        let outcome = read_one_sample(env, runtime, current_now_unit);
        match outcome {
            Ok(sample) => {
                ring.push(sample);
                failure_log.recovered();
            }
            Err(failure) => {
                failure_log.failed(&failure);
                ring.record_failed_read(failure);
            }
        }
        let next = next_read(scheduled, period, Instant::now());
        if next.skipped_ticks > 0 {
            ring.record_missed_ticks(next.skipped_ticks);
        }
        scheduled = next.at;
        let wait = scheduled.saturating_duration_since(Instant::now());
        match stop_waiter.wait(wait) {
            StopWait::TimedOut => {}
            // Подписка уничтожена — выходим.
            StopWait::StopRequested => break,
        }
    }
}

/// Одно чтение: снимок у Kotlin + разбор. Binder-вызовы идут без замков оболочки
/// (`attached_platform` отдаёт клон `Arc`).
fn read_one_sample(
    env: &mut Env<'_>,
    runtime: &ShellRuntime,
    current_now_unit: CurrentNowUnit,
) -> Result<shell_contract::power::BatterySample, PowerReadFailure> {
    let Some(platform) = runtime.attached_platform() else {
        return Err(PowerReadFailure::NotAttached);
    };
    let read = env.with_local_frame(LOCAL_FRAME_CAPACITY, |env| -> jni::errors::Result<SnapshotRead> {
        read_snapshot_slots(env, platform.object())
    });
    match read {
        Ok(SnapshotRead::Slots(slots)) => {
            decode_battery_snapshot(&slots, current_now_unit).map_err(|error| PowerReadFailure::MalformedSnapshot {
                reason: error.to_string(),
            })
        }
        Ok(SnapshotRead::Malformed(error)) => Err(PowerReadFailure::MalformedSnapshot {
            reason: error.to_string(),
        }),
        Err(error) => Err(PowerReadFailure::PlatformCallFailed {
            reason: describe_failure(env, &error),
        }),
    }
}

/// Итог JNI-части чтения: массив нужной длины или ошибка раскладки (не ошибка JNI).
enum SnapshotRead {
    Slots([i64; slot::COUNT]),
    Malformed(BatterySnapshotError),
}

fn read_snapshot_slots(env: &mut Env<'_>, platform: &JObject<'_>) -> jni::errors::Result<SnapshotRead> {
    let returned = env
        .call_method(platform, jni_str!("readBatterySnapshot"), jni_sig!("()[J"), &[])?
        .l()?;
    if returned.is_null() {
        // Kotlin-метод объявлен с ненулевым LongArray; null — нарушение раскладки, а не JNI.
        return Ok(SnapshotRead::Malformed(BatterySnapshotError::wrong_slot_count(0)));
    }
    let array = env.cast_local::<JLongArray>(returned)?;
    let length = array.len(env)?;
    if length != slot::COUNT {
        return Ok(SnapshotRead::Malformed(BatterySnapshotError::wrong_slot_count(length)));
    }
    let mut slots = [0_i64; slot::COUNT];
    array.get_region(env, 0, &mut slots)?;
    Ok(SnapshotRead::Slots(slots))
}

/// Лог сбоев без потопа: пишет при первом сбое нового вида и при восстановлении.
#[derive(Default)]
struct FailureLog {
    current: Option<std::mem::Discriminant<PowerReadFailure>>,
}

impl FailureLog {
    fn failed(&mut self, failure: &PowerReadFailure) {
        let kind = std::mem::discriminant(failure);
        if self.current != Some(kind) {
            tracing::warn!(target: "fasticamera::power", ?failure, "чтение батареи не удалось (повторы того же вида — только в счётчиках)");
            self.current = Some(kind);
        }
    }

    fn recovered(&mut self) {
        if self.current.take().is_some() {
            tracing::info!(target: "fasticamera::power", "чтение батареи восстановилось");
        }
    }
}
