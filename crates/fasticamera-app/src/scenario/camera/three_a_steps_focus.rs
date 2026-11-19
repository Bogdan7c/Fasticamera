//! Шаги `3a` про фокус и AWB: a (живая автоматика), b (AWB), c (тапы), f (блокировка AE/AF).

use camera_contract::control::ManualControl;
use camera_contract::three_a::{AeAfLock, AeState, PointRequestId, SensorPoint};
use camera_core::CameraEvent;
use camera_core::three_a_status::FocusProgress;
use render_loop::viewfinder::ColdStartState;
use shell_contract::haptics::{HapticOutcome, HapticPattern};

use super::engine_watch::RunFailure;
use super::progress::Deadline;
use super::three_a_own_ae::{AeSource, ObservedSceneRange, is_lock_reached};
use super::three_a_report::{
    AwbReport, AwbVsHal, FocusReport, LiveReport, LockReport, TapReport, TapResult, ThreeAObservation,
};
use super::three_a_results::{FrozenCriteria, exposure_product, product_frozen_after_frames, relative_gap};
use super::three_a_steps::{StepEnv, StepProgress, ThreeAStep, invalid_value, sent};
use super::white_balance_report::WhiteBalanceReport;
use crate::three_a_scenario_config::TapPointConfig;

/// Шаг a: вспомогательный поток в сессии и первые состояния AF/AE в результатах (решение 62 в продукте).
#[derive(Default)]
pub struct LiveStep {
    deadline: Option<Deadline>,
}

impl ThreeAStep for LiveStep {
    fn name(&self) -> &'static str {
        "live"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        self.deadline = Some(Deadline::after_millis(env.now, env.config.live_results_timeout_ms));
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        let af_state = env.results.repeating_after(None).find_map(|result| result.af_state);
        let ae_state = env.results.repeating_after(None).find_map(|result| result.ae_state);
        let expired = self.deadline.is_some_and(|deadline| deadline.is_expired(env.now));
        // У нашего AE вендорский автомат выключен: ждать `AE_STATE` не нужно, экспозицию судит шаг e.
        let ae_source = AeSource::of(&env.status.three_a);
        let ae_ready = ae_source == AeSource::Own || ae_state.is_some();
        if !(af_state.is_some() && ae_ready || expired) {
            return StepProgress::Pending;
        }
        let companion = env.running.diagnostics_snapshot().three_a_companion;
        env.stage(
            "three_a_live",
            [
                ("companion", super::detail::to_value(&companion)),
                ("af_state", super::detail::to_value(&af_state)),
                ("ae_state", super::detail::to_value(&ae_state)),
            ],
        );
        observation.live = Some(LiveReport {
            ae_source,
            companion,
            af_state,
            ae_state,
        });
        StepProgress::Done
    }
}

/// Шаг b: первая оценка света и конец холодного старта. На стадии `awb_ready` скрипт может снять скриншот.
#[derive(Default)]
pub struct AwbStep {
    deadline: Option<Deadline>,
}

impl ThreeAStep for AwbStep {
    fn name(&self) -> &'static str {
        "awb"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        self.deadline = Some(Deadline::after_millis(env.now, env.config.awb.ready_timeout_ms));
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        let diagnostics = env
            .presentation
            .viewfinder
            .as_ref()
            .map(|viewfinder| &viewfinder.white_balance);
        let ready = diagnostics.is_some_and(|wb| wb.estimates > 0 && matches!(wb.cold_start, ColdStartState::Warm));
        let expired = self.deadline.is_some_and(|deadline| deadline.is_expired(env.now));
        if !(ready || expired) {
            return StepProgress::Pending;
        }
        let report = diagnostics.map(WhiteBalanceReport::of);
        let hidden = diagnostics.map_or(0, |wb| wb.hidden_frames_cold_start);
        let forced = diagnostics.map_or(0, |wb| wb.cold_start_forced);
        let awb = AwbReport {
            ready,
            hidden_since_start: hidden.saturating_sub(env.baselines.hidden_frames),
            forced_since_start: forced.saturating_sub(env.baselines.forced_shows),
            max_hidden_frames: env.baselines.max_hidden_frames,
            awb_vs_hal: awb_vs_hal(report.as_ref(), env.config.awb.warn_mired_vs_hal),
            white_balance: report,
        };
        env.stage(
            "awb_ready",
            [
                ("ready", awb.ready.into()),
                ("hidden_since_start", awb.hidden_since_start.into()),
                ("forced_since_start", awb.forced_since_start.into()),
                ("awb_vs_hal", super::detail::to_value(&awb.awb_vs_hal)),
            ],
        );
        observation.awb = Some(awb);
        StepProgress::Done
    }
}

/// Наша белая против белой HAL: справка по порогу.
pub fn awb_vs_hal(report: Option<&WhiteBalanceReport>, warn_mired: f64) -> AwbVsHal {
    match report.and_then(|report| report.hal_delta_mired) {
        None => AwbVsHal::Unknown,
        Some(delta) if delta.abs() > warn_mired => AwbVsHal::Warn,
        Some(_) => AwbVsHal::Ok,
    }
}

/// Тап, ожидающий итога.
struct ActiveTap {
    point: TapPointConfig,
    request: PointRequestId,
    frame_at_send: Option<i64>,
    diopters_before: Option<f32>,
    deadline: Deadline,
}

/// Шаг c: тап в центр и в точки у углов; на каждый — итог, кадры до сходимости, дистанция до и после.
#[derive(Default)]
pub struct TapStep {
    next_index: usize,
    active: Option<ActiveTap>,
    taps: Vec<TapReport>,
    triggers_at_start: u64,
    unknown_origin_at_start: u64,
    no_context_at_start: u64,
}

impl TapStep {
    fn start_tap(&mut self, env: &mut StepEnv<'_>, point: TapPointConfig) -> Result<(), RunFailure> {
        let sensor_point =
            SensorPoint::new(point.x, point.y).map_err(|error| invalid_value("focus_and_expose_at", error))?;
        let command = sent(
            "focus_and_expose_at",
            env.running.handle().focus_and_expose_at(sensor_point),
        )?;
        self.active = Some(ActiveTap {
            point,
            // Номер тапа движок берёт из номера команды (см. тесты camera-core).
            request: PointRequestId(command.0),
            frame_at_send: env.results.latest_frame(),
            diopters_before: env
                .results
                .latest_repeating()
                .and_then(|result| result.focus_distance_diopters),
            deadline: Deadline::after_millis(env.now, env.config.tap.settle_timeout_ms),
        });
        Ok(())
    }

    fn settle_of(active: &ActiveTap, env: &StepEnv<'_>) -> Option<TapResult> {
        let from_event = env.events.iter().find_map(|event| match event {
            CameraEvent::FocusSettled { request, outcome } if *request == active.request => {
                Some(TapResult::of(*outcome))
            }
            _ => None,
        });
        from_event.or_else(|| active.deadline.is_expired(env.now).then_some(TapResult::NoEvent))
    }

    fn finish_tap(&mut self, env: &mut StepEnv<'_>, result: TapResult) {
        let Some(active) = self.active.take() else {
            return;
        };
        let frames_to_settle = match (env.results.latest_frame(), active.frame_at_send) {
            (Some(now), Some(sent)) => Some(now - sent),
            _ => None,
        };
        let report = TapReport {
            x: active.point.x,
            y: active.point.y,
            result,
            frames_to_settle,
            diopters_before: active.diopters_before,
            diopters_after: env
                .results
                .latest_repeating()
                .and_then(|result| result.focus_distance_diopters),
        };
        env.stage(
            "three_a_tap",
            [
                ("x", report.x.into()),
                ("y", report.y.into()),
                ("result", super::detail::to_value(&report.result)),
                ("frames_to_settle", report.frames_to_settle.into()),
                ("diopters_after", report.diopters_after.into()),
            ],
        );
        self.taps.push(report);
    }

    fn report(&self, env: &StepEnv<'_>) -> FocusReport {
        let distances: Vec<f32> = self.taps.iter().filter_map(|tap| tap.diopters_after).collect();
        let region_effect_diopters = distances
            .iter()
            .copied()
            .reduce(f32::max)
            .zip(distances.iter().copied().reduce(f32::min))
            .map(|(highest, lowest)| ((highest - lowest) * 100.0).round() / 100.0);
        let counters = env.status.three_a.counters;
        FocusReport {
            taps: self.taps.clone(),
            region_effect_diopters,
            triggers_sent: counters.focus_triggers_sent.saturating_sub(self.triggers_at_start),
            results_without_request_context: env
                .running
                .diagnostics_snapshot()
                .results_without_request_context
                .saturating_sub(self.no_context_at_start),
            results_with_unknown_origin: counters
                .results_with_unknown_origin
                .saturating_sub(self.unknown_origin_at_start),
        }
    }
}

impl ThreeAStep for TapStep {
    fn name(&self) -> &'static str {
        "tap"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        let counters = env.status.three_a.counters;
        self.triggers_at_start = counters.focus_triggers_sent;
        self.unknown_origin_at_start = counters.results_with_unknown_origin;
        self.no_context_at_start = env.running.diagnostics_snapshot().results_without_request_context;
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        if let Some(active) = &self.active {
            if let Some(result) = Self::settle_of(active, env) {
                self.finish_tap(env, result);
            }
            return StepProgress::Pending;
        }
        let Some(point) = env.config.tap.points.get(self.next_index).copied() else {
            observation.focus = Some(self.report(env));
            return StepProgress::Done;
        };
        self.next_index += 1;
        match self.start_tap(env, point) {
            Ok(()) => StepProgress::Pending,
            Err(failure) => StepProgress::Failed(failure),
        }
    }
}

/// Фазы блокировки.
enum LockPhase {
    WaitLocked {
        marker: Option<i64>,
        deadline: Deadline,
    },
    Hold {
        first_locked_frame: i64,
        /// Номер кадра в момент команды блокировки: от него считаем «кадров до замирания».
        lock_marker: Option<i64>,
        until: std::time::Instant,
    },
    WaitUnlocked {
        deadline: Deadline,
    },
}

/// Тактильный сигнал в начале удержания: «блокировка взята, закрывайте объектив рукой».
const HOLD_START_CUE: HapticPattern = HapticPattern::HeavyImpact;
/// Тактильный сигнал в конце удержания: «окно кончилось, объектив можно открывать». Другой паттерн, чтобы
/// сигналы не путались.
const HOLD_END_CUE: HapticPattern = HapticPattern::Success;

/// Шаг f: блокировка AE/AF по центру, удержание под наблюдением (`lock.hold_ms`), сброс точки.
///
/// Удержание размечено тактильными сигналами (человек закрывает объектив по первому и открывает по второму),
/// а доказательство, что сцена менялась, — размах сырого замера сцены нашим AE ([`ObservedSceneRange`]).
#[derive(Default)]
pub struct LockStep {
    phase: Option<LockPhase>,
    report: Option<LockReport>,
    /// Наш AE: тиков под блокировкой, где статус вышел из `Holding`.
    left_holding_ticks: usize,
    /// Замеры сцены на тиках удержания.
    scene_range: ObservedSceneRange,
    /// Итог сигнала в начале удержания.
    haptic_hold_start: Option<HapticOutcome>,
}

/// Что собрано за удержание из журнала результатов.
struct HoldMeasurements {
    samples_held: usize,
    non_locked_after_lock: usize,
    max_product_deviation: Option<f64>,
    product_frozen_after_frames: Option<i64>,
}

impl LockStep {
    /// Блокировка достигнута: вендорский AE — `AE_STATE = Locked` в результате, наш — `Holding` (или честный
    /// предел) в статусе движка. Возвращает номер кадра, с которого судим постоянство.
    fn locked_frame(env: &StepEnv<'_>, marker: Option<i64>) -> Option<i64> {
        match AeSource::of(&env.status.three_a) {
            AeSource::Vendor => env
                .results
                .repeating_after(marker)
                .find(|result| result.ae_state == Some(AeState::Locked))
                .map(|result| result.frame_number.0),
            // Запрос с замороженной целью доходит до сенсора с задержкой в несколько кадров:
            // постоянство судим не раньше, чем через `own_ae_catch_up_frames`.
            AeSource::Own => is_lock_reached(&env.status.three_a).then(|| {
                env.results.latest_frame().unwrap_or(0) + i64::from(env.config.lock.own_ae_catch_up_frames) + 1
            }),
        }
    }

    /// Разбор журнала результатов за удержание: сколько кадров, сколько не под блокировкой, отклонение
    /// произведения и сколько кадров оно доходило до замирания.
    fn measure_hold(
        first_locked_frame: i64,
        lock_marker: Option<i64>,
        env: &StepEnv<'_>,
        left_holding_ticks: usize,
    ) -> HoldMeasurements {
        let held: Vec<_> = env.results.repeating_after(Some(first_locked_frame - 1)).collect();
        let baseline = held.first().and_then(|result| exposure_product(result));
        let non_locked = match AeSource::of(&env.status.three_a) {
            AeSource::Vendor => held
                .iter()
                .filter(|result| result.ae_state != Some(AeState::Locked))
                .count(),
            AeSource::Own => left_holding_ticks,
        };
        let deviation = baseline.and_then(|baseline| {
            held.iter()
                .filter_map(|result| exposure_product(result))
                .filter_map(|product| relative_gap(product, baseline))
                .reduce(f64::max)
        });
        HoldMeasurements {
            samples_held: held.len(),
            non_locked_after_lock: non_locked,
            max_product_deviation: deviation,
            product_frozen_after_frames: Self::frozen_after_frames(lock_marker, env),
        }
    }

    /// Через сколько кадров после команды блокировки произведение замерло до конца удержания. Считаем по
    /// всем результатам повторяющегося запроса после кадра команды (в том числе до `Holding`: догон цели
    /// до сенсора виден именно там).
    fn frozen_after_frames(lock_marker: Option<i64>, env: &StepEnv<'_>) -> Option<i64> {
        let marker = lock_marker?;
        let products: Vec<(i64, f64)> = env
            .results
            .repeating_after(Some(marker))
            .filter_map(|result| exposure_product(result).map(|product| (result.frame_number.0, product)))
            .collect();
        let criteria = FrozenCriteria {
            tolerance_fraction: env.config.lock.constancy_tolerance_fraction,
            min_tail_samples: env.config.lock.product_frozen_min_tail_samples,
        };
        product_frozen_after_frames(marker, &products, criteria)
    }

    /// Один тик удержания: счётчик выхода из `Holding` и замер сцены.
    fn observe_hold_tick(&mut self, env: &StepEnv<'_>) {
        let own = AeSource::of(&env.status.three_a) == AeSource::Own;
        if own && !is_lock_reached(&env.status.three_a) {
            self.left_holding_ticks += 1;
        }
        let observed_scene = env
            .presentation
            .viewfinder
            .as_ref()
            .and_then(|viewfinder| viewfinder.auto_exposure.as_ref())
            .and_then(|auto_exposure| auto_exposure.observed_scene_log2_seconds_iso);
        if let Some(log2_seconds_iso) = observed_scene {
            self.scene_range.record(log2_seconds_iso);
        }
    }

    /// Играет сигнал и возвращает итог платформы; итог всегда попадает в отчёт и в стадию (отказ не тихий).
    fn play_cue(env: &mut StepEnv<'_>, pattern: HapticPattern, moment: &'static str) -> HapticOutcome {
        let outcome = env.haptics.play(pattern);
        env.stage(
            "three_a_lock_cue",
            [
                ("moment", moment.into()),
                ("pattern", pattern.name().into()),
                ("outcome", super::detail::to_value(&outcome)),
            ],
        );
        outcome
    }
}

impl ThreeAStep for LockStep {
    fn name(&self) -> &'static str {
        "lock"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        let handle = env.running.handle();
        // Чистый лист: ручные значения и компенсация предыдущих шагов не должны влиять на блокировку.
        sent("apply_controls", handle.apply_controls(ManualControl::AUTOMATIC))?;
        sent(
            "lock_focus_and_exposure_at",
            handle.lock_focus_and_exposure_at(SensorPoint::CENTER),
        )?;
        self.phase = Some(LockPhase::WaitLocked {
            marker: env.results.latest_frame(),
            deadline: Deadline::after_millis(env.now, env.config.lock.settle_timeout_ms),
        });
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        let now = env.now;
        match self.phase.take() {
            Some(LockPhase::WaitLocked { marker, deadline }) => {
                self.phase = match Self::locked_frame(env, marker) {
                    Some(first_locked_frame) => {
                        // Окно удержания началось: сигнал «закрывайте объектив».
                        self.haptic_hold_start = Some(Self::play_cue(env, HOLD_START_CUE, "hold_start"));
                        Some(LockPhase::Hold {
                            first_locked_frame,
                            lock_marker: marker,
                            until: now + std::time::Duration::from_millis(env.config.lock.hold_ms),
                        })
                    }
                    None if deadline.is_expired(now) => {
                        self.report = Some(not_locked_report(AeSource::of(&env.status.three_a)));
                        return self.request_reset(env, observation);
                    }
                    None => Some(LockPhase::WaitLocked { marker, deadline }),
                };
                StepProgress::Pending
            }
            Some(LockPhase::Hold {
                first_locked_frame,
                lock_marker,
                until,
            }) => {
                self.observe_hold_tick(env);
                if now < until {
                    self.phase = Some(LockPhase::Hold {
                        first_locked_frame,
                        lock_marker,
                        until,
                    });
                    return StepProgress::Pending;
                }
                // Окно кончилось: сигнал «объектив можно открывать».
                let haptic_hold_end = Self::play_cue(env, HOLD_END_CUE, "hold_end");
                let held = Self::measure_hold(first_locked_frame, lock_marker, env, self.left_holding_ticks);
                self.report = Some(LockReport {
                    ae_source: AeSource::of(&env.status.three_a),
                    locked: true,
                    samples_held: held.samples_held,
                    non_locked_after_lock: held.non_locked_after_lock,
                    max_product_deviation: held.max_product_deviation,
                    observed_scene_range_ev: self.scene_range.range_ev(),
                    product_frozen_after_frames: held.product_frozen_after_frames,
                    haptic_hold_start: self.haptic_hold_start,
                    haptic_hold_end: Some(haptic_hold_end),
                    back_to_continuous: false,
                });
                self.request_reset(env, observation)
            }
            Some(LockPhase::WaitUnlocked { deadline }) => {
                let three_a = &env.status.three_a;
                let back =
                    three_a.lock == AeAfLock::Unlocked && matches!(three_a.focus, FocusProgress::Continuous { .. });
                if back || deadline.is_expired(now) {
                    if let Some(mut report) = self.report.take() {
                        report.back_to_continuous = back;
                        observation.lock = Some(report);
                    }
                    return StepProgress::Done;
                }
                self.phase = Some(LockPhase::WaitUnlocked { deadline });
                StepProgress::Pending
            }
            None => StepProgress::Done,
        }
    }
}

impl LockStep {
    /// Сбрасывает точку интереса и начинает ждать возврата непрерывного AF.
    fn request_reset(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> StepProgress {
        match sent(
            "reset_point_of_interest",
            env.running.handle().reset_point_of_interest(),
        ) {
            Ok(_) => {
                self.phase = Some(LockPhase::WaitUnlocked {
                    deadline: Deadline::after_millis(env.now, env.config.lock.unlock_timeout_ms),
                });
                StepProgress::Pending
            }
            Err(failure) => StepProgress::Failed(failure),
        }
    }
}

fn not_locked_report(ae_source: AeSource) -> LockReport {
    LockReport {
        ae_source,
        locked: false,
        samples_held: 0,
        non_locked_after_lock: 0,
        max_product_deviation: None,
        observed_scene_range_ev: None,
        product_frozen_after_frames: None,
        haptic_hold_start: None,
        haptic_hold_end: None,
        back_to_continuous: false,
    }
}
