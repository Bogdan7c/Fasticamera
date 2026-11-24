//! Вердикт сценария `3a`: чистая функция «наблюдение → нарушения». Одно правило — одно нарушение
//! (и один тест). Невыполненные шаги (`None`) не судятся: причина обрыва уже в `failure` или в нарушении
//! раннего шага.
//!
//! Что НЕ провал, а справка в отчёте: расхождение нашей белой с белой HAL (`awb_vs_hal`), региональный
//! эффект фокуса (`region_effect_diopters`), невозможность проверить EV у предела сенсора
//! (`ev_unverifiable_at_limit`), тапы по углам, не сошедшиеся к сроку.

use camera_android::companion_stream::CompanionStreamStatus;

use super::three_a_own_ae::{AeSource, own_ae_violations};
use super::three_a_report::{
    AwbReport, BracketRunReport, EvReport, FocusReport, LiveReport, LockReport, ManualReport, TapResult,
    ThreeAObservation,
};
use super::three_a_results::relative_gap;
use super::white_balance_report::white_taken_from_phone_violations;
use crate::three_a_scenario_config::ThreeAScenarioConfig;

/// Вердикт: список нарушений (пустой — прошёл).
pub fn evaluate_three_a(config: &ThreeAScenarioConfig, observation: &ThreeAObservation) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &observation.failure {
        violations.push(failure.describe());
    }
    violations.extend(ae_source_violations(observation));
    violations.extend(observation.live.iter().flat_map(live_violations));
    violations.extend(observation.awb.iter().flat_map(awb_violations));
    violations.extend(observation.focus.iter().flat_map(focus_violations));
    violations.extend(
        observation
            .manual
            .iter()
            .flat_map(|report| manual_violations(config, report)),
    );
    violations.extend(observation.ev.iter().flat_map(|report| ev_violations(config, report)));
    violations.extend(
        observation
            .lock
            .iter()
            .flat_map(|report| lock_violations(config, report)),
    );
    violations.extend(
        observation
            .bracket
            .iter()
            .flat_map(|report| bracket_violations(config, report)),
    );
    violations.extend(observation.own_ae.iter().flat_map(own_ae_violations));
    if let Some(reason) = &observation.restore_failure {
        violations.push(format!("автоматические параметры не возвращены: {reason}"));
    }
    violations
}

/// Нарушения шага a (решение 62 в продукте): вспомогательный поток в сессии, AF и AE видны в результате.
pub fn live_violations(live: &LiveReport) -> Vec<String> {
    let mut violations = Vec::new();
    if !matches!(live.companion, CompanionStreamStatus::Active { .. }) {
        violations.push(format!(
            "вспомогательный поток 3A не активен ({:?}): автоматика HAL мертва",
            live.companion
        ));
    }
    if live.af_state.is_none() {
        violations.push("в результатах нет AF_STATE (автофокус HAL молчит)".to_owned());
    }
    // У нашего AE вендорский автомат выключен (`AE_MODE_OFF`): `AE_STATE` там — справка, а не правило.
    if live.ae_source == AeSource::Vendor && live.ae_state.is_none() {
        violations.push("в результатах нет AE_STATE (автоэкспозиция HAL молчит)".to_owned());
    }
    violations
}

/// Движок должен вести тот исполнитель экспозиции, который выбрала композиция: «настроен наш, а стоит вендорский»
/// (и наоборот) не должно проходить зелёным.
fn ae_source_violations(observation: &ThreeAObservation) -> Vec<String> {
    let Some(live) = &observation.live else {
        return Vec::new();
    };
    if live.ae_source == observation.expected_ae_source {
        return Vec::new();
    }
    vec![format!(
        "экспозицию ведёт {:?}, а композиция поставила {:?}{}",
        live.ae_source,
        observation.expected_ae_source,
        observation
            .ae_fallback_reason
            .as_deref()
            .map_or(String::new(), |reason| format!(" (замена: {reason})"))
    )]
}

pub fn awb_violations(awb: &AwbReport) -> Vec<String> {
    let mut violations = Vec::new();
    if !awb.ready {
        violations.push(
            "белая не готова за срок: заявка источнику не закрыта исходом applied или холодный старт не кончился"
                .to_owned(),
        );
    }
    violations.extend(awb.white_balance.iter().flat_map(white_taken_from_phone_violations));
    if awb.hidden_since_start > u64::from(awb.max_hidden_frames) {
        violations.push(format!(
            "холодный старт скрыл {} кадров, предел {}",
            awb.hidden_since_start, awb.max_hidden_frames
        ));
    }
    if awb.forced_since_start > 0 {
        violations.push(format!(
            "холодный старт кончился принудительным показом {} раз: ответа источника не было в пределах скрытия",
            awb.forced_since_start
        ));
    }
    violations
}

pub fn focus_violations(focus: &FocusReport) -> Vec<String> {
    let mut violations = Vec::new();
    for (index, tap) in focus.taps.iter().enumerate() {
        let is_center = index == 0;
        match tap.result {
            TapResult::Focused | TapResult::NotFocused => {}
            TapResult::TimedOut if is_center => {
                violations.push("тап в центр: HAL не сообщил итог за срок движка (TimedOut)".to_owned());
            }
            TapResult::TriggerNotDelivered => violations.push(format!(
                "тап {index} ({}, {}): бэкенд отверг разовый запрос, HAL тап не получил",
                tap.x, tap.y
            )),
            TapResult::NoEvent if is_center => {
                violations.push("тап в центр: событие итога не пришло за срок сценария".to_owned());
            }
            // Углы, не сошедшиеся к сроку, — справка для владельца.
            TapResult::TimedOut | TapResult::NoEvent => {}
        }
    }
    if focus.results_without_request_context != 0 {
        violations.push(format!(
            "результатов без контекста запроса: {} (метка триггера теряется)",
            focus.results_without_request_context
        ));
    }
    if focus.results_with_unknown_origin != 0 {
        violations.push(format!(
            "результатов с неопознанным происхождением: {}",
            focus.results_with_unknown_origin
        ));
    }
    if focus.triggers_sent != focus.taps.len() as u64 {
        violations.push(format!(
            "отправлено триггеров {}, тапов {}",
            focus.triggers_sent,
            focus.taps.len()
        ));
    }
    violations
}

pub fn manual_violations(config: &ThreeAScenarioConfig, manual: &ManualReport) -> Vec<String> {
    let tolerance = config.manual.tolerance_fraction;
    let mut violations = Vec::new();
    match manual.applied {
        None => violations.push(format!(
            "ручная экспозиция ({} нс, ISO {}) не появилась в результатах за срок",
            manual.requested_exposure_ns, manual.requested_iso
        )),
        Some(applied) => {
            let gap = relative_gap(applied.exposure_ns as f64, manual.requested_exposure_ns as f64);
            if gap.is_none_or(|gap| gap > tolerance) {
                violations.push(format!(
                    "выдержка в результате {} нс при просьбе {} нс: расхождение больше {:.0} %",
                    applied.exposure_ns,
                    manual.requested_exposure_ns,
                    tolerance * 100.0
                ));
            }
            let iso_gap = relative_gap(f64::from(applied.iso), f64::from(manual.requested_iso));
            if iso_gap.is_none_or(|gap| gap > tolerance) {
                violations.push(format!(
                    "ISO в результате {} при просьбе {}: расхождение больше {:.0} %",
                    applied.iso,
                    manual.requested_iso,
                    tolerance * 100.0
                ));
            }
        }
    }
    if manual.adjustments_for_in_range != 0 {
        violations.push(format!(
            "значения в диапазоне камеры получили {} событий ControlsAdjusted (ожидалось 0)",
            manual.adjustments_for_in_range
        ));
    }
    violations.extend(long_shutter_violations(config, manual));
    violations
}

fn long_shutter_violations(config: &ThreeAScenarioConfig, manual: &ManualReport) -> Vec<String> {
    let long = &manual.long_shutter;
    let mut violations = Vec::new();
    if !long.limited_event_seen {
        violations.push(format!(
            "выдержка {} нс длиннее кадра, а ControlsAdjusted(LimitedByFrameDuration) не пришло",
            long.requested_ns
        ));
    }
    match (long.applied_ns, long.frame_budget_ns) {
        (Some(applied), Some(budget)) => {
            let limit = budget as f64 * (1.0 + config.manual.tolerance_fraction);
            if applied as f64 > limit {
                violations.push(format!(
                    "длинная выдержка в результате {applied} нс больше длительности кадра {budget} нс"
                ));
            }
        }
        _ => violations.push("результат с ограниченной выдержкой не получен за срок".to_owned()),
    }
    violations
}

pub fn ev_violations(config: &ThreeAScenarioConfig, ev: &EvReport) -> Vec<String> {
    let tolerance = config.ev.ratio_tolerance_fraction;
    let mut violations = Vec::new();
    if ev.ae_source == AeSource::Own {
        violations.extend(own_ae_ev_violations(ev));
    }
    for point in ev.points.iter().filter(|point| point.verifiable) {
        let expected = 2.0_f64.powf(f64::from(point.bias_ev));
        match point.ratio_to_baseline {
            None => violations.push(format!(
                "EV {:+}: нет медианы (кадров {})",
                point.bias_ev, point.samples
            )),
            Some(ratio) if relative_gap(ratio, expected).is_none_or(|gap| gap > tolerance) => {
                violations.push(format!(
                    "EV {:+}: «выдержка × ISO» изменилась в {ratio:.2} раза, ожидалось {expected:.2} (допуск {:.0} %)",
                    point.bias_ev,
                    tolerance * 100.0
                ));
            }
            Some(_) => {}
        }
    }
    violations
}

/// Сходимость точек EV при нашем AE: точка без `Settled`/`Holding` и без `LimitReached` не судима; если все
/// ненулевые точки непроверяемы (упёрлись сами или упёрлась опора), компенсация не проверена (честный провал «не проверить»).
fn own_ae_ev_violations(ev: &EvReport) -> Vec<String> {
    let mut violations: Vec<String> = ev
        .points
        .iter()
        .filter(|point| !point.ae_converged && !point.ae_limit_reached)
        .map(|point| {
            format!(
                "EV {:+}: наш AE не сошёлся (ни Settled/Holding, ни LimitReached)",
                point.bias_ev
            )
        })
        .collect();
    // Непроверяемы все сдвинутые точки: упёрлись сами или упёрлась опора (12b, тёмная сцена). Молча «зелёным»
    // такой прогон быть не может — компенсацию никто не проверил.
    let mut shifted = ev.points.iter().filter(|point| point.bias_ev != 0.0).peekable();
    if shifted.peek().is_some() && shifted.all(|point| !point.verifiable) {
        violations.push(
            "EV не проверен: во всех точках с компенсацией наш AE или опорная точка упёрлись в предел камеры"
                .to_owned(),
        );
    }
    violations
}

pub fn lock_violations(config: &ThreeAScenarioConfig, lock: &LockReport) -> Vec<String> {
    let mut violations = Vec::new();
    if !lock.locked {
        violations.push(match lock.ae_source {
            AeSource::Vendor => "AE_STATE не стал Locked за срок после блокировки".to_owned(),
            AeSource::Own => "наш AE не дошёл до Holding (и не упёрся в предел) за срок после блокировки".to_owned(),
        });
    } else {
        if lock.non_locked_after_lock != 0 {
            violations.push(match lock.ae_source {
                AeSource::Vendor => format!(
                    "под блокировкой {} результатов с AE_STATE не Locked",
                    lock.non_locked_after_lock
                ),
                AeSource::Own => format!(
                    "под блокировкой наш AE вышел из Holding на {} тиках",
                    lock.non_locked_after_lock
                ),
            });
        }
        match lock.max_product_deviation {
            Some(deviation) if deviation <= config.lock.constancy_tolerance_fraction => {}
            Some(deviation) => violations.push(format!(
                "под блокировкой «выдержка × ISO» плавала на {:.1} % (допуск {:.0} %)",
                deviation * 100.0,
                config.lock.constancy_tolerance_fraction * 100.0
            )),
            None => violations.push("под блокировкой нет результатов для проверки постоянства".to_owned()),
        }
    }
    if !lock.back_to_continuous {
        violations.push("после сброса точки AF не вернулся в непрерывный режим или блокировка не снята".to_owned());
    }
    violations
}

pub fn bracket_violations(config: &ThreeAScenarioConfig, bracket: &BracketRunReport) -> Vec<String> {
    let requested = &config.bracket.exposures_ns;
    let mut violations = Vec::new();
    match bracket.captured {
        None => violations.push("итог серии (BracketCaptured) не пришёл за срок".to_owned()),
        Some(false) => violations.push(format!(
            "серия не снята: {}",
            bracket.failure.as_deref().unwrap_or("причина не указана")
        )),
        Some(true) => violations.extend(captured_bracket_violations(config, bracket)),
    }
    let expected = requested.len() as u64;
    let lowest = expected.saturating_sub(config.bracket.max_skipped_shortfall);
    if bracket.skipped_delta < lowest || bracket.skipped_delta > expected {
        violations.push(format!(
            "рендер пропустил кадров серии {}, ожидалось от {lowest} до {expected}",
            bracket.skipped_delta
        ));
    }
    if bracket.dropped_during > config.bracket.max_extra_dropped_frames {
        violations.push(format!(
            "за время серии видоискатель потерял {} кадров, допустимо {}",
            bracket.dropped_during, config.bracket.max_extra_dropped_frames
        ));
    }
    if bracket.presented_during == 0 {
        violations.push("за время серии видоискатель не показал ни одного кадра".to_owned());
    }
    violations
}

fn captured_bracket_violations(config: &ThreeAScenarioConfig, bracket: &BracketRunReport) -> Vec<String> {
    let requested = &config.bracket.exposures_ns;
    let tolerance = config.bracket.tolerance_fraction;
    let mut violations = Vec::new();
    if bracket.frames.len() != requested.len() {
        violations.push(format!(
            "в серии {} кадров, просили {}",
            bracket.frames.len(),
            requested.len()
        ));
    }
    if bracket
        .frames
        .iter()
        .enumerate()
        .any(|(position, frame)| frame.index as usize != position)
    {
        violations.push("кадры серии не в порядке индексов".to_owned());
    }
    if bracket.frame_gaps != 0 {
        violations.push(format!(
            "номера кадров серии не подряд: пропущено {}",
            bracket.frame_gaps
        ));
    }
    for (frame, requested_ns) in bracket.frames.iter().zip(requested) {
        let exposure_ok = frame
            .exposure_ns
            .and_then(|applied| relative_gap(applied as f64, *requested_ns as f64))
            .is_some_and(|gap| gap <= tolerance);
        let iso_ok = frame
            .iso
            .and_then(|applied| relative_gap(f64::from(applied), f64::from(config.bracket.iso)))
            .is_some_and(|gap| gap <= tolerance);
        if !exposure_ok || !iso_ok {
            violations.push(format!(
                "кадр {} серии: выдержка {:?} нс, ISO {:?}; просили {requested_ns} нс, ISO {}",
                frame.index, frame.exposure_ns, frame.iso, config.bracket.iso
            ));
        }
    }
    violations
}

#[cfg(test)]
#[path = "three_a_verdict_tests.rs"]
mod tests;
