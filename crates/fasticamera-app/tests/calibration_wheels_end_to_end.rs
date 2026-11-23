//! Сквозной тест колеса Lift в разделе калибровки (14b2, пакет P4): НАСТОЯЩИЙ палец по кругу колеса → экран →
//! эффект → библиотека пресетов → команда рендеру → кадр видоискателя на настоящем бэкенде (RADV) → проход прибора
//! RGB parade на GPU. Доказательство — значения библиотеки и ПИКСЕЛИ/счётчики, а не вызовы.
//!
//! Как ведётся палец. Образцы касаний идут тем же каналом входа, что шлёт оболочка (`CalibrationRig::touch`):
//! `Began` в центре круга → по одному `Moved` на кадр равными шагами → `Ended`. Координаты круга спрашиваются у
//! экрана (`wheel_pad_placement`) — той же раскладкой, по которой он круг рисует.
//!
//! Про сюжет. Стенд камеры отдаёт ОДНОРОДНЫЙ кадр (одно значение ячейки на весь кадр, пространственных деталей нет),
//! поэтому «тени» и «света» — два прогона одного сценария на двух сюжетах: тёмном (сильно ниже серого экрана) и
//! светлом (сильно выше). Колесо Lift обязано сдвинуть тёмный кадр и не тронуть светлый (решение 191).
//!
//! Метрика прибора. У RGB parade плоскость 2 — канал B'. Попадание ячейки `(row, column)` значит «уровень
//! `levels − 1 − row`». Кадр однородный, поэтому все попадания плоскости лежат около одного уровня; «средний уровень
//! B'» = среднее взвешенное по счётчикам плоскости B. Он растёт, когда синего в кадре становится больше.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

#[allow(
    dead_code,
    reason = "стенд общий: каждый тестовый файл берёт своё подмножество шагов"
)]
mod calibration_rig;

use calibration_rig::{
    AcceptingPersistence, CalibrationRig, cpu_scope_counters, gpu_scope_counters, library_over, live_scope_request,
};
use camera_screen::calibration::ControlViewKind;
use camera_screen::modes::CaptureMode;
use camera_screen::{ScreenAction, WheelPadPlacement};
use fasticamera::camera_screen_input::InputDelivery;
use look_contract::{ColorWheel, LookControl, LookSettings, WheelPart, WheelPush, WheelZone};
use render_loop::config::ToneBaseChoice;
use scope_contract::ScopeKind;
use scope_model::ScopeCounters;
use shell_contract::touch::TouchPhase;
use ui_core::gesture::PanSpec;
use viewfinder_rig::{BLACK_LEVEL, CellValues, WHITE_LEVEL, scene_neutral};

/// Минимальное расхождение эталонов, при котором тест различает «изменилось» от «нет» (как в соседнем файле).
const MIN_REFERENCE_GAP: f32 = 8.0 * viewfinder_rig::TOLERANCE;
/// Куда ведём палец: оттенок OKLab ≈ 265° — почти строго «вниз» глазами, синее.
const BLUE_HUE_DEGREES: f64 = 265.0;
/// Какую силу толчка хотим от протяжки (доля радиуса круга).
const WANTED_STRENGTH: f64 = 0.8;
/// Шаг пальца между кадрами, pt поверхности. Крупный (≈ 20 px за кадр — быстрый, но обычный взмах): стенд готовит
/// конечное число кадров камеры (`CAMERA_FRAMES`), а каждый кадр экрана показывает следующий кадр камеры, так что
/// длинная протяжка мелкими шагами исчерпала бы запас.
const FINGER_STEP_PT: f64 = 8.0;
/// Допуск угла толчка, градусы (шаг пальца и округление f32 дают доли градуса).
const ANGLE_TOLERANCE_DEGREES: f64 = 3.0;
/// Допуск силы толчка: ошибка распознавания пана в пределах одного шага (шаг · замедление / радиус) с запасом.
const STRENGTH_TOLERANCE: f64 = 0.05;
/// Пикселей на pt у стенда (экран 1080 px на 440 pt).
const PX_PER_PT: f64 = 1080.0 / 440.0;
/// Кадров на «осесть» после действия или касания: вход → эффект → библиотека → факты → кадр → прибор.
const SETTLE_FRAMES: usize = 4;

/// Сюжет однородного кадра с долей `fraction` от размаха сигнала над чёрным, нейтрального по балансу белого стенда
/// (серый без цветового крена: цвет в кадре дают только колёса).
fn neutral_scene(fraction: f64) -> CellValues {
    let neutral = scene_neutral();
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL) * fraction;
    let channel = |weight: f64| BLACK_LEVEL + (span * weight).round() as u16;
    CellValues {
        red: channel(neutral[0]),
        green: channel(neutral[1]),
        blue: channel(neutral[2]),
    }
}

/// Тени: сигнал 2 % размаха — кадр сильно темнее серого экрана (на 8 % размаха Lift уже не действует: там серый).
fn shadows_scene() -> CellValues {
    neutral_scene(0.02)
}

/// Света: сигнал 90 % размаха — кадр сильно светлее серого экрана.
fn highlights_scene() -> CellValues {
    neutral_scene(0.9)
}

/// Средний уровень B' (см. шапку файла): среднее взвешенное по счётчикам плоскости B parade.
fn mean_blue_level(counters: &ScopeCounters) -> f64 {
    let shape = counters.shape();
    let (mut weighted_levels, mut hits) = (0.0_f64, 0.0_f64);
    for row in 0..shape.rows() {
        let level = f64::from(shape.rows() - 1 - row);
        for column in 0..shape.columns() {
            let count = f64::from(counters.count(2, row, column).expect("ячейка в сетке"));
            weighted_levels += level * count;
            hits += count;
        }
    }
    assert!(hits > 0.0, "в плоскости B есть попадания");
    weighted_levels / hits
}

/// Протяжка пальца: направление «глазами» и длина пути ПОСЛЕ порога распознавания пана, pt.
#[derive(Debug, Clone, Copy)]
struct Drag {
    eye_degrees: f64,
    path_after_recognition_pt: f64,
}

/// Стенд раздела калибровки в виде колёс на колесе Lift, с прибором RGB parade и осевшими кадрами. `None` — GPU нет.
fn wheels_rig(scene: CellValues) -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), scene)?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.send(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.send(ScreenAction::WheelSelected(WheelZone::Lift));
    rig.send(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));
    rig.frames(SETTLE_FRAMES);
    Some(rig)
}

/// Ведёт настоящий палец от центра круга по `drag`: `Began`, по одному `Moved` на кадр, `Ended`.
/// Возвращает силу толчка, которую обязан дать экран по формуле решения 206:
/// `(путь − путь до распознавания) · замедление / радиус области толчка`, где пан распознаётся на первом шаге,
/// чей путь строго больше порога `hysteresis_pt` (до него палец точку не двигал).
fn drag_finger(rig: &mut CalibrationRig, drag: Drag) -> f64 {
    let placement: WheelPadPlacement = rig
        .source
        .wheel_pad_placement()
        .expect("вид колёс открыт, круг помещается в колонку");
    let hysteresis_pt = PanSpec::default().hysteresis_pt;
    let drive_ratio = ui_kit::tokens::UiKitTokens::default().disc_pad.drive_ratio;
    let recognized_steps = (hysteresis_pt / FINGER_STEP_PT).floor() + 1.0;
    let recognized_at_pt = recognized_steps * FINGER_STEP_PT;
    let total_steps = ((recognized_at_pt + drag.path_after_recognition_pt) / FINGER_STEP_PT)
        .ceil()
        .max(recognized_steps);
    let travelled_pt = total_steps * FINGER_STEP_PT;

    // Портрет: оси глаза = оси поверхности, y поверхности растёт вниз, а «вверх глазами» = +b.
    let (sin, cos) = drag.eye_degrees.to_radians().sin_cos();
    let center = placement.circle.center();
    let at = |distance_pt: f64| {
        (
            ((center.x + cos * distance_pt) * PX_PER_PT) as f32,
            ((center.y - sin * distance_pt) * PX_PER_PT) as f32,
        )
    };
    let (x_px, y_px) = at(0.0);
    assert_eq!(rig.touch(TouchPhase::Began, x_px, y_px), InputDelivery::Queued);
    rig.frame();
    for step in 1..=(total_steps as u32) {
        let (x_px, y_px) = at(f64::from(step) * FINGER_STEP_PT);
        assert_eq!(rig.touch(TouchPhase::Moved, x_px, y_px), InputDelivery::Queued);
        rig.frame();
    }
    let (x_px, y_px) = at(travelled_pt);
    assert_eq!(rig.touch(TouchPhase::Ended, x_px, y_px), InputDelivery::Queued);
    rig.frames(SETTLE_FRAMES);

    (travelled_pt - recognized_at_pt) * drive_ratio / placement.pad_radius_pt
}

/// Путь пальца после порога, при котором сила толчка выйдет равной `strength` (обратная формула решения 206).
fn path_for_strength(rig: &CalibrationRig, strength: f64) -> f64 {
    let placement = rig.source.wheel_pad_placement().expect("круг помещается");
    let drive_ratio = ui_kit::tokens::UiKitTokens::default().disc_pad.drive_ratio;
    strength * placement.pad_radius_pt / drive_ratio
}

/// Угол толчка по формуле OKLab: 0° = +a, 90° = +b, против часовой, [0, 360).
fn push_degrees(push: WheelPush) -> f64 {
    f64::from(push.b())
        .atan2(f64::from(push.a()))
        .to_degrees()
        .rem_euclid(360.0)
}

/// Значения библиотеки, в которых изменён ТОЛЬКО толчок колеса Lift относительно `preset`.
fn with_lift_push(preset: &LookSettings, push: WheelPush) -> LookSettings {
    let lift = ColorWheel {
        push,
        ..preset.wheels.lift
    };
    LookSettings {
        wheels: preset.wheels.with_wheel(WheelZone::Lift, lift),
        ..preset.clone()
    }
}

/// Итог прогона сценария «потянул точку → сбросил» на одном сюжете.
struct LiftJourney {
    /// Значения активного пресета до касаний.
    preset: LookSettings,
    /// Толчок Lift, как его держит библиотека после протяжки.
    pushed: WheelPush,
    /// Сила, которую обязан был дать экран по формуле.
    expected_strength: f64,
    /// Счётчики parade на нейтральном кадре.
    neutral: ScopeCounters,
    /// Счётчики parade после протяжки.
    after_drag: ScopeCounters,
    /// Расхождение эталонов «пресет / вид с толчком» — различим ли толчок на этом сюжете.
    reference_gap: f32,
}

/// Весь сценарий на сюжете `scene`; общие проверки (библиотека, кадр = эталон, parade GPU = CPU, сброс «↻») внутри.
fn run_lift_journey(scene: CellValues) -> Option<LiftJourney> {
    let mut rig = wheels_rig(scene)?;
    let preset = rig.library.active_look().preset_recipe.settings;
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "до касания: вид пресета");
    let request = live_scope_request(&mut rig);
    assert_eq!(request.kind, ScopeKind::RgbParade, "рендер держит запрос парада");
    let neutral = gpu_scope_counters(&rig);
    assert_eq!(
        neutral,
        cpu_scope_counters(&rig, &request),
        "нейтраль: GPU = эталон CPU"
    );

    // Настоящий палец тянет точку из центра в сторону синего.
    let drag = Drag {
        eye_degrees: BLUE_HUE_DEGREES,
        path_after_recognition_pt: path_for_strength(&rig, WANTED_STRENGTH),
    };
    let expected_strength = drag_finger(&mut rig, drag);

    // Библиотека: толчок Lift по углу и силе, всё остальное как в пресете.
    let active = rig.library.active_settings();
    let pushed = active.wheels.lift.push;
    assert!(
        (push_degrees(pushed) - BLUE_HUE_DEGREES).abs() < ANGLE_TOLERANCE_DEGREES,
        "угол толчка {}° вместо {BLUE_HUE_DEGREES}°",
        push_degrees(pushed)
    );
    assert!(
        (f64::from(pushed.strength()) - expected_strength).abs() < STRENGTH_TOLERANCE,
        "сила толчка {} вместо {expected_strength}",
        pushed.strength()
    );
    assert!(
        (expected_strength - WANTED_STRENGTH).abs() < STRENGTH_TOLERANCE,
        "протяжка подобрана под 0,8"
    );
    assert_eq!(active, with_lift_push(&preset, pushed), "тронут только толчок Lift");
    let facts = rig.screen().look_facts().expect("факты о виде дошли до экрана");
    assert!(
        facts.control_differs(&LookControl::Wheel {
            zone: WheelZone::Lift,
            part: WheelPart::Push,
        }),
        "экран видит, что толчок Lift отличается от пресета"
    );

    // Кадр видоискателя = эталон isp-model для вида библиотеки; parade GPU = эталон CPU показанного кадра.
    let reference_gap = rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &active));
    rig.assert_shows(ToneBaseChoice::Agx, &active, "после протяжки: вид с толчком Lift");
    let after_drag = gpu_scope_counters(&rig);
    let request_after_drag = live_scope_request(&mut rig);
    assert_eq!(
        after_drag,
        cpu_scope_counters(&rig, &request_after_drag),
        "после протяжки: GPU = эталон CPU показанного кадра"
    );

    // «↻» толчка: вид снова пресета, parade снова как нейтраль.
    rig.send(ScreenAction::ResetControlTapped(LookControl::Wheel {
        zone: WheelZone::Lift,
        part: WheelPart::Push,
    }));
    rig.frames(SETTLE_FRAMES);
    assert_eq!(rig.library.active_settings(), preset, "сброс вернул значения пресета");
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "после «↻» толчка: вид пресета");
    assert_eq!(gpu_scope_counters(&rig), neutral, "после «↻» parade снова как нейтраль");
    rig.finish();

    Some(LiftJourney {
        preset,
        pushed,
        expected_strength,
        neutral,
        after_drag,
        reference_gap,
    })
}

#[test]
fn a_finger_pull_toward_blue_tints_the_shadows_through_the_library_the_frame_and_the_parade_and_the_reset_undoes_it() {
    let Some(journey) = run_lift_journey(shadows_scene()) else {
        return;
    };
    assert!(
        journey.reference_gap > MIN_REFERENCE_GAP,
        "в тенях толчок виден в эталоне ({}): тест что-то доказывает",
        journey.reference_gap
    );
    let (before, after) = (mean_blue_level(&journey.neutral), mean_blue_level(&journey.after_drag));
    assert!(
        after > before + 1.0,
        "в тенях след B' поднялся: средний уровень {before:.2} → {after:.2}"
    );
    assert_ne!(journey.neutral, journey.after_drag, "счётчики parade сдвинулись");
    assert_eq!(
        journey.preset.wheels.lift.push,
        WheelPush::NEUTRAL,
        "у пресета толчка нет"
    );
    assert!(journey.expected_strength > 0.0);
    assert!(journey.pushed.strength() > 0.0);
}

#[test]
fn the_same_pull_leaves_a_frame_brighter_than_the_screen_gray_exactly_as_the_neutral_one() {
    // Решение 191: колесо теней выше серого экрана действует ровно 0 — кадр и parade те же, что у нейтрали.
    let Some(journey) = run_lift_journey(highlights_scene()) else {
        return;
    };
    assert!(
        journey.reference_gap < viewfinder_rig::TOLERANCE,
        "в светах Lift эталон не меняет ({})",
        journey.reference_gap
    );
    let (before, after) = (mean_blue_level(&journey.neutral), mean_blue_level(&journey.after_drag));
    assert!(
        (after - before).abs() < 0.5,
        "в светах след B' не сдвинулся: {before:.2} → {after:.2}"
    );
}

#[test]
fn a_finger_that_leaves_the_circle_puts_the_point_on_the_edge_in_the_same_direction() {
    let Some(mut rig) = wheels_rig(shadows_scene()) else {
        return;
    };
    // Путь в полтора раза длиннее радиуса: без прижатия сила вышла бы около 1,6, а палец ушёл бы далеко за круг.
    let drag = Drag {
        eye_degrees: BLUE_HUE_DEGREES,
        path_after_recognition_pt: path_for_strength(&rig, 1.6),
    };
    let unclamped = drag_finger(&mut rig, drag);
    assert!(
        unclamped > 1.3,
        "палец действительно ушёл за край: сырая сила {unclamped}"
    );

    let pushed = rig.library.active_settings().wheels.lift.push;
    assert!(
        (f64::from(pushed.strength()) - 1.0).abs() < 1e-3,
        "за краем точка на краю: сила {}",
        pushed.strength()
    );
    assert!(
        (push_degrees(pushed) - BLUE_HUE_DEGREES).abs() < ANGLE_TOLERANCE_DEGREES,
        "угол тот же: {}°",
        push_degrees(pushed)
    );
    let active = rig.library.active_settings();
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &active,
        "на краю круга: кадр = эталон вида библиотеки",
    );
    rig.finish();
}
