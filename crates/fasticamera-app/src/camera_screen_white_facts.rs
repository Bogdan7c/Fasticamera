//! Перевод белого из статуса движка в факты экрана (white-balance 02, решения WB21, WB31, WB33).
//!
//! Экран белое не хранит: всё, что он показывает (число на значке, положение шкал, подсветка кнопки света, ожидание,
//! отказ), приходит отсюда. Модуль чистый: статус и доска показаний на входе, [`WhiteFacts`] на выходе.
//!
//! # Откуда что берётся
//! * действующее белое и происхождение — `three_a.white.held` как есть;
//! * пределы — `three_a.white.range` (из конфига движка, известны с первого снимка);
//! * «ждём источник» — заявка движка в ожидании;
//! * исход СВОЕЙ команды «взять» — отчёт `last_take` (номер = номер команды): заявка открытия приложения сюда не
//!   попадает, поэтому возобновление приложения кнопку не трясёт (WB32);
//! * показание телефона для «≈ 3100» — с доски показаний, **и только пока поток идёт**: у закрытой или
//!   перестраиваемой камеры старое число было бы враньём.
//!
//! Показание не округляется: оно меняется в долях Кельвина, но доска обновляется 4 раза в секунду, а факты уходят
//! экрану только при изменении ([`crate::camera_screen_facts::CameraFactsWatcher`]), поэтому перерисовка идёт не
//! чаще смены показания, а не «каждый кадр». Округлять показ до 10 К — дело экрана (решение WB34).

use camera_contract::white::WhiteSourceId;
use camera_contract::white_reading::{WhiteReading, WhiteReadingBoard};
use camera_core::white::{WhiteFollowState, WhiteTakeReport, WhiteTakeState as EngineTakeState};
use camera_core::white::{WhiteRange as EngineWhiteRange, WhiteRequestOutcome};
use camera_core::{CameraState, CameraStatus};
use camera_screen::{WhiteFacts, WhiteFollowFacts, WhiteRange, WhiteTakeFacts, WhiteTakeState};

/// Источник, чьё показание экран выводит на кнопке «Телефон».
const PHONE_SOURCE: WhiteSourceId = WhiteSourceId::PHONE;

/// Факты о белом для экрана по статусу движка и доске показаний.
pub(crate) fn white_facts_of(status: &CameraStatus, readings: &WhiteReadingBoard) -> WhiteFacts {
    let white = &status.three_a.white;
    WhiteFacts {
        held: white.held,
        range: range_facts_of(&white.range),
        take: white.last_take.map(take_facts_of),
        phone_reading: phone_reading_of(status, readings),
        follow: follow_facts_of(white.follow.state),
    }
}

/// Пределы движка на языке экрана (числа, без типов `camera-core`). Ими же приложение сверяет кнопки света
/// конфига с движком при старте (`RuntimeConfig`): одно место перевода — одни пределы у экрана и у проверки.
pub(crate) fn range_facts_of(range: &EngineWhiteRange) -> WhiteRange {
    WhiteRange {
        min_kelvin: range.min_temperature.kelvin(),
        max_kelvin: range.max_temperature.kelvin(),
        max_abs_duv: range.max_abs_tint.duv(),
    }
}

/// Слежение движка → факт экрана. Разобрано без `_`: новое состояние слежения не скомпилируется, пока его не
/// отнесли к «включено» или «выключено». «Заморожено блокировкой» (WB46) для человека — тот же режим: «Auto» горит.
fn follow_facts_of(state: WhiteFollowState) -> WhiteFollowFacts {
    match state {
        WhiteFollowState::Off => WhiteFollowFacts::Off,
        // 14n: пауза (CALIBRATE открыта) и блокировка AE/AF для экрана — «Auto» всё ещё включён.
        WhiteFollowState::Following { .. } | WhiteFollowState::HeldByLock { .. } | WhiteFollowState::Paused { .. } => {
            WhiteFollowFacts::On
        }
    }
}

/// Отчёт движка о своей команде «взять» → факт для экрана. Номер команды переносится как есть.
fn take_facts_of(report: WhiteTakeReport) -> WhiteTakeFacts {
    WhiteTakeFacts {
        serial: report.command.0,
        state: take_state_of(report.state),
    }
}

/// Исходы разобраны без `_`: новый исход заявки не скомпилируется, пока его не отнесли к «взято», «отменено» или
/// «отказ». Все, где белое не взято по причине источника (не знает, нет, сломался, занят, не понял, не успел), и
/// «движок не стал спрашивать» — для человека один исход: отказ.
fn take_state_of(state: EngineTakeState) -> WhiteTakeState {
    match state {
        EngineTakeState::Waiting => WhiteTakeState::Waiting,
        EngineTakeState::Finished(outcome) => match outcome {
            WhiteRequestOutcome::Applied { .. } => WhiteTakeState::Taken,
            WhiteRequestOutcome::Cancelled { .. } => WhiteTakeState::Cancelled,
            WhiteRequestOutcome::Busy
            | WhiteRequestOutcome::Unknown { .. }
            | WhiteRequestOutcome::Unavailable { .. }
            | WhiteRequestOutcome::Failed { .. }
            | WhiteRequestOutcome::NotUnderstood
            | WhiteRequestOutcome::TimedOut => WhiteTakeState::Refused,
        },
        EngineTakeState::NotAsked { .. } => WhiteTakeState::Refused,
    }
}

/// Показание телефона с доски; пока поток не идёт — `None` (прежнее число не показываем).
fn phone_reading_of(status: &CameraStatus, readings: &WhiteReadingBoard) -> Option<WhiteReading> {
    match status.state {
        CameraState::Streaming { .. } => readings.latest_of(PHONE_SOURCE),
        CameraState::Closed { .. }
        | CameraState::Opening { .. }
        | CameraState::Opened { .. }
        | CameraState::Configuring { .. }
        | CameraState::Reconfiguring { .. }
        | CameraState::Error { .. } => None,
    }
}

#[cfg(test)]
#[path = "camera_screen_white_facts_tests.rs"]
mod tests;
