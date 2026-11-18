//! Управляющая часть запроса: из [`RequestControls`] движка в записи тегов Camera2 (сессия 08).
//!
//! Движок уже решил **что** делать (вендорский AE или ручной сенсор, какой фокус, какие регионы,
//! блокировка); здесь решается только **как это записать** на конкретной камере: какие режимы
//! она объявила, сколько регионов принимает, как перевести регион в пиксели. Всё, чего камера
//! не умеет, не записывается, а попадает в [`PlanNote`] — бэкенд считает их в диагностике, чтобы
//! «не применилось» не исчезало молча. Функции чистые: ни NDK, ни логирования.
//!
//! Баланс белого (`CONTROL_AWB_MODE_AUTO`) HAL считает только для диагностики: на RAW-пиксели
//! он не влияет никогда (решения 8, 62), а наш AWB работает на потоке рендера (решение 64).

use camera_contract::capabilities::{CameraCapabilities, FocusCapability};
use camera_contract::control::CaptureIntent;
use camera_contract::stream::TargetFrameRate;
use camera_contract::three_a::{
    AeAfLock, AfModeSupport, AntibandingModeSupport, ExposureCompensationSupport, LockSupport, MainsFrequency,
    MeteringRegion,
};
use camera_core::request_controls::{AfTrigger, ExposurePlan, FocusPlan, RequestControls};
use serde::Serialize;

use crate::entry::EntryValue;
use crate::metering_region::region_to_active_array;
use crate::request_plan::PlannedEntry;
use crate::tags::{self, Tag};

/// Что из желаемого не удалось записать в запрос — и почему.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "note")]
pub enum PlanNote {
    /// Камера не принимает регион AE (`MAX_REGIONS[AE] == 0`): замер идёт по всему кадру.
    AeRegionUnsupported,
    /// Камера не принимает регион AF (`MAX_REGIONS[AF] == 0`).
    AfRegionUnsupported,
    /// Регион не получилось перевести в пиксели (вырожденная активная область).
    RegionUnmappable,
    /// Камера не объявила нужный режим AF; фокусные теги не записаны.
    AfModeUnsupported { requested: AfModeSupport },
    /// У камеры фиксированный фокус: план фокуса и триггер проигнорированы.
    FocusIgnoredOnFixedLens,
    /// Блокировка AE запрошена, но камера её не объявляет.
    AeLockUnavailable,
    /// Компенсация запрошена, но камера её не поддерживает.
    CompensationUnsupported { requested_steps: i32 },
    /// Компенсация вне диапазона камеры: записана ближайшая допустимая.
    CompensationClamped { requested_steps: i32, applied_steps: i32 },
    /// Камера не объявила нужный антибандинг: тег не записан, остаётся умолчание HAL.
    AntibandingUnsupported { requested: AntibandingModeSupport },
}

impl PlanNote {
    /// Короткое устойчивое имя для счётчика в диагностике.
    pub fn name(self) -> &'static str {
        match self {
            PlanNote::AeRegionUnsupported => "ae_region_unsupported",
            PlanNote::AfRegionUnsupported => "af_region_unsupported",
            PlanNote::RegionUnmappable => "region_unmappable",
            PlanNote::AfModeUnsupported { .. } => "af_mode_unsupported",
            PlanNote::FocusIgnoredOnFixedLens => "focus_ignored_on_fixed_lens",
            PlanNote::AeLockUnavailable => "ae_lock_unavailable",
            PlanNote::CompensationUnsupported { .. } => "compensation_unsupported",
            PlanNote::CompensationClamped { .. } => "compensation_clamped",
            PlanNote::AntibandingUnsupported { .. } => "antibanding_unsupported",
        }
    }
}

/// Всё, что нужно знать о камере и потоке при переводе плана в записи.
#[derive(Debug, Clone, Copy)]
pub struct ControlsContext<'a> {
    pub intent: CaptureIntent,
    pub camera: &'a CameraCapabilities,
    pub mains: MainsFrequency,
    pub frame_rate: TargetFrameRate,
}

/// Записи управляющей части и заметки о том, что записать не удалось.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ControlsEntries {
    pub entries: Vec<PlannedEntry>,
    pub notes: Vec<PlanNote>,
}

/// Строит записи управляющей части. `trigger` — разовое действие AF для этого запроса.
pub fn controls_entries(
    context: &ControlsContext<'_>,
    controls: &RequestControls,
    trigger: Option<AfTrigger>,
) -> ControlsEntries {
    let mut out = ControlsEntries::default();
    // Общий режим 3A и баланс белого HAL: всегда авто (баланс — только диагностика, см. доку модуля).
    out.push_byte(tags::CONTROL_MODE, tags::CONTROL_MODE_AUTO);
    out.push_byte(tags::AWB_MODE, tags::AWB_MODE_AUTO);
    exposure_entries(context, &controls.exposure, &mut out);
    focus_entries(context, &controls.focus, &mut out);
    trigger_entries(context, trigger, &mut out);
    out
}

impl ControlsEntries {
    fn push(&mut self, tag: Tag, value: EntryValue) {
        self.entries.push(PlannedEntry::Public { tag, value });
    }

    fn push_byte(&mut self, tag: Tag, value: u8) {
        self.push(tag, EntryValue::Bytes(vec![value]));
    }

    fn note(&mut self, note: PlanNote) {
        self.notes.push(note);
    }
}

// ---------------------------------------------------------------------------------------------
// Экспозиция
// ---------------------------------------------------------------------------------------------

fn exposure_entries(context: &ControlsContext<'_>, plan: &ExposurePlan, out: &mut ControlsEntries) {
    match plan {
        ExposurePlan::VendorAuto {
            compensation_steps,
            lock,
            region,
        } => vendor_auto_entries(context, *compensation_steps, *lock, *region, out),
        ExposurePlan::Sensor { exposure_time, iso } => {
            let exposure_nanos = saturating_i64(u64::try_from(exposure_time.duration().as_nanos()).unwrap_or(u64::MAX));
            // Кадр не может быть короче выдержки: HAL ограничит сам, но мы просим честно.
            let frame_duration_nanos =
                saturating_i64(context.frame_rate.frame_duration().as_nanos()).max(exposure_nanos);
            out.push_byte(tags::AE_MODE, tags::AE_MODE_OFF);
            out.push(tags::SENSOR_EXPOSURE_TIME, EntryValue::Int64s(vec![exposure_nanos]));
            out.push(
                tags::SENSOR_SENSITIVITY,
                EntryValue::Int32s(vec![saturating_i32(iso.value())]),
            );
            out.push(
                tags::SENSOR_FRAME_DURATION,
                EntryValue::Int64s(vec![frame_duration_nanos]),
            );
        }
    }
}

fn vendor_auto_entries(
    context: &ControlsContext<'_>,
    compensation_steps: i32,
    lock: AeAfLock,
    region: Option<MeteringRegion>,
    out: &mut ControlsEntries,
) {
    let three_a = &context.camera.three_a;
    out.push_byte(tags::AE_MODE, tags::AE_MODE_ON);

    match three_a.exposure_compensation {
        ExposureCompensationSupport::Supported {
            min_steps, max_steps, ..
        } => {
            let applied_steps = compensation_steps.clamp(min_steps, max_steps);
            if applied_steps != compensation_steps {
                out.note(PlanNote::CompensationClamped {
                    requested_steps: compensation_steps,
                    applied_steps,
                });
            }
            out.push(tags::AE_EXPOSURE_COMPENSATION, EntryValue::Int32s(vec![applied_steps]));
        }
        ExposureCompensationSupport::NotSupported => {
            if compensation_steps != 0 {
                out.note(PlanNote::CompensationUnsupported {
                    requested_steps: compensation_steps,
                });
            }
        }
    }

    match (lock, three_a.ae_lock) {
        (AeAfLock::Locked, LockSupport::Available) => out.push_byte(tags::AE_LOCK, tags::AE_LOCK_ON),
        (AeAfLock::Locked, LockSupport::NotAvailable) => out.note(PlanNote::AeLockUnavailable),
        // Снятая блокировка пишется явно: запрос строится с нуля и не должен зависеть от шаблона.
        (AeAfLock::Unlocked, LockSupport::Available) => out.push_byte(tags::AE_LOCK, tags::AE_LOCK_OFF),
        (AeAfLock::Unlocked, LockSupport::NotAvailable) => {}
    }

    if let Some(region) = region {
        push_region(
            context,
            tags::AE_REGIONS,
            three_a.max_regions.auto_exposure,
            PlanNote::AeRegionUnsupported,
            region,
            out,
        );
    }

    let (requested, code) = match context.mains {
        MainsFrequency::Hz50 => (AntibandingModeSupport::Hz50, tags::ANTIBANDING_MODE_50HZ),
        MainsFrequency::Hz60 => (AntibandingModeSupport::Hz60, tags::ANTIBANDING_MODE_60HZ),
    };
    if three_a.antibanding_modes.contains(&requested) {
        out.push_byte(tags::AE_ANTIBANDING_MODE, code);
    } else {
        out.note(PlanNote::AntibandingUnsupported { requested });
    }
}

// ---------------------------------------------------------------------------------------------
// Фокус
// ---------------------------------------------------------------------------------------------

fn focus_entries(context: &ControlsContext<'_>, plan: &FocusPlan, out: &mut ControlsEntries) {
    if matches!(plan, FocusPlan::FixedLens) {
        return;
    }
    // Фиксированный фокус (фронт): режимы AF бессмысленны и могут отвергаться — не пишем ничего.
    if context.camera.focus == FocusCapability::Fixed {
        out.note(PlanNote::FocusIgnoredOnFixedLens);
        return;
    }
    let three_a = &context.camera.three_a;
    match plan {
        FocusPlan::FixedLens => {}
        FocusPlan::Continuous { region } => {
            let (mode, code) = match context.intent {
                CaptureIntent::Preview => (AfModeSupport::ContinuousPicture, tags::AF_MODE_CONTINUOUS_PICTURE),
                CaptureIntent::VideoRecord => (AfModeSupport::ContinuousVideo, tags::AF_MODE_CONTINUOUS_VIDEO),
            };
            let mode_written = push_af_mode(context, mode, code, out);
            if let (true, Some(region)) = (mode_written, region) {
                push_af_region(context, three_a.max_regions.auto_focus, *region, out);
            }
        }
        FocusPlan::SingleShot { region } => {
            if push_af_mode(context, AfModeSupport::Auto, tags::AF_MODE_AUTO, out) {
                push_af_region(context, three_a.max_regions.auto_focus, *region, out);
            }
        }
        FocusPlan::Manual { distance } => {
            if push_af_mode(context, AfModeSupport::Off, tags::AF_MODE_OFF, out) {
                out.push(tags::LENS_FOCUS_DISTANCE, EntryValue::Floats(vec![distance.diopters()]));
            }
        }
    }
}

/// Пишет режим AF, если камера его объявила; иначе заметка и `false` (остальное про фокус не пишется).
fn push_af_mode(context: &ControlsContext<'_>, mode: AfModeSupport, code: u8, out: &mut ControlsEntries) -> bool {
    if context.camera.three_a.supports_af_mode(mode) {
        out.push_byte(tags::AF_MODE, code);
        true
    } else {
        out.note(PlanNote::AfModeUnsupported { requested: mode });
        false
    }
}

fn push_af_region(context: &ControlsContext<'_>, max_regions: u32, region: MeteringRegion, out: &mut ControlsEntries) {
    push_region(
        context,
        tags::AF_REGIONS,
        max_regions,
        PlanNote::AfRegionUnsupported,
        region,
        out,
    );
}

/// Пишет регион, если камера принимает хотя бы один; иначе — заметка, регион не пишется.
fn push_region(
    context: &ControlsContext<'_>,
    tag: Tag,
    max_regions: u32,
    unsupported: PlanNote,
    region: MeteringRegion,
    out: &mut ControlsEntries,
) {
    if max_regions == 0 {
        out.note(unsupported);
        return;
    }
    match region_to_active_array(region, context.camera.active_array.size) {
        Some(entry) => out.push(tag, EntryValue::Int32s(entry.to_vec())),
        None => out.note(PlanNote::RegionUnmappable),
    }
}

// ---------------------------------------------------------------------------------------------
// Триггер
// ---------------------------------------------------------------------------------------------

fn trigger_entries(context: &ControlsContext<'_>, trigger: Option<AfTrigger>, out: &mut ControlsEntries) {
    let Some(trigger) = trigger else {
        return;
    };
    if context.camera.focus == FocusCapability::Fixed {
        out.note(PlanNote::FocusIgnoredOnFixedLens);
        return;
    }
    let code = match trigger {
        AfTrigger::Start => tags::AF_TRIGGER_START,
        AfTrigger::Cancel => tags::AF_TRIGGER_CANCEL,
    };
    out.push_byte(tags::AF_TRIGGER, code);
}

fn saturating_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
