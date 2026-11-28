//! Сквозной тест полосок BARS в разделе калибровки (14t, пакет P3): НАСТОЯЩИЙ палец по шкале канала → экран →
//! эффект `SetPrimary` → библиотека пресетов → команда рендеру (`SetLook` несёт полоски) → кадр видоискателя на
//! настоящем бэкенде (RADV) → проход прибора RGB parade на GPU. Доказательство — значения библиотеки и ПИКСЕЛИ/счётчики.
//!
//! Как ведётся палец. Образцы касаний идут тем же каналом входа, что шлёт оболочка (`CalibrationRig::touch`): `Began`
//! правее центра шкалы → по одному `Moved` на кадр равными шагами влево (влево — значение растёт) → `Ended`. Центр шкалы
//! спрашивается у раскладки раздела (`bars_ruler_placements`) — той же функцией, по которой экран шкалу рисует.
//!
//! Сюжет — однородный кадр 12 % размаха (≈ 0,62 кода экрана): Gain двигает его заметно, а поднятый канал ещё не
//! упирается в потолок кода (при 50 % и силах владельца 14t красный упирался в 511 из 512, и шаг выхода, сохраняя
//! оттенок пересвета, опускал соседние каналы — parade мерил бы обрезку, а не полоску).
//!
//! Метрика прибора. У RGB parade плоскости: 0 — R', 1 — G', 2 — B'. Попадание ячейки `(row, column)` значит «уровень
//! `levels − 1 − row`»; средний уровень плоскости — среднее взвешенное по счётчикам.

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
use camera_screen::ScreenAction;
use camera_screen::calibration::{CalibrationWidgetsConfig, ControlViewKind, bars_ruler_placements};
use camera_screen::modes::CaptureMode;
use fasticamera::camera_screen_input::InputDelivery;
use look_contract::{LookControl, LookSettings, PrimaryChannel, WheelZone};
use render_loop::config::ToneBaseChoice;
use scope_contract::ScopeKind;
use scope_model::ScopeCounters;
use shell_contract::touch::TouchPhase;
use viewfinder_rig::{BLACK_LEVEL, CellValues, WHITE_LEVEL, scene_neutral};

/// Минимальное расхождение эталонов, при котором тест различает «изменилось» от «нет».
const MIN_REFERENCE_GAP: f32 = 8.0 * viewfinder_rig::TOLERANCE;
/// Шаг пальца между кадрами, pt поверхности (как у колёс: стенд готовит конечное число кадров камеры).
const FINGER_STEP_PT: f64 = 8.0;
/// Шагов протяжки: 22 шага по 8 pt при 1 pt на единицу шкалы (−200…+200) — около +170 единиц, почти край хода.
const SWIPE_STEPS: u32 = 22;
/// Кадров неподвижного пальца перед отпусканием.
const HOLD_FRAMES: usize = 6;
/// Пикселей на pt у стенда (экран 1080 px на 440 pt).
const PX_PER_PT: f64 = 1080.0 / 440.0;
/// Кадров на «осесть» после действия или касания: вход → эффект → библиотека → факты → кадр → прибор.
const SETTLE_FRAMES: usize = 4;
/// Плоскость parade канала.
fn plane_of(channel: PrimaryChannel) -> u32 {
    match channel {
        PrimaryChannel::Red => 0,
        PrimaryChannel::Green => 1,
        PrimaryChannel::Blue => 2,
    }
}

/// Однородный сюжет 12 % размаха (см. шапку: поднятый канал не упирается в потолок), нейтральный по балансу белого стенда.
fn bright_scene() -> CellValues {
    let neutral = scene_neutral();
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL) * 0.12;
    let channel = |weight: f64| BLACK_LEVEL + (span * weight).round() as u16;
    CellValues {
        red: channel(neutral[0]),
        green: channel(neutral[1]),
        blue: channel(neutral[2]),
    }
}

/// Средний уровень плоскости `plane` parade (см. шапку файла).
fn mean_level(counters: &ScopeCounters, plane: u32) -> f64 {
    let shape = counters.shape();
    let (mut weighted_levels, mut hits) = (0.0_f64, 0.0_f64);
    for row in 0..shape.rows() {
        let level = f64::from(shape.rows() - 1 - row);
        for column in 0..shape.columns() {
            let count = f64::from(counters.count(plane, row, column).expect("ячейка в сетке"));
            weighted_levels += level * count;
            hits += count;
        }
    }
    assert!(hits > 0.0, "в плоскости {plane} есть попадания");
    weighted_levels / hits
}

/// Стенд раздела калибровки в виде BARS на вкладке `zone`, с прибором RGB parade и осевшими кадрами. `None` — GPU нет.
fn bars_rig(scene: CellValues, zone: WheelZone) -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), scene)?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.send(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    rig.send(ScreenAction::WheelSelected(zone));
    rig.send(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));
    rig.frames(SETTLE_FRAMES);
    Some(rig)
}

/// Ведёт настоящий палец по шкале `channel`: начало правее центра, `SWIPE_STEPS` шагов влево, отпускание.
fn swipe_scale(rig: &mut CalibrationRig, channel: PrimaryChannel) {
    let index = PrimaryChannel::ALL
        .iter()
        .position(|known| *known == channel)
        .expect("канал из списка");
    let layout = *rig
        .screen()
        .calibration()
        .layout()
        .expect("раздел калибровки открыт и размечен");
    let placements = bars_ruler_placements(&layout, &CalibrationWidgetsConfig::default())
        .expect("вид BARS открыт, шкалы помещаются в колонку");
    let center = placements[index].ruler_center;
    // Портрет: оси глаза = оси поверхности; палец стартует правее центра, чтобы путь влево остался внутри линейки.
    let start_x = center.x + FINGER_STEP_PT * f64::from(SWIPE_STEPS) / 2.0;
    let at = |step: u32| {
        (
            ((start_x - f64::from(step) * FINGER_STEP_PT) * PX_PER_PT) as f32,
            (center.y * PX_PER_PT) as f32,
        )
    };
    let (x_px, y_px) = at(0);
    assert_eq!(rig.touch(TouchPhase::Began, x_px, y_px), InputDelivery::Queued);
    rig.frame();
    for step in 1..=SWIPE_STEPS {
        let (x_px, y_px) = at(step);
        assert_eq!(rig.touch(TouchPhase::Moved, x_px, y_px), InputDelivery::Queued);
        rig.frame();
    }
    // Палец задерживается на месте (скорость отпускания нулевая): линейка встаёт под пальцем, а не катится по инерции.
    let (x_px, y_px) = at(SWIPE_STEPS);
    for _ in 0..HOLD_FRAMES {
        assert_eq!(rig.touch(TouchPhase::Moved, x_px, y_px), InputDelivery::Queued);
        rig.frame();
    }
    assert_eq!(rig.touch(TouchPhase::Ended, x_px, y_px), InputDelivery::Queued);
    rig.frames(SETTLE_FRAMES);
}

/// Значения, в которых относительно `preset` изменена ТОЛЬКО полоска `(zone, channel)`.
fn with_primary(
    preset: &LookSettings,
    zone: WheelZone,
    channel: PrimaryChannel,
    value: look_contract::LookKnob,
) -> LookSettings {
    LookSettings {
        primaries: preset.primaries.with_value(zone, channel, value),
        ..preset.clone()
    }
}

/// Итог сценария «палец по шкале канала вкладки GAIN → сброс «↻»» на светлом сюжете.
struct BarJourney {
    /// Счётчики parade на нейтральном кадре.
    neutral: ScopeCounters,
    /// Счётчики parade после протяжки.
    after_swipe: ScopeCounters,
    /// Расхождение эталонов «пресет / вид с полоской» — различима ли полоска на этом сюжете.
    reference_gap: f32,
}

fn run_gain_journey(channel: PrimaryChannel) -> Option<BarJourney> {
    let mut rig = bars_rig(bright_scene(), WheelZone::Gain)?;
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

    swipe_scale(&mut rig, channel);

    // Библиотека: изменена ровно одна полоска выбранной зоны, остальные 11 и всё прочее — как в пресете.
    let active = rig.library.active_settings();
    let value = active.primaries.value(WheelZone::Gain, channel);
    let units = (value.get() * 200.0).round() as i32;
    // Протяжка в `SWIPE_STEPS` шагов по `FINGER_STEP_PT` pt при 1 pt на единицу: до порога распознавания пана палец
    // значение не двигает, поэтому итог чуть меньше полного пути, но заведомо больше половины.
    let full_path_units = (f64::from(SWIPE_STEPS) * FINGER_STEP_PT) as i32;
    assert!(
        (full_path_units / 2..=full_path_units).contains(&units),
        "палец тянул ≤ {full_path_units} единиц, получилось {units}"
    );
    assert_eq!(
        active,
        with_primary(&preset, WheelZone::Gain, channel, value),
        "тронута только полоска Gain.{channel:?}"
    );
    let facts = rig.screen().look_facts().expect("факты о виде дошли до экрана");
    assert!(
        facts.control_differs(&LookControl::Primary {
            zone: WheelZone::Gain,
            channel
        }),
        "экран видит, что полоска отличается от пресета"
    );

    // Кадр видоискателя = эталон isp-model для вида библиотеки; parade GPU = эталон CPU показанного кадра.
    let reference_gap = rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &active));
    rig.assert_shows(ToneBaseChoice::Agx, &active, "после протяжки: вид с полоской");
    let after_swipe = gpu_scope_counters(&rig);
    let request_after = live_scope_request(&mut rig);
    assert_eq!(
        after_swipe,
        cpu_scope_counters(&rig, &request_after),
        "после протяжки: GPU = эталон CPU показанного кадра"
    );

    // «↻» полоски: вид снова пресета, parade снова как нейтраль.
    rig.send(ScreenAction::ResetControlTapped(LookControl::Primary {
        zone: WheelZone::Gain,
        channel,
    }));
    rig.frames(SETTLE_FRAMES);
    assert_eq!(rig.library.active_settings(), preset, "сброс вернул значения пресета");
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "после «↻»: вид пресета");
    assert_eq!(gpu_scope_counters(&rig), neutral, "после «↻» parade снова как нейтраль");
    rig.finish();

    Some(BarJourney {
        neutral,
        after_swipe,
        reference_gap,
    })
}

/// Следы двух других каналов не поднялись и сдвинулись меньше четверти сдвига поднятого: полоска двигает «свой» канал.
///
/// Небольшой спад соседей — не утечка полоски, а экран стенда: стенд рисует в sRGB (`render_config`), а R, G, B полосок —
/// основные цвета Display P3 (решение 342). Перевод P3 → sRGB даёт соседям отрицательный вклад (у R → G коэффициент
/// −0,042 в линейном свете), в кодах замерено ≈ 11 % (R +118 → G −13, B −10). На экране телефона (P3) утечки нет:
/// это доказывает `scope-model/tests/primaries_on_parade.rs` через настоящий шаг выхода в P3 (чужие следы 0,00).
fn assert_only_the_channel_rose(journey: &BarJourney, channel: PrimaryChannel) {
    let own_plane = plane_of(channel);
    let own_rise = mean_level(&journey.after_swipe, own_plane) - mean_level(&journey.neutral, own_plane);
    assert!(own_rise > 10.0, "след {channel:?} поднялся заметно: +{own_rise:.2}");
    for other in PrimaryChannel::ALL.into_iter().filter(|known| *known != channel) {
        let plane = plane_of(other);
        let shift = mean_level(&journey.after_swipe, plane) - mean_level(&journey.neutral, plane);
        assert!(shift <= 0.5, "след {other:?} не поднялся: {shift:+.2}");
        assert!(
            shift.abs() < own_rise / 4.0,
            "след {other:?} сдвинулся на {shift:+.2}, больше четверти сдвига {channel:?} (+{own_rise:.2})"
        );
    }
}

#[test]
fn a_finger_on_the_red_scale_of_gain_makes_the_highlights_redder_through_the_library_the_frame_and_the_red_parade_trace()
 {
    let Some(journey) = run_gain_journey(PrimaryChannel::Red) else {
        return;
    };
    assert!(
        journey.reference_gap > MIN_REFERENCE_GAP,
        "полоска видна в эталоне ({}): тест что-то доказывает",
        journey.reference_gap
    );
    assert_ne!(journey.neutral, journey.after_swipe, "счётчики parade сдвинулись");
    assert_only_the_channel_rose(&journey, PrimaryChannel::Red);
}

#[test]
fn a_finger_on_the_blue_scale_moves_the_blue_trace_and_not_the_red_one() {
    // Перестановку R ↔ B на всём пути «палец → кадр → прибор» ловит именно этот тест.
    let Some(journey) = run_gain_journey(PrimaryChannel::Blue) else {
        return;
    };
    assert!(journey.reference_gap > MIN_REFERENCE_GAP);
    assert_only_the_channel_rose(&journey, PrimaryChannel::Blue);
}
