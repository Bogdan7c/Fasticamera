//! Разбор состояния автоматики 3A из результата кадра (сессия 08).
//!
//! Правило результата то же, что у остальных ключей: HAL ключ не отдал — `None`; отдал, но
//! значение непригодно (не тот тип, незнакомый код перечисления, отрицательная дистанция) —
//! тоже `None`, и ключ **отдельно** попадает в список непригодных, чтобы не путать с отсутствием.
//! В сессии из одного RAW-потока `AF_STATE` не приходит вовсе (факт зонда 08) — это штатное `None`.

use camera_contract::three_a::{AeState, AfState, LensState, RequestOrigin, SceneFlicker, ThreeAResultState};
use serde::Serialize;

use crate::entry::{MetadataSource, Read, read_byte, read_float};
use crate::tags;

/// Ключи состояния 3A, по которым ведётся учёт непригодных записей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreeAResultKey {
    AfState,
    AeState,
    LensState,
    FocusDistance,
    SceneFlicker,
}

/// Состояние 3A кадра и ключи, которые были, но не прочитались (в порядке [`ThreeAResultKey`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedThreeA {
    pub state: ThreeAResultState,
    pub unreadable_keys: Vec<ThreeAResultKey>,
}

/// Разбирает состояние 3A. `origin` приходит снаружи: его знает слой колбэков (опознание запроса).
pub fn parse_three_a_result(source: &dyn MetadataSource, origin: RequestOrigin) -> ParsedThreeA {
    let mut unreadable_keys = Vec::new();
    let af_state = enum_value(
        &mut unreadable_keys,
        ThreeAResultKey::AfState,
        read_byte(source, tags::AF_STATE),
        af_state_from_code,
    );
    let ae_state = enum_value(
        &mut unreadable_keys,
        ThreeAResultKey::AeState,
        read_byte(source, tags::AE_STATE),
        ae_state_from_code,
    );
    let lens_state = enum_value(
        &mut unreadable_keys,
        ThreeAResultKey::LensState,
        read_byte(source, tags::LENS_STATE),
        lens_state_from_code,
    );
    let scene_flicker = enum_value(
        &mut unreadable_keys,
        ThreeAResultKey::SceneFlicker,
        read_byte(source, tags::SCENE_FLICKER),
        scene_flicker_from_code,
    );
    let focus_distance_diopters = focus_distance(&mut unreadable_keys, read_float(source, tags::LENS_FOCUS_DISTANCE));
    ParsedThreeA {
        state: ThreeAResultState {
            af_state,
            ae_state,
            lens_state,
            focus_distance_diopters,
            scene_flicker,
            origin,
        },
        unreadable_keys,
    }
}

/// Значение перечисления: ключ с незнакомым кодом или нечитаемый — `None` и запись в непригодные.
fn enum_value<T>(
    unreadable: &mut Vec<ThreeAResultKey>,
    key: ThreeAResultKey,
    read: Read<u8>,
    convert: fn(u8) -> Option<T>,
) -> Option<T> {
    match read {
        Ok(None) => None,
        Ok(Some(code)) => {
            let value = convert(code);
            if value.is_none() {
                unreadable.push(key);
            }
            value
        }
        Err(_) => {
            unreadable.push(key);
            None
        }
    }
}

/// Дистанция фокуса: конечная и неотрицательная (диоптрии); иное — непригодная запись.
fn focus_distance(unreadable: &mut Vec<ThreeAResultKey>, read: Read<f32>) -> Option<f32> {
    match read {
        Ok(None) => None,
        Ok(Some(diopters)) if diopters.is_finite() && diopters >= 0.0 => Some(diopters),
        Ok(Some(_)) | Err(_) => {
            unreadable.push(ThreeAResultKey::FocusDistance);
            None
        }
    }
}

fn af_state_from_code(code: u8) -> Option<AfState> {
    Some(match code {
        tags::AF_STATE_INACTIVE => AfState::Inactive,
        tags::AF_STATE_PASSIVE_SCAN => AfState::PassiveScan,
        tags::AF_STATE_PASSIVE_FOCUSED => AfState::PassiveFocused,
        tags::AF_STATE_PASSIVE_UNFOCUSED => AfState::PassiveUnfocused,
        tags::AF_STATE_ACTIVE_SCAN => AfState::ActiveScan,
        tags::AF_STATE_FOCUSED_LOCKED => AfState::FocusedLocked,
        tags::AF_STATE_NOT_FOCUSED_LOCKED => AfState::NotFocusedLocked,
        _ => return None,
    })
}

fn ae_state_from_code(code: u8) -> Option<AeState> {
    Some(match code {
        tags::AE_STATE_INACTIVE => AeState::Inactive,
        tags::AE_STATE_SEARCHING => AeState::Searching,
        tags::AE_STATE_CONVERGED => AeState::Converged,
        tags::AE_STATE_LOCKED => AeState::Locked,
        tags::AE_STATE_FLASH_REQUIRED => AeState::FlashRequired,
        tags::AE_STATE_PRECAPTURE => AeState::Precapture,
        _ => return None,
    })
}

fn lens_state_from_code(code: u8) -> Option<LensState> {
    Some(match code {
        tags::LENS_STATE_STATIONARY => LensState::Stationary,
        tags::LENS_STATE_MOVING => LensState::Moving,
        _ => return None,
    })
}

fn scene_flicker_from_code(code: u8) -> Option<SceneFlicker> {
    Some(match code {
        tags::SCENE_FLICKER_NONE => SceneFlicker::None,
        tags::SCENE_FLICKER_50HZ => SceneFlicker::Mains50Hz,
        tags::SCENE_FLICKER_60HZ => SceneFlicker::Mains60Hz,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "three_a_result_tests.rs"]
mod tests;
