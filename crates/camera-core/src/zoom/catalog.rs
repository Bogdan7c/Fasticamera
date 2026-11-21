//! Построение шкалы зума из каталога камер (чистая функция).
//!
//! Правила (решения 111, 112, 115):
//! * в шкалу входят только камеры с ролью в каталоге (классификатор ролей уже исключил
//!   логические и системные камеры);
//! * задняя группа — `UltraWide/Wide/Tele`, опорная камера — `Wide` («1×»); фронтальная группа —
//!   `Front`, опорная она сама;
//! * коэффициент камеры — отношение эквивалентных фокусных расстояний к опорной камере
//!   (`Optics::equivalent_focal_length_mm`), а не число в коде. Нет фокусного расстояния — камера
//!   исключается с диагностикой, коэффициент не угадывается;
//! * цифровой зум камеры — честное `min(HAL, предел направления ÷ native)`; ключа HAL нет — цифрового
//!   зума нет (1×), угадывать не будем;
//! * упоры: настоящие объективы + цифровые кропы из конфига (только задняя группа, у фронтальной
//!   ленты зума нет — решение 115), кроп не создаётся рядом с настоящим объективом.

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::catalog::CameraCatalog;
use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::lens_calibration::{CalibrationPlausibility, OpticalAxis, UncalibratedReason};
use camera_contract::viewfinder_crop::ZoomFactor;
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};
use serde::Serialize;

use super::ZoomConfig;

/// Почему камера или группа получилась не такой, как ожидалось. Ничего не молчит.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ZoomDiagnostic {
    /// У камеры с ролью нет эквивалентного фокусного расстояния (битые метаданные): исключена.
    FocalLengthUnknown { camera: CameraId, role: LensRole },
    /// Коэффициент камеры не получился конечным положительным числом: исключена.
    NativeRatioInvalid { camera: CameraId, role: LensRole },
    /// HAL не отдал `availableMaxDigitalZoom`: цифрового зума у камеры нет (1×).
    MaxDigitalZoomNotReported { camera: CameraId },
    /// HAL отдал неправдоподобный `availableMaxDigitalZoom` (NaN или меньше 1): цифрового зума нет.
    MaxDigitalZoomInvalid { camera: CameraId, value: f32 },
    /// Опорной камеры направления нет (или она исключена): шкалы нет вовсе.
    ReferenceLensMissing { facing: LensFacing },
    /// Профиль требует не верить оси камеры, которой нет в каталоге (чужая сборка телефона): игнорируется.
    UntrustedCameraNotInCatalog { camera: String },
}

/// Шкала зума и диагностика её построения.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomCatalogBuild {
    pub catalog: ZoomCatalog,
    pub diagnostics: Vec<ZoomDiagnostic>,
}

/// Шкала зума без диагностики (см. [`build_zoom_catalog_reporting`]).
pub fn build_zoom_catalog(catalog: &CameraCatalog, config: &ZoomConfig) -> ZoomCatalog {
    build_zoom_catalog_reporting(catalog, config).catalog
}

/// Шкала зума и диагностика: что исключено и почему.
pub fn build_zoom_catalog_reporting(catalog: &CameraCatalog, config: &ZoomConfig) -> ZoomCatalogBuild {
    let mut diagnostics = Vec::new();
    report_unknown_untrusted_cameras(catalog, config, &mut diagnostics);
    let back = build_group(
        catalog,
        config,
        GroupSpec {
            facing: LensFacing::Back,
            roles: &[LensRole::UltraWide, LensRole::Wide, LensRole::Tele],
            reference: LensRole::Wide,
            max_ratio: config.back_max_ratio,
            with_digital_crops: true,
        },
        &mut diagnostics,
    );
    let front = build_group(
        catalog,
        config,
        GroupSpec {
            facing: LensFacing::Front,
            roles: &[LensRole::Front],
            reference: LensRole::Front,
            max_ratio: config.front_max_ratio,
            with_digital_crops: false,
        },
        &mut diagnostics,
    );
    ZoomCatalogBuild {
        catalog: ZoomCatalog { back, front },
        diagnostics,
    }
}

/// Профиль называет камеры, которых в каталоге нет: молча не пропускаем, а пишем в диагностику.
fn report_unknown_untrusted_cameras(
    catalog: &CameraCatalog,
    config: &ZoomConfig,
    diagnostics: &mut Vec<ZoomDiagnostic>,
) {
    for id in &config.untrusted_optical_axis_cameras {
        let known = CameraId::new(id.as_str()).is_ok_and(|camera| catalog.camera(&camera).is_some());
        if !known {
            diagnostics.push(ZoomDiagnostic::UntrustedCameraNotInCatalog { camera: id.clone() });
        }
    }
}

/// Оптическая ось камеры: по калибровке HAL, если профиль телефона ей не отказал в доверии.
/// Недоверенная ось даёт `Uncalibrated`, и кадрирование идёт по центру сенсора (`crop_center`).
fn optical_axis_for(descriptor: &CameraDescriptor, config: &ZoomConfig) -> OpticalAxis {
    let capabilities = &descriptor.capabilities;
    if config
        .untrusted_optical_axis_cameras
        .iter()
        .any(|id| id == descriptor.id.as_str())
    {
        return OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::DistrustedByProfile,
        };
    }
    let plausibility = CalibrationPlausibility {
        max_principal_point_offset: config.calibration_max_principal_point_offset,
    };
    capabilities
        .optics
        .calibration
        .optical_axis(capabilities.active_array, &plausibility)
}

/// Описание группы: какие роли в неё входят и чем она ограничена.
struct GroupSpec {
    facing: LensFacing,
    roles: &'static [LensRole],
    reference: LensRole,
    /// Предел коэффициента направления из конфига.
    max_ratio: f32,
    with_digital_crops: bool,
}

/// Эквивалентное фокусное расстояние камеры, мм (`None` — метаданные битые).
fn equivalent_focal_mm(descriptor: &CameraDescriptor) -> Option<f64> {
    descriptor
        .capabilities
        .optics
        .equivalent_focal_length_mm()
        .filter(|focal| focal.is_finite() && *focal > 0.0)
}

fn build_group(
    catalog: &CameraCatalog,
    config: &ZoomConfig,
    spec: GroupSpec,
    diagnostics: &mut Vec<ZoomDiagnostic>,
) -> Option<LensGroup> {
    // Опорная камера: без неё коэффициенты не от чего считать.
    let reference_focal = reference_focal_mm(catalog, &spec, diagnostics);
    let Some(reference_focal) = reference_focal else {
        if catalog_has_any_role(catalog, spec.roles) {
            diagnostics.push(ZoomDiagnostic::ReferenceLensMissing { facing: spec.facing });
        }
        return None;
    };
    let mut lenses: Vec<LensZoomPoint> = spec
        .roles
        .iter()
        .filter_map(|role| lens_point(catalog, config, &spec, *role, reference_focal, diagnostics))
        .collect();
    // Инвариант группы: камеры по возрастанию коэффициента.
    lenses.sort_by(|left, right| left.native_ratio.value().total_cmp(&right.native_ratio.value()));
    let range = group_range(&lenses, spec.max_ratio)?;
    let stops = group_stops(&lenses, &range, config, &spec);
    Some(LensGroup {
        facing: spec.facing,
        lenses,
        stops,
        range,
    })
}

fn catalog_has_any_role(catalog: &CameraCatalog, roles: &[LensRole]) -> bool {
    roles
        .iter()
        .any(|role| catalog.roles().iter().any(|binding| binding.role == *role))
}

/// Фокусное расстояние опорной камеры группы; её собственные проблемы — тоже в диагностику.
fn reference_focal_mm(catalog: &CameraCatalog, spec: &GroupSpec, diagnostics: &mut Vec<ZoomDiagnostic>) -> Option<f64> {
    let binding = catalog.roles().iter().find(|binding| binding.role == spec.reference)?;
    let descriptor = catalog.camera(&binding.camera)?;
    let focal = equivalent_focal_mm(descriptor);
    if focal.is_none() {
        diagnostics.push(ZoomDiagnostic::FocalLengthUnknown {
            camera: binding.camera.clone(),
            role: binding.role,
        });
    }
    focal
}

/// Точка камеры роли `role` на шкале группы или `None` (роли нет в каталоге либо камера исключена).
fn lens_point(
    catalog: &CameraCatalog,
    config: &ZoomConfig,
    spec: &GroupSpec,
    role: LensRole,
    reference_focal_mm: f64,
    diagnostics: &mut Vec<ZoomDiagnostic>,
) -> Option<LensZoomPoint> {
    let binding = catalog.roles().iter().find(|binding| binding.role == role)?;
    let descriptor = catalog.camera(&binding.camera)?;
    let Some(focal_mm) = equivalent_focal_mm(descriptor) else {
        // Опорная уже записана в диагностику в `reference_focal_mm`; остальные — здесь.
        if role != spec.reference {
            diagnostics.push(ZoomDiagnostic::FocalLengthUnknown {
                camera: binding.camera.clone(),
                role,
            });
        }
        return None;
    };
    let Ok(native_ratio) = ZoomRatio::new((focal_mm / reference_focal_mm) as f32) else {
        diagnostics.push(ZoomDiagnostic::NativeRatioInvalid {
            camera: binding.camera.clone(),
            role,
        });
        return None;
    };
    let capabilities = &descriptor.capabilities;
    Some(LensZoomPoint {
        role,
        camera: binding.camera.clone(),
        native_ratio,
        equivalent_focal_mm: focal_mm as f32,
        optical_axis: optical_axis_for(descriptor, config),
        active_array: capabilities.active_array.size,
        preview_mirror: match spec.facing {
            LensFacing::Front => PreviewMirror::Horizontal,
            LensFacing::Back | LensFacing::External => PreviewMirror::None,
        },
        max_digital_zoom: lens_digital_zoom(descriptor, native_ratio, spec.max_ratio, diagnostics),
    })
}

/// Цифровой зум камеры: `min(HAL, предел направления ÷ native)`, не меньше 1.
///
/// Нет ключа HAL — цифрового зума нет: кадрировать «на глаз» без заявленного предела HAL не
/// будем (инвариант 6 README), причина — в диагностике.
fn lens_digital_zoom(
    descriptor: &CameraDescriptor,
    native_ratio: ZoomRatio,
    direction_max_ratio: f32,
    diagnostics: &mut Vec<ZoomDiagnostic>,
) -> ZoomFactor {
    let camera = &descriptor.id;
    let reported = match descriptor.capabilities.optics.max_digital_zoom {
        None => {
            diagnostics.push(ZoomDiagnostic::MaxDigitalZoomNotReported { camera: camera.clone() });
            1.0
        }
        Some(value) if value.is_finite() && value >= 1.0 => value,
        Some(value) => {
            diagnostics.push(ZoomDiagnostic::MaxDigitalZoomInvalid {
                camera: camera.clone(),
                value,
            });
            1.0
        }
    };
    let allowed_by_direction = direction_max_ratio / native_ratio.value();
    // Оба слагаемых конечны и ≥ 1 после `max`; запасной путь — без зума.
    ZoomFactor::new(reported.min(allowed_by_direction).max(1.0)).unwrap_or(ZoomFactor::NONE)
}

/// Диапазон группы: от самого короткого объектива до `min(предел направления, дальность самого
/// длинного объектива)`. `None` — объективов нет.
fn group_range(lenses: &[LensZoomPoint], direction_max_ratio: f32) -> Option<ZoomRange> {
    let shortest = lenses.first()?;
    let longest = lenses.last()?;
    let max = direction_max_ratio
        .min(longest.max_ratio().value())
        .max(shortest.native_ratio.value());
    Some(ZoomRange {
        min: shortest.native_ratio,
        max: ZoomRatio::new(max).unwrap_or(shortest.native_ratio),
    })
}

/// Упоры: объективы и (для задней группы) цифровые кропы внутри дальности опорной камеры.
fn group_stops(lenses: &[LensZoomPoint], range: &ZoomRange, config: &ZoomConfig, spec: &GroupSpec) -> Vec<ZoomStop> {
    let mut stops: Vec<ZoomStop> = lenses
        .iter()
        .map(|lens| ZoomStop {
            ratio: lens.native_ratio,
            kind: ZoomStopKind::Lens(lens.role),
        })
        .collect();
    if spec.with_digital_crops {
        stops.extend(digital_crop_stops(lenses, range, config, spec.reference));
    }
    stops.sort_by(|left, right| left.ratio.value().total_cmp(&right.ratio.value()));
    stops
}

fn digital_crop_stops(
    lenses: &[LensZoomPoint],
    range: &ZoomRange,
    config: &ZoomConfig,
    reference: LensRole,
) -> Vec<ZoomStop> {
    // Кроп — это кадрирование опорной камеры: дальше её дальности он невозможен.
    let reference_reach = lenses
        .iter()
        .find(|lens| lens.role == reference)
        .map_or(1.0, |lens| lens.max_ratio().value().min(range.max.value()));
    config
        .digital_crop_stops
        .iter()
        .copied()
        .filter(|stop| *stop > 1.0 && *stop <= reference_reach)
        .filter(|stop| {
            !lenses
                .iter()
                .any(|lens| (lens.native_ratio.value() - stop).abs() / stop <= config.stop_dedupe_tolerance)
        })
        .filter_map(|stop| ZoomRatio::new(stop).ok())
        .map(|ratio| ZoomStop {
            ratio,
            kind: ZoomStopKind::DigitalCrop,
        })
        .collect()
}
