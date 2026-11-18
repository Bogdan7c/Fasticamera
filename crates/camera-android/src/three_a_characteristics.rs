//! Разбор возможностей автоматики 3A из характеристик камеры (сессия 08).
//!
//! Правило то же, что у остальных характеристик: ключ, которого HAL не дал, — честное «не
//! поддерживается» **и заметка** (`CharacteristicNote`), а не выдуманное значение; ключ, который
//! есть, но непригоден, — то же самое, но заметка называет проблему. Паники нет: описать камеру
//! без 3A можно, просто автоматика на ней будет выключена.

use camera_contract::capabilities::Rational;
use camera_contract::three_a::{
    AfModeSupport, AntibandingModeSupport, ExposureCompensationSupport, LockSupport, MaxRegions, ThreeACapabilities,
};

use crate::characteristics::Notes;
use crate::entry::{EntryProblem, MetadataSource, read_byte, read_bytes, read_int32s, read_rationals};
use crate::tags;

/// Сколько чисел в `ACAMERA_CONTROL_MAX_REGIONS`: AE, AWB, AF — в таком порядке (заголовок NDK).
const MAX_REGIONS_ENTRY_LEN: usize = 3;

/// Разбирает все возможности 3A камеры.
pub(crate) fn parse_three_a(source: &dyn MetadataSource, notes: &mut Notes) -> ThreeACapabilities {
    ThreeACapabilities {
        exposure_compensation: parse_exposure_compensation(source, notes),
        max_regions: parse_max_regions(source, notes),
        af_modes: parse_af_modes(source, notes),
        ae_lock: parse_ae_lock(source, notes),
        antibanding_modes: parse_antibanding_modes(source, notes),
    }
}

/// Компенсация экспозиции: диапазон в шагах и размер шага. Нужны **оба** ключа; диапазон `[0, 0]`
/// означает «компенсации нет» (так говорит HAL у камер без неё).
fn parse_exposure_compensation(source: &dyn MetadataSource, notes: &mut Notes) -> ExposureCompensationSupport {
    let range = notes.optional(
        tags::AE_COMPENSATION_RANGE,
        read_int32s(source, tags::AE_COMPENSATION_RANGE, 2),
    );
    let step = notes.optional(
        tags::AE_COMPENSATION_STEP,
        read_rationals(source, tags::AE_COMPENSATION_STEP, 1),
    );
    let (Some(range), Some(step)) = (range, step) else {
        return ExposureCompensationSupport::NotSupported;
    };
    let (min_steps, max_steps) = (range[0], range[1]);
    let step_ev: Rational = step[0];
    // Шаг с нулевым или отрицательным знаменателем/числителем бессмыслен: перевести шаги в EV нельзя.
    if min_steps > max_steps || step_ev.numerator <= 0 || step_ev.denominator <= 0 {
        notes.malformed(tags::AE_COMPENSATION_RANGE, EntryProblem::OutOfRange);
        return ExposureCompensationSupport::NotSupported;
    }
    if min_steps == 0 && max_steps == 0 {
        return ExposureCompensationSupport::NotSupported;
    }
    ExposureCompensationSupport::Supported {
        min_steps,
        max_steps,
        step_ev,
    }
}

/// Число регионов AE/AWB/AF. Отрицательное число — непригодная запись: все нули и заметка.
fn parse_max_regions(source: &dyn MetadataSource, notes: &mut Notes) -> MaxRegions {
    let Some(values) = notes.optional(
        tags::MAX_REGIONS,
        read_int32s(source, tags::MAX_REGIONS, MAX_REGIONS_ENTRY_LEN),
    ) else {
        return MaxRegions::default();
    };
    let converted: Result<Vec<u32>, _> = values
        .iter()
        .take(MAX_REGIONS_ENTRY_LEN)
        .map(|value| u32::try_from(*value))
        .collect();
    match converted.as_deref() {
        Ok([auto_exposure, auto_white_balance, auto_focus]) => MaxRegions {
            auto_exposure: *auto_exposure,
            auto_white_balance: *auto_white_balance,
            auto_focus: *auto_focus,
        },
        _ => {
            notes.malformed(tags::MAX_REGIONS, EntryProblem::OutOfRange);
            MaxRegions::default()
        }
    }
}

/// Режимы AF, которые объявила камера. Незнакомый код (вендорский) пропускается **с заметкой**.
fn parse_af_modes(source: &dyn MetadataSource, notes: &mut Notes) -> Vec<AfModeSupport> {
    let codes = notes
        .optional(
            tags::AF_AVAILABLE_MODES,
            read_bytes(source, tags::AF_AVAILABLE_MODES, 0),
        )
        .unwrap_or_default();
    let mut skipped_unknown = false;
    let modes = codes
        .into_iter()
        .filter_map(|code| {
            let mode = match code {
                tags::AF_MODE_OFF => AfModeSupport::Off,
                tags::AF_MODE_AUTO => AfModeSupport::Auto,
                tags::AF_MODE_MACRO => AfModeSupport::Macro,
                tags::AF_MODE_CONTINUOUS_VIDEO => AfModeSupport::ContinuousVideo,
                tags::AF_MODE_CONTINUOUS_PICTURE => AfModeSupport::ContinuousPicture,
                tags::AF_MODE_EDOF => AfModeSupport::ExtendedDepthOfField,
                _ => {
                    skipped_unknown = true;
                    return None;
                }
            };
            Some(mode)
        })
        .collect();
    if skipped_unknown {
        notes.substituted(tags::AF_AVAILABLE_MODES, "незнакомые режимы AF пропущены");
    }
    modes
}

/// Доступна ли блокировка AE. Нет ключа или непонятное значение — «недоступна».
fn parse_ae_lock(source: &dyn MetadataSource, notes: &mut Notes) -> LockSupport {
    match notes.optional(tags::AE_LOCK_AVAILABLE, read_byte(source, tags::AE_LOCK_AVAILABLE)) {
        Some(tags::AE_LOCK_AVAILABLE_TRUE) => LockSupport::Available,
        Some(tags::AE_LOCK_AVAILABLE_FALSE) | None => LockSupport::NotAvailable,
        Some(_) => {
            notes.malformed(tags::AE_LOCK_AVAILABLE, EntryProblem::OutOfRange);
            LockSupport::NotAvailable
        }
    }
}

/// Режимы антибандинга. Незнакомый код пропускается с заметкой.
fn parse_antibanding_modes(source: &dyn MetadataSource, notes: &mut Notes) -> Vec<AntibandingModeSupport> {
    let codes = notes
        .optional(
            tags::AE_AVAILABLE_ANTIBANDING_MODES,
            read_bytes(source, tags::AE_AVAILABLE_ANTIBANDING_MODES, 0),
        )
        .unwrap_or_default();
    let mut skipped_unknown = false;
    let modes = codes
        .into_iter()
        .filter_map(|code| {
            let mode = match code {
                tags::ANTIBANDING_MODE_OFF => AntibandingModeSupport::Off,
                tags::ANTIBANDING_MODE_50HZ => AntibandingModeSupport::Hz50,
                tags::ANTIBANDING_MODE_60HZ => AntibandingModeSupport::Hz60,
                tags::ANTIBANDING_MODE_AUTO => AntibandingModeSupport::Auto,
                _ => {
                    skipped_unknown = true;
                    return None;
                }
            };
            Some(mode)
        })
        .collect();
    if skipped_unknown {
        notes.substituted(
            tags::AE_AVAILABLE_ANTIBANDING_MODES,
            "незнакомые режимы антибандинга пропущены",
        );
    }
    modes
}

#[cfg(test)]
#[path = "three_a_characteristics_tests.rs"]
mod tests;
