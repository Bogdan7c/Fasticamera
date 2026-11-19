//! Тесты демо-сцены стекла: наблюдаемый результат — списки рисования, которые увидит рендер.
//!
//! Время во всех проверках — целые наносекунды кадров 60 Гц (`k · 1e9 / 60`), чтобы 60 и 120 Гц
//! совпадали в общие моменты без накопления ошибки.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

use std::sync::Arc;

use shell_contract::clock::BoottimeNanos;
use shell_contract::gravity::{GravitySample, GravitySlot, GravityVector};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, MAX_GLASS_GROUP_MEMBERS};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::light::LightDirection;

use super::config::{GlassShowcaseConfig, ShowcaseLayout};
use super::error::ShowcaseConfigError;
use super::timeline::{Schedule, ShowcaseTarget, ShowcaseTimeline, WindowEdge};
use super::*;

const SCREEN_WIDTH_PX: f32 = 1080.0;
const SCREEN_HEIGHT_PX: f32 = 2412.0;
/// Масштаб нашего телефона: 1080 px / 440 pt (решение 82).
const PX_PER_PT: f32 = SCREEN_WIDTH_PX / 440.0;
const FRAMES_PER_SECOND: i64 = 60;

// Порядок групп стекла в списке (фиксирован строителем).
const CAPSULE: usize = 0;
const FLASH: usize = 1;
const FLIP: usize = 2;
const ZOOM: usize = 3;
const MENU: usize = 4;
const PANEL: usize = 5;

fn request_at(nanos: i64) -> UiFrameRequest {
    UiFrameRequest {
        present_time: MonotonicNanos::from_nanos(nanos),
        logical_size: PxSize::new(SCREEN_WIDTH_PX, SCREEN_HEIGHT_PX),
        px_per_pt: PX_PER_PT,
    }
}

fn nanos_of_frame(frame: i64, frames_per_second: i64) -> i64 {
    frame * 1_000_000_000 / frames_per_second
}

fn showcase_with(gravity: Option<Arc<GravitySlot>>) -> GlassShowcase {
    GlassShowcase::new(GlassShowcaseConfig::default(), &UiKitTokens::default(), gravity).unwrap()
}

fn showcase() -> GlassShowcase {
    showcase_with(None)
}

/// Непрерывный прогон кадров 60 Гц от нуля на `seconds` секунд: список на каждый кадр.
fn run_60hz(showcase: &mut GlassShowcase, seconds: i64) -> Vec<DrawList> {
    (0..=seconds * FRAMES_PER_SECOND)
        .map(|frame| {
            showcase
                .produce_frame(&request_at(nanos_of_frame(frame, FRAMES_PER_SECOND)))
                .unwrap()
        })
        .collect()
}

/// Список на момент `seconds` из прогона 60 Гц (момент обязан попадать на кадр).
fn at_seconds(frames: &[DrawList], seconds: f64) -> &DrawList {
    let frame = seconds * FRAMES_PER_SECOND as f64;
    assert!((frame - frame.round()).abs() < 1e-9, "{seconds} с не на кадре 60 Гц");
    &frames[frame.round() as usize]
}

fn glass_groups(list: &DrawList) -> Vec<&GlassGroupItem> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Glass(group) => Some(group),
            _ => None,
        })
        .collect()
}

fn member_center(group: &GlassGroupItem, member: usize) -> PxPoint {
    group.members[member].transform.apply_point(PxPoint::ORIGIN)
}

fn member_bounds(group: &GlassGroupItem, member: usize) -> PxRect {
    group.members[member].shape_bounds_px().unwrap()
}

fn midpoint(a: PxPoint, b: PxPoint) -> PxPoint {
    PxPoint::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

/// Два периода цикла (16 с) при 60 Гц: каждый кадр проходит проверку словаря,
/// число элементов и групп постоянно, члены группы в пределе.
#[test]
fn every_frame_of_two_cycles_validates_with_stable_item_count() {
    let mut showcase = showcase();
    let frames = run_60hz(&mut showcase, 16);
    let expected_items = frames[0].items.len();
    assert_eq!(expected_items, 16, "3 не-стекла + 6 групп + 7 подписей/иконок");
    for (index, list) in frames.iter().enumerate() {
        list.validate().unwrap_or_else(|error| panic!("кадр {index}: {error}"));
        assert_eq!(list.items.len(), expected_items, "кадр {index}");
        let groups = glass_groups(list);
        assert_eq!(groups.len(), 6, "кадр {index}");
        assert!(
            groups
                .iter()
                .all(|group| group.members.len() <= MAX_GLASS_GROUP_MEMBERS)
        );
        assert_eq!(groups[ZOOM].members.len(), 3, "кружки зума — в одной группе");
    }
}

/// Все стеклянные формы остаются внутри экрана 1080×2412 за весь цикл (нажатия, разведение, раскрытие).
#[test]
fn glass_shapes_stay_inside_the_screen_for_the_whole_cycle() {
    let mut showcase = showcase();
    let frames = run_60hz(&mut showcase, 8);
    for (index, list) in frames.iter().enumerate() {
        for (group_index, group) in glass_groups(list).iter().enumerate() {
            for member in 0..group.members.len() {
                let bounds = member_bounds(group, member);
                assert!(
                    bounds.origin.x >= -0.5
                        && bounds.origin.y >= -0.5
                        && bounds.max_x() <= SCREEN_WIDTH_PX + 0.5
                        && bounds.max_y() <= SCREEN_HEIGHT_PX + 0.5,
                    "кадр {index}, группа {group_index}, член {member}: {bounds:?}"
                );
            }
        }
    }
}

/// Раскладка как в `layout.md`: капсула по центру по x, низ — 34,1 pt от низа экрана.
#[test]
fn capsule_is_centered_and_sits_one_safe_area_above_the_bottom() {
    let mut showcase = showcase();
    let list = showcase.produce_frame(&request_at(0)).unwrap();
    let capsule = member_bounds(glass_groups(&list)[CAPSULE], 0);
    let layout = ShowcaseLayout::default();
    assert!(
        (capsule.center().x - SCREEN_WIDTH_PX * 0.5).abs() < 0.5,
        "центр по x: {capsule:?}"
    );
    let gap_to_bottom_pt = (SCREEN_HEIGHT_PX - capsule.max_y()) / PX_PER_PT;
    assert!(
        (f64::from(gap_to_bottom_pt) - layout.bottom_margin_pt).abs() < 0.05,
        "до низа {gap_to_bottom_pt} pt"
    );
    assert!(
        (capsule.size.width / PX_PER_PT - 220.0).abs() < 0.05,
        "ширина капсулы в pt"
    );
    // Капсула и нижние кнопки стоят в одном ряду: общий низ.
    let flash = member_bounds(glass_groups(&list)[FLASH], 0);
    assert!((flash.max_y() - capsule.max_y()).abs() < 0.5);
}

/// Два экземпляра, одни и те же моменты → одинаковые списки; 60 и 120 Гц совпадают в общие моменты.
#[test]
fn same_time_gives_the_same_list_regardless_of_instance_and_frame_rate() {
    let frames_60 = run_60hz(&mut showcase(), 8);
    let frames_60_again = run_60hz(&mut showcase(), 8);
    assert_eq!(frames_60, frames_60_again, "два экземпляра, один прогон");

    let mut showcase_120 = showcase();
    for frame in 0..=8 * 120 {
        let list = showcase_120
            .produce_frame(&request_at(nanos_of_frame(frame, 120)))
            .unwrap();
        if frame % 2 == 0 {
            assert_eq!(&list, &frames_60[(frame / 2) as usize], "кадр {frame} при 120 Гц");
        }
    }
}

/// Первый кадр посреди цикла = то, что показал бы непрерывный прогон: сцена восстанавливается
/// проигрыванием событий цикла, а не стартует «с нуля» в неверном состоянии.
#[test]
fn first_frame_mid_cycle_matches_continuous_run() {
    let frames = run_60hz(&mut showcase(), 8);
    for seconds in [1.0, 3.0, 5.0, 6.0] {
        let mut late_start = showcase();
        let nanos = (seconds * 1e9) as i64;
        let list = late_start.produce_frame(&request_at(nanos)).unwrap();
        assert_eq!(&list, at_seconds(&frames, seconds), "старт в {seconds} с");
    }
}

/// Скачок времени назад: список равен тому, что дал бы свежий экземпляр на этот момент.
#[test]
fn time_jump_backwards_rebuilds_the_scene_consistently() {
    let mut jumped = showcase();
    jumped.produce_frame(&request_at(5_000_000_000)).unwrap();
    let after_jump = jumped.produce_frame(&request_at(1_000_000_000)).unwrap();
    let fresh = showcase().produce_frame(&request_at(1_000_000_000)).unwrap();
    assert_eq!(after_jump, fresh);
    // И длинный скачок вперёд (больше периода), и переход через границу цикла.
    let mut forward = showcase();
    forward.produce_frame(&request_at(0)).unwrap();
    let far = forward
        .produce_frame(&request_at(100_000_000_000 + 3_000_000_000))
        .unwrap();
    let fresh_far = showcase()
        .produce_frame(&request_at(100_000_000_000 + 3_000_000_000))
        .unwrap();
    assert_eq!(far, fresh_far);
}

/// Нажатие: `amount` растёт к 1 в окне и возвращается к 0; масштаб члена > 1 в нажатии.
#[test]
fn button_press_rises_to_one_and_returns_to_zero_with_scale() {
    let frames = run_60hz(&mut showcase(), 8);
    let timeline = ShowcaseTimeline::default();
    let window = timeline.flash_press;
    let press_of = |list: &DrawList| glass_groups(list)[FLASH].members[0].press.amount;
    let scale_of = |list: &DrawList| glass_groups(list)[FLASH].members[0].transform.a;

    let rest_scale = scale_of(at_seconds(&frames, 0.5));
    assert_eq!(press_of(at_seconds(&frames, 0.5)), 0.0, "до окна не нажата");

    let peak = (36..=72).map(|frame| press_of(&frames[frame])).fold(0.0_f32, f32::max);
    assert!(window.start_s < 0.7 && peak > 0.95, "пик нажатия {peak}");
    assert!(peak <= 1.0, "amount зажат в 0..1 даже при перелёте пружины: {peak}");
    let pressed = &frames[72]; // конец окна, 1,2 с
    assert!(
        scale_of(pressed) > rest_scale * 1.08,
        "масштаб в нажатии {} против покоя {rest_scale}",
        scale_of(pressed)
    );

    // Через ~0,8 с после отпускания снова покой, масштаб вернулся.
    let released = at_seconds(&frames, 2.0);
    assert!(press_of(released) < 0.01, "после отпускания {}", press_of(released));
    assert!((scale_of(released) - rest_scale).abs() < 1e-3);
    // Другая кнопка за это время не нажималась в момент нажатия первой.
    assert_eq!(glass_groups(&frames[54])[FLIP].members[0].press.amount, 0.0);
}

/// Вторая кнопка нажимается своим окном, независимо от первой.
#[test]
fn flip_button_has_its_own_press_window() {
    let frames = run_60hz(&mut showcase(), 8);
    let window = ShowcaseTimeline::default().flip_press;
    let end_frame = (window.end_s * 60.0).round() as usize;
    let peak = (84..=end_frame)
        .map(|frame| glass_groups(&frames[frame])[FLIP].members[0].press.amount)
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.95, "пик нажатия переворота {peak}");
    assert!(glass_groups(&frames[end_frame])[FLASH].members[0].press.amount < 0.02);
}

/// Слияние «как капли»: в сведённом состоянии поле между соседними кружками < 0, в разведённом > 0.
#[test]
fn zoom_circles_merge_when_together_and_split_when_apart() {
    let frames = run_60hz(&mut showcase(), 8);
    let field_between_first_two = |list: &DrawList| {
        let group = glass_groups(list)[ZOOM];
        let between = midpoint(member_center(group, 0), member_center(group, 1));
        group.field_at(between).unwrap().unwrap().distance_px
    };
    let gap = |list: &DrawList| {
        let group = glass_groups(list)[ZOOM];
        member_center(group, 1).x - member_center(group, 0).x
    };
    let together = at_seconds(&frames, 2.2);
    let apart = at_seconds(&frames, 3.8);
    let together_again = at_seconds(&frames, 7.5);
    assert!(
        field_between_first_two(together) < 0.0,
        "сведены: {}",
        field_between_first_two(together)
    );
    assert!(
        field_between_first_two(apart) > 0.0,
        "разведены: {}",
        field_between_first_two(apart)
    );
    assert!(field_between_first_two(together_again) < 0.0, "снова сведены");
    // Шаг центров: 44,7 pt сведены, 58 pt разведены.
    assert!(
        (gap(together) / PX_PER_PT - 44.7).abs() < 0.05,
        "{}",
        gap(together) / PX_PER_PT
    );
    assert!(
        (gap(apart) / PX_PER_PT - 58.0).abs() < 0.2,
        "{}",
        gap(apart) / PX_PER_PT
    );
    // Радиус слияния — именно из конфига.
    let expected_merge = 12.0 * PX_PER_PT;
    assert!((glass_groups(together)[ZOOM].merge_radius_px - expected_merge).abs() < 1e-3);
}

/// Морф панели: `effect` 0 свёрнута, 1 раскрыта; рамка растёт из рамки кнопки меню.
#[test]
fn panel_grows_out_of_the_menu_button_with_materialization() {
    let frames = run_60hz(&mut showcase(), 8);
    let panel_bounds = |list: &DrawList| member_bounds(glass_groups(list)[PANEL], 0);
    let effect = |list: &DrawList| glass_groups(list)[PANEL].effect;

    let collapsed = at_seconds(&frames, 4.0);
    assert_eq!(effect(collapsed), 0.0);
    let menu = member_bounds(glass_groups(collapsed)[MENU], 0);
    let start = panel_bounds(collapsed);
    assert!(
        (start.center().x - menu.center().x).abs() < 0.5,
        "центры x: {start:?} / {menu:?}"
    );
    assert!(
        (start.center().y - menu.center().y).abs() < 0.5,
        "центры y: {start:?} / {menu:?}"
    );
    assert!(start.size.width <= menu.size.width + 0.5 && start.size.height <= menu.size.height + 0.5);

    // Рост: эффект и размер монотонно растут, в середине пути эффект между 0 и 1.
    let mut previous_effect = 0.0_f32;
    let mut previous_width = start.size.width;
    for frame in (264..=330).step_by(6) {
        let list = &frames[frame];
        assert!(
            effect(list) >= previous_effect - 1e-4,
            "эффект не убывает, кадр {frame}"
        );
        assert!(
            panel_bounds(list).size.width >= previous_width - 1e-3,
            "рамка растёт, кадр {frame}"
        );
        previous_effect = effect(list);
        previous_width = panel_bounds(list).size.width;
    }
    let mid = effect(&frames[270]);
    assert!(
        mid > 0.05 && mid < 0.95,
        "материализация идёт плавно, а не скачком: {mid}"
    );

    let open = at_seconds(&frames, 6.0);
    assert!(effect(open) > 0.999, "раскрыта: {}", effect(open));
    let full = panel_bounds(open);
    assert!((full.size.width / PX_PER_PT - 382.0).abs() < 0.1);
    assert!((full.size.height / PX_PER_PT - 416.0).abs() < 0.1);
    // Раскрытая панель сидит над кнопкой меню (с зазором), справа с отступом ≈ 10,5 pt.
    assert!(full.max_y() < menu.origin.y);
    assert!(((SCREEN_WIDTH_PX - full.max_x()) / PX_PER_PT - 10.5).abs() < 0.1);
    // Кнопка меню, из которой растёт панель, к этому моменту погашена.
    assert!(glass_groups(open)[MENU].opacity.get() < 0.01);

    // Сворачивание обратно к кнопке.
    let closed = at_seconds(&frames, 7.5);
    assert_eq!(effect(closed), 0.0);
    assert!((panel_bounds(closed).center().x - menu.center().x).abs() < 0.5);
}

/// Все пять групп используют нужные пресеты: панель — панельный, остальное — кнопочный.
#[test]
fn glass_presets_come_from_tokens_per_role() {
    let tokens = UiKitTokens::default();
    let list = showcase().produce_frame(&request_at(0)).unwrap();
    let groups = glass_groups(&list);
    let capsule_side = groups[CAPSULE].members[0].geometry.extent().min_side();
    let expected_button = tokens
        .glass
        .button
        .resolve(PxSize::new(capsule_side, capsule_side), PX_PER_PT);
    assert_eq!(
        groups[CAPSULE].params, expected_button,
        "капсула берёт кнопочный пресет"
    );
    assert_eq!(groups[FLASH].params.blur_radius_px, 2.0 * PX_PER_PT);
    // Свёрнутая панель: размытие панельного пресета 8 pt (а не 2 pt кнопки).
    assert_eq!(groups[PANEL].params.blur_radius_px, 8.0 * PX_PER_PT);
}

/// Обычные фигуры и текст: жёлтая обводка рамки фокуса, жёлтая плашка, текст в списке с ненулевой шириной.
#[test]
fn plain_shapes_and_text_are_present_and_yellow() {
    let list = showcase().produce_frame(&request_at(0)).unwrap();
    let config = GlassShowcaseConfig::default();
    let shapes: Vec<_> = list
        .items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) => Some(shape),
            _ => None,
        })
        .collect();
    assert_eq!(shapes.len(), 2);
    assert_eq!(
        shapes[0].stroke.unwrap().color,
        config.look.focus_yellow,
        "рамка фокуса — обводка"
    );
    assert_eq!(shapes[0].fill, ui_draw::color::DisplayColor::TRANSPARENT);
    assert_eq!(shapes[1].fill, config.look.focus_yellow, "плашка залита жёлтым");
    let texts: Vec<_> = list
        .items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    // Плашка, PHOTO, три подписи зума.
    assert_eq!(texts.len(), 5);
    assert!(
        texts
            .iter()
            .all(|text| text.run.advance_px > 0.0 && !text.run.glyphs.is_empty())
    );
    // Подпись PHOTO стоит по центру капсулы: середина строки (baseline-left + advance/2) в центре капсулы.
    let capsule_center = member_center(glass_groups(&list)[CAPSULE], 0);
    let photo = texts[1];
    let middle = photo
        .transform
        .apply_point(PxPoint::new(photo.run.advance_px * 0.5, 0.0));
    assert!((middle.x - capsule_center.x).abs() < 0.5);
    let icons = list
        .items
        .iter()
        .filter(|item| matches!(item, DrawItem::Icon(_)))
        .count();
    assert_eq!(icons, 3, "вспышка, переворот, шеврон меню");
}

fn tilt_right_slot() -> Arc<GravitySlot> {
    let slot = Arc::new(GravitySlot::new());
    // Верх телефона уходит вправо на 30° (gx = −g·sin φ), плоскость экрана вертикальна.
    slot.publish(GravitySample {
        time: BoottimeNanos::from_nanos(1_000_000_000),
        vector: GravityVector {
            x: -4.905,
            y: 8.496,
            z: 0.0,
        },
    });
    slot
}

/// Свет: без ящика — покой (угол пресета), с наклоном вправо — повёрнут против часовой на 30°.
#[test]
fn light_follows_tilt_when_gravity_is_given_and_rests_otherwise() {
    let rest_angle = UiKitTokens::default().glass.button.specular.rest_angle_deg;
    let rest = LightDirection::from_angle_degrees(rest_angle);

    let without = showcase().produce_frame(&request_at(0)).unwrap();
    assert_eq!(without.light, rest, "без слота — покой");

    let empty_slot = Arc::new(GravitySlot::new());
    let with_empty_slot = showcase_with(Some(empty_slot)).produce_frame(&request_at(0)).unwrap();
    assert_eq!(with_empty_slot.light, rest, "датчик ещё молчит — покой");

    let tilted = showcase_with(Some(tilt_right_slot()))
        .produce_frame(&request_at(0))
        .unwrap();
    assert_ne!(tilted.light, rest);
    assert!(
        (tilted.light.angle_degrees() - (rest_angle - 30.0)).abs() < 0.01,
        "угол света {}",
        tilted.light.angle_degrees()
    );
}

/// Ошибки источника: масштаб расходится со сценой, экран непригоден; после ошибки кадр строится снова.
#[test]
fn bad_requests_fail_loudly_and_the_source_recovers() {
    let mut showcase = showcase();
    let good = showcase.produce_frame(&request_at(0)).unwrap();

    let wrong_scale = UiFrameRequest {
        px_per_pt: 3.0,
        ..request_at(16_666_667)
    };
    match showcase.produce_frame(&wrong_scale) {
        Err(UiFrameError::SourceFailed { reason }) => assert!(reason.contains("px_per_pt"), "{reason}"),
        other => panic!("ожидался отказ из-за масштаба, получено {other:?}"),
    }
    let empty_screen = UiFrameRequest {
        logical_size: PxSize::new(0.0, 0.0),
        ..request_at(16_666_667)
    };
    assert!(matches!(
        showcase.produce_frame(&empty_screen),
        Err(UiFrameError::SourceFailed { .. })
    ));
    let nan_screen = UiFrameRequest {
        logical_size: PxSize::new(f32::NAN, 2412.0),
        ..request_at(16_666_667)
    };
    assert!(matches!(
        showcase.produce_frame(&nan_screen),
        Err(UiFrameError::SourceFailed { .. })
    ));
    // Источник восстановился и снова даёт тот же список на тот же момент.
    let recovered = showcase.produce_frame(&request_at(0)).unwrap();
    assert_eq!(recovered, good);
}

/// Смена размера экрана: сцена перестраивается, раскладка следует новой высоте (привязка к низу).
#[test]
fn resize_keeps_bottom_anchoring() {
    let mut showcase = showcase();
    showcase.produce_frame(&request_at(0)).unwrap();
    let taller = UiFrameRequest {
        logical_size: PxSize::new(SCREEN_WIDTH_PX, 2600.0),
        ..request_at(16_666_667)
    };
    let list = showcase.produce_frame(&taller).unwrap();
    let capsule = member_bounds(glass_groups(&list)[CAPSULE], 0);
    assert!(
        ((2600.0 - capsule.max_y()) / PX_PER_PT - 34.1).abs() < 0.05,
        "{capsule:?}"
    );
}

// ---- конфиг и расписание ----

#[test]
fn default_config_is_valid_and_roundtrips_through_json() {
    let config = GlassShowcaseConfig::default();
    config.validate().unwrap();
    let json = serde_json::to_string(&config).unwrap();
    let parsed: GlassShowcaseConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, config);
    // Пустой раздел → умолчания (`#[serde(default)]`).
    assert_eq!(serde_json::from_str::<GlassShowcaseConfig>("{}").unwrap(), config);
}

#[test]
fn unknown_config_fields_are_rejected() {
    assert!(serde_json::from_str::<GlassShowcaseConfig>(r#"{"cycle":1}"#).is_err());
    assert!(serde_json::from_str::<GlassShowcaseConfig>(r#"{"timeline":{"cycle_period":1}}"#).is_err());
    assert!(serde_json::from_str::<GlassShowcaseConfig>(r#"{"layout":{"edge":1}}"#).is_err());
}

#[test]
fn inconsistent_configs_are_refused_with_a_reason() {
    // Радиус слияния меньше удвоенного зазора сведённых кружков — «капли» не слипнутся.
    let mut config = GlassShowcaseConfig::default();
    config.look.zoom_merge_radius_pt = 0.5;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::Inconsistent { .. })
    ));
    // … и больше удвоенного зазора разведённых — не разойдутся.
    config.look.zoom_merge_radius_pt = 40.0;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::Inconsistent { .. })
    ));

    // Окно заканчивается позже, чем пружины успеют успокоиться до конца цикла.
    let mut config = GlassShowcaseConfig::default();
    config.timeline.panel_open.end_s = 7.5;
    assert!(matches!(config.validate(), Err(ShowcaseConfigError::OutOfRange { .. })));
    // Конец окна раньше начала.
    let mut config = GlassShowcaseConfig::default();
    config.timeline.flash_press.end_s = 0.1;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::Inconsistent { .. })
    ));
    // Разведённый шаг не больше сведённого.
    let mut config = GlassShowcaseConfig::default();
    config.layout.zoom_pitch_apart_pt = 40.0;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::Inconsistent { .. })
    ));
    // Нечисло и нулевой размер.
    let mut config = GlassShowcaseConfig::default();
    config.layout.panel.width_pt = 0.0;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::NotPositive { .. })
    ));
    let mut config = GlassShowcaseConfig::default();
    config.look.mode_label_size_pt = f32::NAN;
    assert!(matches!(
        config.validate(),
        Err(ShowcaseConfigError::NotPositive { .. })
    ));
    // Пустая подпись.
    let mut config = GlassShowcaseConfig::default();
    config.look.labels.mode.clear();
    assert!(matches!(config.validate(), Err(ShowcaseConfigError::EmptyLabel { .. })));
    // Конструктор отказывает на таком конфиге, а не молча строит сцену.
    assert!(GlassShowcase::new(config, &UiKitTokens::default(), None).is_err());
}

#[test]
fn label_outside_the_baked_charset_fails_the_frame_not_silently() {
    let mut config = GlassShowcaseConfig::default();
    config.look.labels.mode = "ФОТО".to_owned();
    let mut showcase = GlassShowcase::new(config, &UiKitTokens::default(), None).unwrap();
    match showcase.produce_frame(&request_at(0)) {
        Err(UiFrameError::SourceFailed { reason }) => assert!(reason.contains("не входит"), "{reason}"),
        other => panic!("ожидался отказ вёрстки, получено {other:?}"),
    }
}

#[test]
fn schedule_orders_events_and_crosses_cycle_boundary() {
    let schedule = Schedule::from_timeline(&ShowcaseTimeline::default());
    assert_eq!(schedule.events_per_cycle(), 8);
    let first = schedule.first_index_of_cycle(MonotonicNanos::from_nanos(0));
    let mut previous = i64::MIN;
    for index in first..first + 3 * schedule.events_per_cycle() {
        let (time, _) = schedule.event(index);
        assert!(time.as_nanos() >= previous, "время событий не убывает");
        previous = time.as_nanos();
    }
    // Первое событие — вход в нажатие вспышки в 0,6 с; первое событие второго цикла — на период позже.
    let (time, event) = schedule.event(0);
    assert_eq!(time.as_nanos(), 600_000_000);
    assert_eq!(
        (event.target, event.edge),
        (ShowcaseTarget::FlashPress, WindowEdge::Enter)
    );
    let (next_cycle_time, next_cycle_event) = schedule.event(8);
    assert_eq!(next_cycle_time.as_nanos(), 8_600_000_000);
    assert_eq!(next_cycle_event, event);
    // Начало цикла: по модулю периода, в том числе для отрицательного времени.
    assert_eq!(
        schedule
            .cycle_start(MonotonicNanos::from_nanos(17_000_000_000))
            .as_nanos(),
        16_000_000_000
    );
    assert_eq!(
        schedule.cycle_start(MonotonicNanos::from_nanos(-1)).as_nanos(),
        -8_000_000_000
    );
}
