//! Вердикт сценария `zoom` (сессия 13): чистая функция «наблюдение → список нарушений», пустой — прошёл.
//!
//! Правила:
//! 1. запуск и вложения без отказа: камера готова, движок не в ошибке, экран принимает вход ([`ZoomObservationReport::run_failure`]);
//! 2. все шаги плана выполнены, причина провала шага названа (`zoom_runner`);
//! 3. **чёрных кадров там, где ждали картинку, ровно 0** (критерий выхода 13, решение 113) — за прогон и по шагам;
//! 4. `LensSwitchFailed` нет (счётчик движка `switches_failed` за прогон = 0): неудачная смена — провал;
//! 5. времена переключения в пределах: «действие → событие движка» и «действие → первый живой кадр новой камеры»;
//!    живой кадр не найден из-за потери записей кольца — не нарушение времени, а «не измерено» (в записи шага);
//! 6. частота потока камеры не ниже ожидаемой (задние 60, фронтальная 30) с допуском;
//! 7. зеркало: фронтальная — по горизонтали, задние — без зеркала; кадров камеры нет в кольце — «не измерено» = нарушение,
//!    если кольцо ничего не теряло;
//! 8. протяжка по колесу: зум сдвинулся, вибраций не меньше порога и не больше, чем мог пропустить ограничитель частоты
//!    за окно шага (сами промежутки не меряем: часы ограничителя и исполнителя разные, решение 112);
//!
//! Дополнительно (8а): **чёрных кадров «нет картинки» (`black_frames_before_first_picture`) за прогон — 0**: сценарий стартует, когда
//!    живой кадр уже показан, так что такой кадр значит чёрный экран без удержания при смене объектива (решения 110, 113);
//! 9. каждая смена объектива раскрылась **по условию** готовности цвета и яркости: раскрытие по сроку и удержание,
//!    закончившееся честным чёрным, — нарушения ([`super::zoom_reveal::RevealFaults`]);
//! 10. после прогона экран на 1× основной камеры, как до него (завершающий тап `1`).

use camera_contract::ids::{LensFacing, LensRole};

use super::zoom_record::{MirrorCheck, ZoomStepRecord};
use super::zoom_report::ZoomObservationReport;
use super::zoom_seamless_verdict::{seamless_step_violations, seamless_totals_violations};
use crate::zoom_scenario_config::ZoomScenarioConfig;

/// После протяжки по колесу зум обязан уйти заметно выше 1× (протяжка влево ведёт к ≈ 5×).
const MIN_RATIO_AFTER_DRAG: f64 = 1.5;
/// Коэффициент 1× в тысячных и допуск проверки конца прогона (2 %).
const ONE_X_MILLI: u32 = 1_000;
const ONE_X_TOLERANCE_MILLI: u32 = 20;

/// Нарушения по наблюдению; пустой список — сценарий прошёл.
pub fn evaluate_zoom(config: &ZoomScenarioConfig, report: &ZoomObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &report.run_failure {
        violations.push(failure.clone());
    }
    for step in &report.steps {
        if let Some(failure) = &step.failure {
            violations.push(format!("{}: {failure}", step.name));
        }
    }
    if report.run_failure.is_none()
        && report.steps.iter().all(|step| step.failure.is_none())
        && !report.unfinished.is_empty()
    {
        violations.push(format!("не выполнены шаги: {}", report.unfinished.join(", ")));
    }
    if report.totals.black_frames_while_picture_expected > 0 {
        let culprits: Vec<&str> = report
            .steps
            .iter()
            .filter(|step| step.black_frames_while_picture_expected > 0)
            .map(|step| step.name.as_str())
            .collect();
        violations.push(format!(
            "чёрных кадров там, где ждали картинку: {} (должно быть 0), шаги: {}",
            report.totals.black_frames_while_picture_expected,
            culprits.join(", ")
        ));
    }
    let black_before_picture_steps: Vec<&str> = report
        .steps
        .iter()
        .filter(|step| step.continuity.black_frames_before_first_picture > 0)
        .map(|step| step.name.as_str())
        .collect();
    if report.totals.black_frames_before_first_picture > 0 || !black_before_picture_steps.is_empty() {
        violations.push(format!(
            "чёрный кадр «нет картинки» во время смены объектива — удержание не началось: кадров {} (должно быть 0), шаги: {}",
            report
                .totals
                .black_frames_before_first_picture
                .max(report.steps.iter().map(|step| step.continuity.black_frames_before_first_picture).sum()),
            black_before_picture_steps.join(", ")
        ));
    }
    if !report.totals.reveal_faults.is_clean() {
        let culprits: Vec<&str> = report
            .steps
            .iter()
            .filter(|step| !step.reveal_faults.is_clean())
            .map(|step| step.name.as_str())
            .collect();
        violations.push(format!(
            "переход не раскрылся по условию готовности цвета и яркости ({}), шаги: {}",
            report.totals.reveal_faults.describe(),
            culprits.join(", ")
        ));
    }
    if report.totals.switches_failed > 0 {
        violations.push(format!(
            "движок сообщил о неудачных сменах объектива: {} (должно быть 0)",
            report.totals.switches_failed
        ));
    }
    if report.totals.lens_intents_published == 0 && !report.steps.is_empty() {
        violations.push("экран не положил ни одного намерения объектива в ящик движка".to_owned());
    }
    for step in &report.steps {
        violations.extend(step_violations(config, step));
    }
    violations.extend(seamless_totals_violations(report));
    violations.extend(final_state_violations(report));
    violations
}

fn step_violations(config: &ZoomScenarioConfig, step: &ZoomStepRecord) -> Vec<String> {
    let mut violations = Vec::new();
    let name = &step.name;
    // У бесшовных жестов момент пересечения порога выбирает палец, а не движок: «начало движения → событие» включает
    // путь пальца, поэтому пороги времени тапа к ним не применяются (их заменяет разрыв показа в `seamless_verdict`).
    let finger_driven = step.seamless.is_some();
    if let Some(switch) = step.switch {
        if !finger_driven && switch.observed_ms > config.max_switch_ms {
            violations.push(format!(
                "{name}: событие смены объектива через {} мс при пороге {} мс",
                switch.observed_ms, config.max_switch_ms
            ));
        }
        match step.first_live_frame_ms {
            Some(live) if !finger_driven && live > config.max_first_live_frame_ms as f64 => violations.push(format!(
                "{name}: первый живой кадр новой камеры через {live:.0} мс при пороге {} мс",
                config.max_first_live_frame_ms
            )),
            Some(_) => {}
            None if step.ring_overran => {}
            None => violations.push(format!("{name}: живой кадр новой камеры в кольце кадров не найден")),
        }
    }
    violations.extend(seamless_step_violations(&config.seamless, step));
    if !step.reveal_faults.is_clean() {
        violations.push(format!(
            "{name}: переход не раскрылся по условию готовности: {}",
            step.reveal_faults.describe()
        ));
    }
    if step.failure.is_none() {
        violations.extend(frame_rate_violation(config, step));
    }
    match step.mirror {
        MirrorCheck::Wrong { frames } => {
            violations.push(format!(
                "{name}: у {frames} кадров зеркало не то (фронтальная — по горизонтали, задние — нет)"
            ));
        }
        MirrorCheck::NoFrameObserved if !step.ring_overran => {
            violations.push(format!("{name}: кадров новой камеры в кольце нет, зеркало не измерено"));
        }
        MirrorCheck::NotApplicable | MirrorCheck::Correct | MirrorCheck::NoFrameObserved => {}
    }
    if let Some(haptics) = step.haptics {
        if step.screen_ratio <= MIN_RATIO_AFTER_DRAG {
            violations.push(format!(
                "{name}: протяжка не сдвинула зум (на экране {:.2}×, ждали больше {MIN_RATIO_AFTER_DRAG}×)",
                step.screen_ratio
            ));
        }
        if haptics.count < config.min_haptics_in_drag {
            violations.push(format!(
                "{name}: вибраций за протяжку {} при минимуме {}",
                haptics.count, config.min_haptics_in_drag
            ));
        }
        // Верхняя граница: ограничитель частоты пропускает не больше одной вибрации на `min_haptic_gap_ms` окна
        // (плюс первая). Больше — ограничитель не работает или считается лишнее. Сами промежутки не меряем: часы
        // ограничителя (время касаний) и исполнителя (время кадра UI) разные, см. `HapticWindow`.
        let max_ticks = (haptics.window_ms / config.min_haptic_gap_ms as f64).floor() as u64 + 1;
        if haptics.count > max_ticks {
            violations.push(format!(
                "{name}: вибраций за протяжку {} при потолке {max_ticks} (окно {:.0} мс, не чаще раза в {} мс)",
                haptics.count, haptics.window_ms, config.min_haptic_gap_ms
            ));
        }
    }
    violations
}

fn frame_rate_violation(config: &ZoomScenarioConfig, step: &ZoomStepRecord) -> Option<String> {
    let expected = step.expected_fps?;
    let Some(measured) = step.measured_fps else {
        return Some(format!("{}: частота потока не измерена", step.name));
    };
    let floor = f64::from(expected) * (1.0 - config.frame_rate_tolerance);
    (measured < floor).then(|| {
        format!(
            "{}: частота потока {measured:.1} к/с ниже {floor:.1} (ожидали {expected} к/с с допуском {:.0} %)",
            step.name,
            config.frame_rate_tolerance * 100.0
        )
    })
}

fn final_state_violations(report: &ZoomObservationReport) -> Vec<String> {
    let Some(state) = report.final_state else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    if state.facing != LensFacing::Back
        || state.active_role != Some(LensRole::Wide)
        || state.ratio_milli.abs_diff(ONE_X_MILLI) > ONE_X_TOLERANCE_MILLI
    {
        violations.push(format!(
            "после прогона экран не на 1× основной камеры: {} тыс., направление {:?}, объектив {:?}",
            state.ratio_milli,
            state.facing,
            state.active_role.map(LensRole::name)
        ));
    }
    violations
}

#[cfg(test)]
#[path = "zoom_verdict_tests.rs"]
mod tests;
