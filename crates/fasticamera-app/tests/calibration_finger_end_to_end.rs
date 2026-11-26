//! Сквозные тесты «палец → пиксели» для линеек раздела калибровки (14l, пункт 5): НАСТОЯЩЕЕ касание в пикселях
//! поверхности → экран → эффект → библиотека пресетов → команда рендеру → кадр видоискателя на настоящем бэкенде
//! (RADV) → пиксели против эталона `isp-model`. Доказательство — значения библиотеки, подписи на экране и ПИКСЕЛИ.
//!
//! Как ведётся палец. Образцы идут тем же каналом входа, что шлёт оболочка (`CalibrationRig::touch`). Точки
//! (линейки строк, «↻», точки полос, окно списка) спрашиваются у экрана (`calibration_touch_targets`) — у тех же
//! узлов, которые ловят касание, поэтому цель не может разойтись с нарисованным. Протяжка линейки: `Began` →
//! по одному `Moved` на кадр → задержка на месте (скорость отпускания нулевая, инерции нет) → `Ended`.
//! Значение считается от точки касания: влево — больше, pt на единицу шкалы и число единиц берутся из конфига
//! экрана (`knob_full_travel_pt` / (2·`knob_units_per_full_scale`): −200…+200 на 400 pt, решение 251), а не из
//! литералов.
//!
//! Что доказывается (у каждого теста сначала проверяется, что эталон ВООБЩЕ отличается от нейтрали на этой
//! сцене, иначе «кадр совпал» ничего бы не значил):
//! * тон — CONTRAST до +200, HIGHLIGHTS до −200, SHADOWS до +200 на AgX и на legacy (тени на legacy раньше не
//!   работали): значение в библиотеке бит-в-бит, кадр = эталон, кадр ≠ нейтрали, «↻» возвращает пресет;
//! * шаг 1 — палец ставит SATURATION ровно +30 (`0.15` бит в бит, подпись «+30»);
//! * полоса BANDS — точка полосы выбрана пальцем, ось HUE, SAT, LUM доходит до пикселей;
//! * кольцо яркости колеса Gamma до +200.
//!
//! Метрика: расхождение эталонов «пресет / вид после протяжки» (`reference_gap`, максимум по каналам, доли выхода).

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

use calibration_rig::{AcceptingPersistence, CalibrationRig, library_over};
use camera_screen::ScreenAction;
use camera_screen::calibration::{CalibrationConfig, ControlViewKind};
use camera_screen::modes::CaptureMode;
use fasticamera::camera_screen_input::InputDelivery;
use look_contract::{
    BandAxis, GlobalKnob, HueBandId, KnobSideStops, KnobStop, KnobTravelStops, LookControl, LookKnob, LookSettings,
    LookWhiteStops, StopFraction, ToneBaseModel, WheelPart, WheelZone,
};
use render_loop::config::ToneBaseChoice;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use viewfinder_rig::{BLACK_LEVEL, CellValues, WHITE_LEVEL, reddish, scene_neutral};

/// Пикселей на pt у стенда (экран 1080 px на 440 pt).
const PX_PER_PT: f64 = 1080.0 / 440.0;
/// Шагов шкалы на значение 1,0 (конфиг раздела по умолчанию, решения 234, 251): край шкалы — ±это число.
const UNITS_PER_FULL_SCALE: i32 = 200;
/// Путь пальца на единицу шкалы, pt: из конфига экрана (решение 251), а не литерал.
fn points_per_unit() -> f64 {
    let widgets = CalibrationConfig::default().widgets;
    assert_eq!(
        widgets.knob_units_per_full_scale, UNITS_PER_FULL_SCALE as u32,
        "тест написан под шкалу из конфига"
    );
    widgets.knob_points_per_unit()
}
/// Шаг пальца между кадрами, pt: крупный, но обычный взмах (кадры камеры стенда конечны).
const FINGER_STEP_PT: f64 = 8.0;
/// Кадров задержки пальца на месте перед отпусканием: скорость отпускания нулевая, инерция не уносит значение.
const HOLD_FRAMES: usize = 8;
/// Кадров на «осесть» после касания: вход → эффект → библиотека → факты → кадр.
const SETTLE_FRAMES: usize = 4;
/// Самый длинный путь пальца за одну протяжку, pt: помещается в линейку с запасом по обе стороны от центра.
const MAX_CHUNK_PT: f64 = 100.0;

/// Самая длинная протяжка за один раз, единиц шкалы (`MAX_CHUNK_PT` пальца при текущей плотности).
fn max_chunk_units() -> i32 {
    (MAX_CHUNK_PT / points_per_unit()) as i32
}
/// Запас «за край» при выводе на ±200, единиц: палец идёт дальше края, значение прижимается к нему.
const EDGE_OVERSHOOT_UNITS: i32 = 10;
/// Сколько протяжек и листаний разрешено на одну цель, прежде чем тест признаёт, что палец не доходит.
const MAX_ATTEMPTS: usize = 10;
/// Расхождение эталонов, при котором тест различает «изменилось» от «нет» (8 допусков сравнения пикселей).
const MIN_REFERENCE_GAP: f32 = 8.0 * viewfinder_rig::TOLERANCE;
/// Порог силы тон-ручки на краю (доли выхода): край обязан менять кадр заметно, а не «в пределах допуска».
const MIN_TONE_EDGE_GAP: f32 = 0.04;

/// Сюжет однородного кадра с долей `fraction` от размаха сигнала над чёрным, нейтральный по балансу белого стенда.
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

// ----- палец ---------------------------------------------------------------------------------------------------------

/// Касание в pt поверхности; кадр после каждого образца, как идёт настоящий жест.
fn touch(rig: &mut CalibrationRig, phase: TouchPhase, point: Point) {
    let delivery = rig.touch(phase, (point.x * PX_PER_PT) as f32, (point.y * PX_PER_PT) as f32);
    assert_eq!(delivery, InputDelivery::Queued, "касание {phase:?} не принято каналом");
    rig.frame();
}

/// Тап пальцем в точку.
fn tap(rig: &mut CalibrationRig, point: Point) {
    touch(rig, TouchPhase::Began, point);
    touch(rig, TouchPhase::Ended, point);
    rig.frames(SETTLE_FRAMES);
}

/// Протяжка пальца из `from` на `(dx, dy)` pt: равные шаги, задержка на месте, отпускание, осесть.
fn drag(rig: &mut CalibrationRig, from: Point, (dx, dy): (f64, f64)) {
    touch(rig, TouchPhase::Began, from);
    let steps = (dx.abs().max(dy.abs()) / FINGER_STEP_PT).ceil().max(1.0) as u32;
    let at = |step: u32| {
        let t = f64::from(step) / f64::from(steps);
        Point::new(from.x + dx * t, from.y + dy * t)
    };
    for step in 1..=steps {
        touch(rig, TouchPhase::Moved, at(step));
    }
    for _ in 0..HOLD_FRAMES {
        touch(rig, TouchPhase::Moved, at(steps));
    }
    touch(rig, TouchPhase::Ended, at(steps));
    rig.frames(SETTLE_FRAMES);
}

/// Цели касаний раздела по текущей сцене.
fn targets(rig: &CalibrationRig) -> camera_screen::CalibrationTouchTargets {
    rig.source
        .calibration_touch_targets()
        .expect("раздел калибровки на экране")
}

/// Центр точки из списка целей; номера вне списка — провал теста, а не тихий пропуск.
fn nth(points: &[Point], slot: usize, what: &str) -> Point {
    points
        .get(slot)
        .copied()
        .unwrap_or_else(|| panic!("{what} №{slot}: в сцене только {}", points.len()))
}

/// Листает список ручек пальцем, пока линейка строки `slot` не окажется в видимой части колонки, и возвращает её
/// центр. Строка вне окна пальцем не берётся, поэтому её доводят вертикальной протяжкой, а не берут «из воздуха».
fn bring_row_into_view(rig: &mut CalibrationRig, slot: usize) -> Point {
    for _ in 0..MAX_ATTEMPTS {
        let current = targets(rig);
        let ruler = nth(&current.rulers, slot, "линейка");
        // Верх окна — под шапкой и переключателем вида; низ — выше нижнего края колонки.
        let visible = ruler.y >= current.knobs_zone.min_y() + 140.0 && ruler.y <= current.knobs_zone.max_y() - 60.0;
        if visible {
            return ruler;
        }
        let to_window = (ruler.y - current.list_window_center.y).clamp(-300.0, 300.0);
        drag(rig, current.list_window_center, (0.0, -to_window));
    }
    panic!("строка {slot} не доехала до видимой части окна за {MAX_ATTEMPTS} протяжек");
}

/// Целое число шкалы по значению ручки.
fn units_of(value: LookKnob) -> i32 {
    (value.get() * UNITS_PER_FULL_SCALE as f32).round() as i32
}

/// Значение ручки РОВНО `units / 200` в `f32`, как литерал пресета.
fn knob_of_units(units: i32) -> LookKnob {
    LookKnob::new(units as f32 / UNITS_PER_FULL_SCALE as f32).expect("значение шкалы в диапазоне")
}

/// Ведёт линейку строки `slot` пальцем до `target_units`, читая значение у библиотеки (`read`), как это делает
/// владелец: протяжка за протяжкой, пока число не станет нужным. На краю ±200 палец идёт дальше края.
fn finger_to_units(rig: &mut CalibrationRig, slot: usize, read: impl Fn(&LookSettings) -> LookKnob, target_units: i32) {
    for _ in 0..MAX_ATTEMPTS {
        let current_units = units_of(read(&rig.library.active_settings()));
        if current_units == target_units {
            return;
        }
        let remaining = target_units - current_units;
        let mut chunk = remaining.clamp(-max_chunk_units(), max_chunk_units());
        if chunk == remaining && target_units.abs() == UNITS_PER_FULL_SCALE {
            chunk += chunk.signum() * EDGE_OVERSHOOT_UNITS;
        }
        let travel_pt = f64::from(chunk) * points_per_unit();
        let ruler = bring_row_into_view(rig, slot);
        // Влево значение растёт: начинаем правее центра на полпути и кончаем левее на столько же.
        drag(rig, Point::new(ruler.x + travel_pt / 2.0, ruler.y), (-travel_pt, 0.0));
    }
    panic!(
        "палец не довёл строку {slot} до {target_units:+}: значение {:+}",
        units_of(read(&rig.library.active_settings()))
    );
}

/// Стенд: библиотека с пресетами по умолчанию, раздел калибровки открыт, основа `base` выбрана.
fn rig_on(base: ToneBaseModel, scene: CellValues) -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), scene)?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    rig.send(ScreenAction::ToneBaseSelected(base));
    rig.frames(SETTLE_FRAMES);
    assert_eq!(rig.library.active_look().recipe.base, base, "основа выбрана");
    Some(rig)
}

/// Основа конфига рендера для основы словаря вида.
fn choice_of(base: ToneBaseModel) -> ToneBaseChoice {
    match base {
        ToneBaseModel::Agx => ToneBaseChoice::Agx,
        ToneBaseModel::Aces => ToneBaseChoice::Aces,
        ToneBaseModel::Filmic => ToneBaseChoice::Filmic,
        ToneBaseModel::Legacy => ToneBaseChoice::Legacy,
    }
}

/// Номер строки глобальной ручки в виде шкал.
fn knob_slot(knob: GlobalKnob) -> usize {
    GlobalKnob::ALL
        .iter()
        .position(|candidate| *candidate == knob)
        .expect("ручка есть в списке")
}

/// Номер строки оси полосы: после восьми ручек, в порядке `BandAxis::ALL`.
fn band_axis_slot(axis: BandAxis) -> usize {
    GlobalKnob::ALL.len()
        + BandAxis::ALL
            .iter()
            .position(|candidate| *candidate == axis)
            .expect("ось есть в списке")
}

/// Общий сценарий: палец ведёт ручку тона/цвета `knob` до `target_units`; кадр = эталон, ≠ нейтрали, «↻» возвращает.
fn run_knob_journey(base: ToneBaseModel, knob: GlobalKnob, target_units: i32, scene: CellValues, min_gap: f32) {
    let Some(mut rig) = rig_on(base, scene) else {
        return;
    };
    let choice = choice_of(base);
    let preset = rig.library.active_look().preset_recipe.settings;
    rig.assert_shows(choice, &preset, "до касания: вид пресета");

    finger_to_units(&mut rig, knob_slot(knob), |settings| settings.knob(knob), target_units);

    let active = rig.library.active_settings();
    assert_eq!(
        active.knob(knob).get().to_bits(),
        knob_of_units(target_units).get().to_bits(),
        "{knob:?}: значение библиотеки ровно {target_units:+}/200"
    );
    assert_eq!(
        active,
        preset.clone().with_knob(knob, knob_of_units(target_units)),
        "тронута только {knob:?}"
    );
    let gap = rig.reference_gap((choice, &preset), (choice, &active));
    assert!(
        gap > min_gap,
        "{base:?} {knob:?} {target_units:+}: край меняет эталон на {gap}, нужно больше {min_gap}"
    );
    assert!(
        rig.screen()
            .look_facts()
            .expect("факты о виде дошли до экрана")
            .control_differs(&LookControl::Knob(knob)),
        "экран видит, что {knob:?} отличается от пресета"
    );
    rig.assert_shows(choice, &active, "после протяжки: вид с правкой, ровно эталон CPU");

    // «↻» строки (настоящий тап): значение пресета, кадр прежний.
    let reset = nth(&targets(&rig).resets, knob_slot(knob), "«↻»");
    tap(&mut rig, reset);
    assert_eq!(rig.library.active_settings(), preset, "«↻» вернул значения пресета");
    rig.assert_shows(choice, &preset, "после «↻»: вид пресета");
    rig.finish();
}

// ----- тон: CONTRAST, HIGHLIGHTS, SHADOWS -----------------------------------------------------------------------------

#[test]
fn finger_drives_contrast_to_plus_200_on_agx() {
    run_knob_journey(
        ToneBaseModel::Agx,
        GlobalKnob::Contrast,
        UNITS_PER_FULL_SCALE,
        neutral_scene(0.15),
        MIN_TONE_EDGE_GAP,
    );
}

#[test]
fn finger_drives_contrast_to_plus_200_on_legacy() {
    run_knob_journey(
        ToneBaseModel::Legacy,
        GlobalKnob::Contrast,
        UNITS_PER_FULL_SCALE,
        neutral_scene(0.15),
        MIN_TONE_EDGE_GAP,
    );
}

#[test]
fn finger_drives_highlights_to_minus_200_on_agx() {
    run_knob_journey(
        ToneBaseModel::Agx,
        GlobalKnob::Highlights,
        -UNITS_PER_FULL_SCALE,
        neutral_scene(0.9),
        MIN_TONE_EDGE_GAP,
    );
}

#[test]
fn finger_drives_highlights_to_minus_200_on_legacy() {
    run_knob_journey(
        ToneBaseModel::Legacy,
        GlobalKnob::Highlights,
        -UNITS_PER_FULL_SCALE,
        neutral_scene(0.9),
        MIN_TONE_EDGE_GAP,
    );
}

#[test]
fn finger_drives_shadows_to_plus_200_on_agx() {
    run_knob_journey(
        ToneBaseModel::Agx,
        GlobalKnob::Shadows,
        UNITS_PER_FULL_SCALE,
        neutral_scene(0.02),
        MIN_TONE_EDGE_GAP,
    );
}

#[test]
fn finger_drives_shadows_to_plus_200_on_legacy_where_they_did_not_work_before() {
    run_knob_journey(
        ToneBaseModel::Legacy,
        GlobalKnob::Shadows,
        UNITS_PER_FULL_SCALE,
        neutral_scene(0.02),
        MIN_TONE_EDGE_GAP,
    );
}

// ----- шаг 1: палец ставит ровно +30 (0,15) -----------------------------------------------------------------------------------

#[test]
fn finger_sets_saturation_to_exactly_plus_30_and_the_screen_says_plus_30() {
    let Some(mut rig) = rig_on(ToneBaseModel::Agx, reddish()) else {
        return;
    };
    let preset = rig.library.active_look().preset_recipe.settings;
    finger_to_units(
        &mut rig,
        knob_slot(GlobalKnob::Saturation),
        |s| s.knob(GlobalKnob::Saturation),
        30,
    );

    let active = rig.library.active_settings();
    assert_eq!(
        active.knob(GlobalKnob::Saturation).get().to_bits(),
        0.15_f32.to_bits(),
        "значение в библиотеке — литерал 0.15 бит в бит"
    );
    assert_eq!(
        rig.drawn_scale_value_count("+30"),
        1,
        "подпись строки SATURATION — «+30»"
    );
    assert!(rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &active)) > MIN_REFERENCE_GAP);
    rig.assert_shows(ToneBaseChoice::Agx, &active, "после +30: кадр = эталон");
    rig.finish();
}

// ----- полоса BANDS: выбрать точку и провести HUE, SAT, LUM ------------------------------------------------------------

/// Выбирает пальцем (тап по точке) полосу `band` и ведёт ось `axis` до `target_units`; кадр = эталон и ≠ пресета.
fn run_band_journey(axis: BandAxis, target_units: i32) {
    let Some(mut rig) = rig_on(ToneBaseModel::Agx, reddish()) else {
        return;
    };
    let band = HueBandId::new("red").expect("идентификатор полосы");
    let preset = rig.library.active_look().preset_recipe.settings;
    let dot_index = rig
        .screen()
        .look_facts()
        .expect("факты о виде")
        .bands
        .iter()
        .position(|candidate| candidate.id == band)
        .expect("полоса red есть в модели вида");
    let dot = nth(&targets(&rig).band_dots, dot_index, "точка полосы");
    tap(&mut rig, dot);
    assert_eq!(
        rig.screen().calibration().selected_band(),
        Some(&band),
        "тап по точке выбрал полосу red"
    );

    let read = |settings: &LookSettings| settings.bands.for_band(&band).axis(axis);
    finger_to_units(&mut rig, band_axis_slot(axis), read, target_units);

    let active = rig.library.active_settings();
    assert_eq!(
        read(&active).get().to_bits(),
        knob_of_units(target_units).get().to_bits(),
        "{axis:?}: значение полосы ровно {target_units:+}/200"
    );
    let only_this_axis = LookSettings {
        bands: preset.bands.with_adjustment(
            preset
                .bands
                .for_band(&band)
                .with_axis(axis, knob_of_units(target_units)),
        ),
        ..preset.clone()
    };
    assert_eq!(active, only_this_axis, "тронута только ось {axis:?} полосы red");
    let gap = rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &active));
    assert!(
        gap > MIN_REFERENCE_GAP,
        "{axis:?} {target_units:+}: ось доходит до пикселей, расхождение {gap}"
    );
    rig.assert_shows(ToneBaseChoice::Agx, &active, "после протяжки оси полосы: кадр = эталон");
    rig.finish();
}

#[test]
fn finger_drives_the_hue_axis_of_a_band_to_the_pixels() {
    run_band_journey(BandAxis::Hue, UNITS_PER_FULL_SCALE);
}

#[test]
fn finger_drives_the_saturation_axis_of_a_band_to_the_pixels() {
    run_band_journey(BandAxis::Saturation, UNITS_PER_FULL_SCALE);
}

#[test]
fn finger_drives_the_luminance_axis_of_a_band_to_the_pixels() {
    run_band_journey(BandAxis::Luminance, UNITS_PER_FULL_SCALE);
}

// ----- кольцо яркости колеса ---------------------------------------------------------------------------------------------

#[test]
fn finger_drives_the_gamma_wheel_ring_to_plus_200_and_the_frame_matches_the_reference() {
    let Some(mut rig) = rig_on(ToneBaseModel::Agx, neutral_scene(0.1)) else {
        return;
    };
    rig.send(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.send(ScreenAction::WheelSelected(WheelZone::Gamma));
    rig.frames(SETTLE_FRAMES);
    let preset = rig.library.active_look().preset_recipe.settings;

    // В виде колёс одна строка с линейкой — кольцо яркости выбранного колеса.
    finger_to_units(
        &mut rig,
        0,
        |settings| settings.wheels.wheel(WheelZone::Gamma).ring,
        UNITS_PER_FULL_SCALE,
    );

    let active = rig.library.active_settings();
    assert_eq!(
        active.wheels.wheel(WheelZone::Gamma).ring.get().to_bits(),
        1.0_f32.to_bits(),
        "кольцо Gamma ровно на краю"
    );
    assert_eq!(
        active.wheels.wheel(WheelZone::Gamma).push,
        preset.wheels.wheel(WheelZone::Gamma).push,
        "толчок колеса не тронут"
    );
    let gap = rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &active));
    assert!(gap > MIN_REFERENCE_GAP, "кольцо доходит до пикселей, расхождение {gap}");
    assert!(
        rig.screen()
            .look_facts()
            .expect("факты о виде")
            .control_differs(&LookControl::Wheel {
                zone: WheelZone::Gamma,
                part: WheelPart::Ring,
            }),
        "экран видит, что кольцо Gamma отличается от пресета"
    );
    rig.assert_shows(ToneBaseChoice::Agx, &active, "кольцо Gamma +200: кадр = эталон");

    // «↻» кольца (настоящий тап): значения пресета, кадр прежний.
    let reset = nth(&targets(&rig).resets, 0, "«↻» кольца");
    tap(&mut rig, reset);
    assert_eq!(rig.library.active_settings(), preset, "«↻» вернул пресет");
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "после «↻»: вид пресета");
    rig.finish();
}

// ----- упоры теплоты и оттенка (решения 288, 293) --------------------------------------------------------------------

#[test]
fn finger_drives_tint_past_the_render_stop_and_the_screen_marks_the_stop_and_dims_the_value() {
    let Some(mut rig) = rig_on(ToneBaseModel::Agx, reddish()) else {
        return;
    };
    let tint_slot = knob_slot(GlobalKnob::Tint);
    bring_row_into_view(&mut rig, tint_slot);
    rig.frames(20);
    assert_eq!(rig.drawn_stop_tick_count(), 0, "рендер упоров не сообщал: отметок нет");

    // Рендер положил упор оттенка 0,46 на доску (в продукте это делает поток рендера при смене входов).
    let none = KnobSideStops {
        minus: KnobStop::None,
        plus: KnobStop::None,
    };
    rig.look_white_stops.publish(LookWhiteStops::Known(KnobTravelStops {
        warmth: none,
        tint: KnobSideStops {
            plus: KnobStop::At(StopFraction::new(0.46).expect("доля")),
            ..none
        },
    }));
    rig.frames(20);
    assert_eq!(
        rig.drawn_stop_tick_count(),
        1,
        "упор оттенка нарисован риской на линейке TINT"
    );
    assert_eq!(
        rig.board.latest().expect("отчёт").look_white_stops_sent,
        1,
        "экрану сообщено один раз"
    );

    // Палец ведёт оттенок: до упора число обычное, за упором (+92) — тусклое.
    let normal = rig.drawn_scale_value_alpha("0").expect("число 0 нарисовано");
    finger_to_units(&mut rig, tint_slot, |settings| settings.tint, 50);
    assert_eq!(
        rig.drawn_scale_value_alpha("+50"),
        Some(normal),
        "+50 до упора: число обычное"
    );
    finger_to_units(&mut rig, tint_slot, |settings| settings.tint, 120);
    let dimmed = rig.drawn_scale_value_alpha("+120").expect("число +120 нарисовано");
    assert!(
        dimmed < normal,
        "+120 за упором: число тусклее ({dimmed} против {normal})"
    );

    // Поток закончился: доска вернулась к «неизвестно», отметка и тусклое число пропали.
    rig.look_white_stops.publish(LookWhiteStops::Unknown);
    rig.frames(20);
    assert_eq!(rig.drawn_stop_tick_count(), 0);
    assert_eq!(
        rig.drawn_scale_value_alpha("+120"),
        Some(normal),
        "без упоров число обычное"
    );
    rig.finish();
}
