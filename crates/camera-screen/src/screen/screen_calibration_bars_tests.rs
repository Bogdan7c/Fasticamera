//! Вид BARS целиком (решение 343): переключатель на три вида, общие вкладки с колёсами, три шкалы R, G, B,
//! «↻» у каждой, кольцо на вкладке и геометрия в обеих ориентациях эталонного устройства (440 × 982,67 pt).
//!
//! Проверяется то, что делает пользователь: тап по BARS, палец по шкале канала — и наружу уходит `SetPrimary` с
//! адресом именно этой шкалы выбранной зоны.

use look_contract::{LookControl, LookFacts, LookIntent, LookKnob, PrimaryChannel, WheelZone};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, center, drag_eye, edited, surface_vector};
use super::screen_calibration_wheels_tests::{ALL_ORIENTATIONS, intents, reset_opacity_at};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::{BarsFrames, ColumnFrames, ControlViewKind};
use crate::look_test_support::{knob, look_facts};

/// Экран в разделе калибровки с включённым видом BARS (эффекты включения отброшены).
fn bars(facts: LookFacts, orientation: DeviceOrientation) -> Rig {
    let mut rig = calibrating(facts, orientation);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    rig.run(300);
    rig.effects();
    rig
}

/// Факты, где полоска `(zone, channel)` стоит на `value`, остальные 11 — нейтраль (правка поверх пресета).
fn facts_with_primary(zone: WheelZone, channel: PrimaryChannel, value: f32) -> LookFacts {
    edited(|mut settings| {
        settings.primaries = settings.primaries.with_value(zone, channel, knob(value));
        settings
    })
}

/// Значение в единицах шкалы (−200…+200).
fn units_of(value: LookKnob) -> i32 {
    (value.get() * 200.0).round() as i32
}

/// Все намерения `SetPrimary` из эффектов: `(зона, канал, значение)` по порядку.
fn primaries_sent(effects: &[crate::effect::Effect]) -> Vec<(WheelZone, PrimaryChannel, LookKnob)> {
    intents(effects)
        .into_iter()
        .filter_map(|intent| match intent {
            LookIntent::SetPrimary { zone, channel, value } => Some((zone, channel, value)),
            _ => None,
        })
        .collect()
}

/// Палец проводит шкалу на `units` единиц вокруг её центра (влево — значение растёт, как на всех шкалах): начало и
/// конец симметричны относительно `center`, чтобы путь не выходил за линейку в любой ориентации.
fn drag_unit_swipe(rig: &mut Rig, center: Point, units: i32) {
    let finger_dx = -knob_finger_pt(f64::from(units));
    let back_half = surface_vector(rig, Vector::new(-finger_dx / 2.0, 0.0));
    let from = Point::new(center.x + back_half.dx, center.y + back_half.dy);
    drag_eye(rig, from, Vector::new(finger_dx, 0.0));
}

/// Кольца-метки на вкладках (диаметр 7 pt, обводка без заливки).
fn tab_marker_rings(list: &DrawList) -> usize {
    list.leaf_items()
        .filter(|item| {
            matches!(item, DrawItem::Shape(shape)
                if matches!(shape.geometry, ui_draw::shape::ShapeGeometry::Circle { diameter }
                    if near(diameter, px(7.0), 0.6))
                    && shape.stroke.is_some()
                    && shape.fill.a == 0.0)
        })
        .count()
}

#[test]
fn the_switcher_has_three_views_and_the_labels_fit_their_capsules() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    assert_eq!(rig.screen.node_centers("calib_view_segment").len(), 3);
    let list = rig.frame().clone();
    // Портретная колонка: внутренняя ширина 196 pt, три сегмента с зазором 4 pt (как в тесте вкладок колёс).
    let capsule_pt = (196.0 - 2.0 * 4.0) / 3.0;
    for label in ["SCALES", "WHEELS", "BARS"] {
        let runs = texts_of(&list, label, TextRoleKind::SegmentLabel);
        assert!(!runs.is_empty(), "сегмент {label} нарисован");
        let width_pt = f64::from(runs[0].run.advance_px) / f64::from(PX_PER_PT);
        assert!(
            width_pt < capsule_pt - 2.0,
            "{label}: {width_pt} pt не влезает в {capsule_pt} pt"
        );
    }
}

#[test]
fn a_tap_on_the_third_segment_opens_bars_and_taps_on_the_others_leave_it() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let bars_segment = center(&rig, "calib_view_segment", 2);
    rig.tap(bars_segment);
    rig.run(300);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Bars);
    assert_eq!(rig.screen.node_centers("calib_bars_tab").len(), 4, "четыре вкладки зон");
    assert_eq!(rig.screen.node_centers("calib_ruler").len(), 3, "три шкалы R, G, B");
    let yellow = ui_kit::tokens::UiKitTokens::default().colors.focus_yellow;
    let list = rig.frame().clone();
    assert_eq!(
        texts_of(&list, "BARS", TextRoleKind::SegmentLabel)[0].color,
        yellow,
        "активный сегмент подсвечен"
    );

    let wheels_segment = center(&rig, "calib_view_segment", 1);
    rig.tap(wheels_segment);
    rig.run(300);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Wheels);
    assert!(rig.screen.node_centers("calib_bars_tab").is_empty(), "вид BARS ушёл");
}

#[test]
fn the_tab_is_shared_with_wheels_in_both_directions() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    // Выбрали GAIN в колёсах — в BARS открыт GAIN.
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(300);
    let gain_in_wheels = center(&rig, "calib_wheel_tab", 2);
    rig.tap(gain_in_wheels);
    rig.run(200);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    let list = rig.run(300).clone();
    let yellow = ui_kit::tokens::UiKitTokens::default().colors.focus_yellow;
    assert_eq!(texts_of(&list, "GAIN", TextRoleKind::SegmentLabel)[0].color, yellow);
    assert_ne!(texts_of(&list, "LIFT", TextRoleKind::SegmentLabel)[0].color, yellow);

    // Выбрали OFFSET в BARS — в колёсах открыт OFFSET.
    let offset_in_bars = center(&rig, "calib_bars_tab", 3);
    rig.tap(offset_in_bars);
    rig.run(200);
    assert_eq!(rig.screen.state().calibration().selected_wheel(), WheelZone::Offset);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    let list = rig.run(300).clone();
    assert_eq!(texts_of(&list, "OFFSET", TextRoleKind::SegmentLabel)[0].color, yellow);
}

#[test]
fn each_scale_sends_its_own_channel_of_the_selected_zone_in_every_orientation() {
    // Перестановка R ↔ B или зон ловится: палец по i-й шкале шлёт ровно канал i выбранной зоны.
    for orientation in ALL_ORIENTATIONS {
        for zone in [WheelZone::Lift, WheelZone::Gain] {
            for (index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
                let mut rig = bars(look_facts(), orientation);
                rig.act(ScreenAction::WheelSelected(zone));
                rig.run(200);
                rig.effects();
                let ruler = rig.screen.bars_ruler_placements().expect("шкалы на экране")[index].ruler_center;
                drag_unit_swipe(&mut rig, ruler, 50);
                let sent = primaries_sent(&rig.effects());
                let (last_zone, last_channel, last_value) = *sent.last().expect("палец двигал полоску");
                assert_eq!(
                    (last_zone, last_channel),
                    (zone, channel),
                    "{orientation:?} / {zone:?} / шкала №{index}"
                );
                assert_eq!(units_of(last_value), 50, "{orientation:?} / {zone:?} / {channel:?}");
                assert!(
                    sent.iter()
                        .all(|(sent_zone, sent_channel, _)| (*sent_zone, *sent_channel) == (zone, channel)),
                    "палец тронул только свою полоску: {sent:?}"
                );
            }
        }
    }
}

#[test]
fn a_reset_arrow_resets_exactly_its_own_bar_and_only_when_it_differs() {
    let mut rig = bars(
        facts_with_primary(WheelZone::Gain, PrimaryChannel::Green, 0.4),
        DeviceOrientation::Portrait,
    );
    let resets: Vec<Point> = (0..3).map(|index| center(&rig, "calib_reset", index)).collect();
    // У выбранной Lift все полоски как в пресете: все «↻» тусклые и молчат.
    for (index, point) in resets.iter().enumerate() {
        rig.tap(*point);
        rig.run(100);
        assert!(intents(&rig.effects()).is_empty(), "Lift: «↻» №{index} молчит");
    }

    let gain_tab = center(&rig, "calib_bars_tab", 2);
    rig.tap(gain_tab);
    rig.run(300);
    rig.effects();
    let list = rig.frame().clone();
    let opacities: Vec<f32> = resets
        .iter()
        .map(|point| reset_opacity_at(&list, *point).expect("«↻» нарисована"))
        .collect();
    assert!(near(opacities[1], 1.0, 0.02), "G правлена — яркая: {opacities:?}");
    assert!(
        near(opacities[0], 0.44, 0.02) && near(opacities[2], 0.44, 0.02),
        "{opacities:?}"
    );

    // Тап по «↻» G сбрасывает именно Gain.G; по «↻» R и B — ничего.
    rig.tap(resets[1]);
    rig.run(100);
    assert_eq!(
        intents(&rig.effects()),
        vec![LookIntent::ResetControl(LookControl::Primary {
            zone: WheelZone::Gain,
            channel: PrimaryChannel::Green
        })]
    );
    for index in [0, 2] {
        rig.tap(resets[index]);
        rig.run(100);
        assert!(
            intents(&rig.effects()).is_empty(),
            "«↻» №{index} при полоске как в пресете молчит"
        );
    }
}

#[test]
fn the_tab_ring_appears_with_an_edit_in_its_zone_and_disappears_when_it_returns_to_the_preset() {
    let untouched = bars(look_facts(), DeviceOrientation::Portrait).frame().clone();
    assert_eq!(
        tab_marker_rings(&untouched),
        0,
        "ни одна полоска не правлена — колец нет"
    );

    // Любая из трёх полосок зоны ставит кольцо на ЕЁ вкладку и только на неё.
    for channel in PrimaryChannel::ALL {
        let edited_facts = facts_with_primary(WheelZone::Gamma, channel, -0.3);
        let mut rig = bars(edited_facts, DeviceOrientation::Portrait);
        assert_eq!(
            tab_marker_rings(&rig.frame().clone()),
            1,
            "правлена {channel:?} у Gamma"
        );
        // Библиотека вернула полоску к пресету — кольцо исчезает.
        rig.screen
            .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts())));
        rig.run(300);
        assert_eq!(
            tab_marker_rings(&rig.frame().clone()),
            0,
            "возврат к пресету снимает кольцо"
        );
    }
}

#[test]
fn the_view_fits_without_scrolling_in_both_orientations_of_the_reference_device() {
    for orientation in ALL_ORIENTATIONS {
        let rig = bars(look_facts(), orientation);
        let config = crate::calibration::CalibrationWidgetsConfig::default();
        let layout = *rig.screen.state().calibration().layout().expect("раскладка раздела");
        let column = ColumnFrames::of(layout.zones().knobs, &config);
        let frames = BarsFrames::of(column.content, &config).expect("вид помещается без прокрутки");
        assert!(
            frames.rows[2].max_y() + config.panel_padding_pt <= layout.zones().knobs.max_y() + 1e-9,
            "{orientation:?}: нижняя шкала и отступ внутри панели"
        );
        // Запрос фасада совпадает с нарисованными узлами строк (в порядке R, G, B).
        let placements = rig.screen.bars_ruler_placements().expect("шкалы на экране");
        for (index, placement) in placements.iter().enumerate() {
            let drawn = center(&rig, "calib_row", index);
            let queried = placement.row.center();
            assert!(
                (drawn.x - queried.x).abs() < 0.5 && (drawn.y - queried.y).abs() < 0.5,
                "{orientation:?}: строка №{index} нарисована в {drawn:?}, запрос {queried:?}"
            );
            assert_eq!(placement.full_travel_pt, 400.0, "ход пальца на полную шкалу");
        }
    }
}

#[test]
fn the_landscape_column_gives_exactly_the_planned_numbers() {
    // Решение 343: окно под шапкой и переключателем 272 pt, вкладки и три шкалы с зазором и отступом — 260 pt.
    let rig = bars(look_facts(), DeviceOrientation::LandscapeLeft);
    let config = crate::calibration::CalibrationWidgetsConfig::default();
    let layout = *rig.screen.state().calibration().layout().expect("раскладка раздела");
    let column = ColumnFrames::of(layout.zones().knobs, &config);
    assert!(
        (column.window.size.height - 272.0).abs() < 0.1,
        "окно вида в альбоме ≈ 272 pt: {}",
        column.window.size.height
    );
    let frames = BarsFrames::of(column.content, &config).expect("помещается");
    assert_eq!(
        frames.rows[2].max_y() - column.content.min_y(),
        248.0,
        "вкладки + зазор + три шкалы"
    );
}

#[test]
fn bars_survive_a_rotation_and_work_in_the_new_orientation() {
    let mut rig = bars(look_facts(), DeviceOrientation::Portrait);
    rig.act(ScreenAction::WheelSelected(WheelZone::Gamma));
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(600);
    rig.effects();
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Bars);
    assert_eq!(rig.screen.node_centers("calib_ruler").len(), 3, "вид собран заново");
    let blue = rig.screen.bars_ruler_placements().expect("шкалы после поворота")[2].ruler_center;
    drag_unit_swipe(&mut rig, blue, -40);
    let sent = primaries_sent(&rig.effects());
    let last = sent.last().expect("палец двигал полоску");
    assert_eq!(
        (last.0, last.1, units_of(last.2)),
        (WheelZone::Gamma, PrimaryChannel::Blue, -40)
    );
}

#[test]
fn the_placement_query_is_empty_outside_the_bars_view() {
    let rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    assert!(rig.screen.bars_ruler_placements().is_none(), "вид шкал");
    assert!(
        Rig::new().screen.bars_ruler_placements().is_none(),
        "не раздел калибровки"
    );
}

#[test]
fn when_the_view_does_not_fit_the_column_lives_without_bars_counts_once_and_scales_come_back() {
    let mut config = crate::ScreenConfig::default();
    // Строка выше колонки: вкладки и три шкалы заведомо не помещаются.
    config.calibration.widgets.row_height_pt = 400.0;
    let mut rig = Rig::with(config, ui_core::time::MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts())));
    rig.act(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    rig.run(1000);
    rig.effects();

    assert!(rig.screen.bars_ruler_placements().is_none(), "шкал нет");
    assert!(rig.screen.node_centers("calib_bars_tab").is_empty());
    assert_eq!(
        rig.screen.node_centers("calib_view_segment").len(),
        3,
        "переключатель на месте"
    );
    assert_eq!(
        rig.screen
            .state()
            .rejections()
            .get(crate::outcome::Rejection::InvalidSurface),
        1,
        "причина посчитана один раз, а не на каждом кадре"
    );
    let scales = center(&rig, "calib_view_segment", 0);
    rig.tap(scales);
    rig.run(300);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Scales);
}

#[test]
fn without_look_facts_the_bars_are_inert() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    rig.run(300);
    rig.effects();
    let ruler = rig.screen.bars_ruler_placements().expect("вид есть и без фактов")[0].ruler_center;
    drag_unit_swipe(&mut rig, ruler, 50);
    assert!(
        primaries_sent(&rig.effects()).is_empty(),
        "без фактов палец ничего не шлёт"
    );
}
