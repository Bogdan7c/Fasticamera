//! Сквозные тесты `viewfinder-stage-budget` на продуктовом экране (сессия 16a): настоящий маршрутизатор, настоящий
//! движок камеры на fake-платформе, НАСТОЯЩИЙ экран камеры, подставной рендер и подставной скрипт-драйвер
//! (`event_router_viewfinder_surface_support`). Каждый тест доходит до строки результата.

use render_loop::viewfinder::{GpuWorkSuppression, SuppressibleGpuWork};

use super::surface_support::{Driver, SurfaceFaults, run_surface, run_surface_timed};
use super::*;
use crate::look_row::LookRow;
use crate::stage_budget_surface_config::StageBudgetSurface;

fn surface_config(surface: StageBudgetSurface, rows: Vec<StageBudgetRowSetting>) -> CameraConfig {
    let mut config = short_config(|viewfinder, _| {
        viewfinder.stage_budget.rows = rows;
        viewfinder.stage_budget.surface = surface;
        viewfinder.stage_budget.surface_ready_timeout_ms = 3_000;
        // Окно в тестах 300 мс: запас и охранный срок удержания соразмерны.
        viewfinder.stage_budget.display_hold_margin_ms = 10;
        viewfinder.stage_budget.display_hold_guard_ms = 100;
    });
    config.scenarios.screen.warmup_ui_frames = 8;
    config
}

fn table_of(result: &serde_json::Value) -> Vec<String> {
    result["report"]["table"]
        .as_array()
        .expect("таблица")
        .iter()
        .map(|line| line.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn suppression_commands(harness: &Harness) -> Vec<GpuWorkSuppression> {
    harness
        .control
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(mask)) => Some(mask),
            _ => None,
        })
        .collect()
}

fn mask_of(work: SuppressibleGpuWork) -> GpuWorkSuppression {
    GpuWorkSuppression::NONE.suppressing(work)
}

#[test]
fn photo_screen_ui_row_sends_exactly_the_expected_commands_in_order_and_restores_the_product() {
    let config = surface_config(StageBudgetSurface::PhotoScreen, vec![StageBudgetRowSetting::UiOverlay]);
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["surface"], "photo_screen");
    let table = table_of(&result);
    assert_eq!(table.len(), 1, "одна строка, базы у `ui_overlay` нет: {table:?}");
    // A: UI рисуется (1,5 мс GPU), B: подавлен (0 мс по определению) → цена 1,50 / 1,80; ток 520 против 500.
    assert!(table[0].starts_with("ui_overlay | 1.50 / 1.80 | 20.0"), "{table:?}");
    assert!(table[0].ends_with("| measured"), "{table:?}");

    // ABBA = окна A B B A A B, команды идут при смене положения: A, B, A, B. Каждая — рисовальщик, стадии, маска.
    let recorded = harness.control.recorded();
    let product_pair = [
        RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(ViewfinderPainter::Isp)),
        RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(IspStageSet::ALL)),
    ];
    let state = |mask: GpuWorkSuppression| {
        let mut commands = product_pair.to_vec();
        commands.push(RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(mask)));
        commands
    };
    let ui_off = mask_of(SuppressibleGpuWork::UiOverlayDrawing);
    let expected: Vec<RenderAdjustment> = [GpuWorkSuppression::NONE, ui_off, GpuWorkSuppression::NONE, ui_off]
        .into_iter()
        .flat_map(state)
        .collect();
    let viewfinder_commands: Vec<RenderAdjustment> = recorded
        .iter()
        // Место видоискателя (`SetFraming`) шлёт сам экран камеры — это не рычаги сценария.
        .filter(|adjustment| {
            matches!(
                adjustment,
                RenderAdjustment::Viewfinder(
                    ViewfinderCommand::SetPainter(_)
                        | ViewfinderCommand::SetIspStages(_)
                        | ViewfinderCommand::SuppressGpuWork(_)
                )
            )
        })
        .take(expected.len())
        .cloned()
        .collect();
    assert_eq!(viewfinder_commands, expected, "{recorded:?}");
    assert_eq!(
        suppression_commands(&harness).last(),
        Some(&GpuWorkSuppression::NONE),
        "маска возвращена к продукту"
    );
    let left = effective(&recorded);
    assert_eq!(
        (left.painter, left.stages, left.suppression),
        (ViewfinderPainter::Isp, IspStageSet::ALL, GpuWorkSuppression::NONE)
    );
    // Экран остался на месте и в PHOTO, снимков не делалось.
    assert!(harness.router.camera_screen().status().is_installed());
    assert!(harness.router.camera_screen().capture_journal().is_empty());
    assert_fits_logcat(&harness, &result);
}

#[test]
fn calibrate_rows_wait_for_the_swipe_and_live_scopes_then_measure_and_swipe_back_to_photo() {
    let config = surface_config(
        StageBudgetSurface::CalibrateScreen,
        vec![
            StageBudgetRowSetting::ColorScopes,
            StageBudgetRowSetting::UiOverlay,
            StageBudgetRowSetting::WholeProduct,
        ],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["surface"], "calibrate_screen");
    // Вход: свайп влево, выход: свайп вправо — по одному разу, в этом порядке.
    let gestures: Vec<String> = stages_named(&harness, "budget_gesture")
        .iter()
        .map(|stage| stage["step"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(gestures, ["modes_swipe_left", "modes_swipe_right"]);
    let names = harness.output.stage_names();
    let position = |name: &str| names.iter().position(|stage| stage == name).expect(name);
    let first_row = position("row_started");
    let last_row = names
        .iter()
        .rposition(|stage| stage == "row_finished")
        .expect("row_finished");
    let entry = position("budget_gesture");
    let exit = names
        .iter()
        .rposition(|stage| stage == "budget_gesture")
        .expect("жест выхода");
    assert!(entry < first_row, "замер начинается ПОСЛЕ входа в раздел: {names:?}");
    assert!(last_row < exit, "возврат в PHOTO — ПОСЛЕ последней строки: {names:?}");
    assert_eq!(
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт экрана")
            .state
            .mode,
        crate::camera_screen_source::ModeKind::Photo,
        "экран остался в нейтральном состоянии"
    );

    // Прибор и возможности устройства — в результате (пункт отчёта 14j).
    assert!(report["scopes"]["request"].is_object(), "{report}");
    assert_eq!(
        report["scopes"]["float32_controls"]["rounding_mode_rte"], true,
        "{report}"
    );

    // Приборы: A — продукт (0,8 мс), B — подавлены (0); UI: 1,5 мс; целиком: ISP + UI против мозаики 0,7 мс.
    let table = table_of(&result);
    assert_eq!(table.len(), 4, "три строки + база без нашей работы GPU: {table:?}");
    assert!(table[0].starts_with("color_scopes | 0.80 / 1.10 | 20.0"), "{table:?}");
    assert!(table[1].starts_with("ui_overlay | 1.50 / 1.80 | 20.0"), "{table:?}");
    assert!(table[2].starts_with("whole_product | "), "{table:?}");
    assert!(table[2].ends_with("| measured"), "{table:?}");
    assert!(
        table[3].starts_with("camera_without_product_gpu | 0.70 / 1.00 | 500.0 (абс.) | baseline"),
        "{table:?}"
    );
    // Рычаги вернулись: ничего не подавлено, рисовальщик ISP.
    let left = effective(&harness.control.recorded());
    assert_eq!(
        (left.painter, left.stages, left.suppression),
        (ViewfinderPainter::Isp, IspStageSet::ALL, GpuWorkSuppression::NONE)
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn color_scopes_row_applies_the_mask_only_to_window_b_and_leaves_ui_alone() {
    let config = surface_config(
        StageBudgetSurface::CalibrateScreen,
        vec![StageBudgetRowSetting::ColorScopes],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    let scopes_off = mask_of(SuppressibleGpuWork::ColorScopes);
    let masks = suppression_commands(&harness);
    // A B A B на смене положения + возврат к продукту перед выходом.
    assert_eq!(
        masks[..4],
        [
            GpuWorkSuppression::NONE,
            scopes_off,
            GpuWorkSuppression::NONE,
            scopes_off
        ],
        "{masks:?}"
    );
    assert_eq!(masks.last(), Some(&GpuWorkSuppression::NONE));
    assert!(
        masks
            .iter()
            .all(|mask| !mask.suppresses(SuppressibleGpuWork::UiOverlayDrawing))
    );
}

#[test]
fn a_suppression_the_render_does_not_apply_rejects_the_window_instead_of_measuring_it() {
    let config = surface_config(StageBudgetSurface::PhotoScreen, vec![StageBudgetRowSetting::UiOverlay]);
    let faults = SurfaceFaults {
        suppression_ignored: true,
        ..SurfaceFaults::default()
    };
    let (_harness, result) = run_surface(config, faults, Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "failed", "{result}");
    let table = table_of(&result);
    assert!(table[0].ends_with("| rejected: state_not_applied"), "{table:?}");
    assert!(!table[0].contains("measured"), "{table:?}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("state_not_applied"), "{error}");
    assert!(error.contains("ui_suppression_not_counted"), "{error}");
}

#[test]
fn display_row_holds_a_touch_at_the_start_of_every_a_window_and_accepts_120_hz() {
    let config = surface_config(
        StageBudgetSurface::PhotoScreen,
        vec![StageBudgetRowSetting::Display120Hz],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    let holds = stages_named(&harness, "budget_hold_touch");
    let windows: Vec<u64> = holds
        .iter()
        .map(|hold| hold["window"].as_u64().expect("окно"))
        .collect();
    // ABBA из трёх пар: A на окнах 0, 3 и 4 — стадия на каждое A и ни одной на B.
    assert_eq!(windows, [0, 3, 4], "{holds:?}");
    let presses: Vec<bool> = holds.iter().map(|hold| hold["needs_new_press"] == true).collect();
    assert_eq!(presses, [true, true, false], "окно 4 покрыто нажатием окна 3");
    // Окно = 300 мс (в нём уже разогрев): одиночное A держит 300 мс; первое из двух соседних — 600, и второе
    // справкой несёт то, что осталось держаться (300).
    let hold_ms: Vec<u64> = holds
        .iter()
        .map(|hold| hold["gesture"]["hold_ms"].as_u64().expect("hold_ms"))
        .collect();
    assert_eq!(hold_ms, [290, 590, 290], "окна минус запас 10 мс");
    let covered: Vec<u64> = holds
        .iter()
        .map(|hold| hold["windows_covered"].as_u64().expect("windows_covered"))
        .collect();
    assert_eq!(covered, [1, 2, 1]);
    let point = &holds[0]["gesture"];
    assert_eq!((point["x"].as_f64(), point["y"].as_f64()), (Some(0.03), Some(0.85)));
    assert_eq!(point["kind"], "long_press");
    // Платформе по очереди ушли 120 и 60 (A B A B), частота показа в окнах совпала.
    let requested = harness.shell.frame_rates.lock().expect("замок").clone();
    assert_eq!(requested[..4], [120.0, 60.0, 120.0, 60.0], "{requested:?}");
    let table = table_of(&result);
    assert!(table[0].starts_with("display_120hz | "), "{table:?}");
    assert!(table[0].ends_with("| measured"), "{table:?}");
    let notes = result["report"]["row_notes"].to_string();
    // Нажатие короче окна на запас, поэтому средняя A чуть ниже 120 (допуск 6 Гц).
    assert!(notes.contains("hz A 11") && notes.contains("B 60"), "{notes}");
}

#[test]
fn display_window_that_stays_at_60_is_rejected_with_its_own_reason() {
    let config = surface_config(
        StageBudgetSurface::PhotoScreen,
        vec![StageBudgetRowSetting::Display120Hz],
    );
    // Скрипт стадии выдачи слышит, но палец не держит: экран остаётся на 60 Гц, хотя запрошено 120.
    let (_harness, result) = run_surface(
        config,
        SurfaceFaults::default(),
        Driver::IgnoresHolds,
        PowerPlug::Unplugged,
    );

    assert_eq!(result["status"], "failed", "{result}");
    let table = table_of(&result);
    assert!(
        table[0].ends_with("| rejected: display_refresh_not_reached"),
        "{table:?}"
    );
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("display_refresh_not_reached"), "{error}");
    assert!(error.contains("60.0"), "в тексте видна реальная частота: {error}");
}

#[test]
fn calibrate_that_never_gets_its_swipe_fails_with_a_clear_reason_and_measures_nothing() {
    let config = surface_config(
        StageBudgetSurface::CalibrateScreen,
        vec![StageBudgetRowSetting::ColorScopes],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Silent, PowerPlug::Unplugged);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("calibrate_screen"), "{error}");
    assert!(error.contains("нужен Calibrate"), "{error}");
    assert!(stages_named(&harness, "row_started").is_empty(), "замер не начинали");
    // Жест входа был запрошен, выхода — нет: экран и не покидал PHOTO.
    assert_eq!(stages_named(&harness, "budget_gesture").len(), 1);
}

#[test]
fn external_power_still_refuses_on_a_screen_surface_and_the_screen_is_returned_to_photo() {
    let config = surface_config(
        StageBudgetSurface::CalibrateScreen,
        vec![StageBudgetRowSetting::ColorScopes],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Usb);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("ток не измерен"), "{error}");
    assert_eq!(
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт экрана")
            .state
            .mode,
        crate::camera_screen_source::ModeKind::Photo,
        "даже при отказе тока экран возвращён в PHOTO"
    );
    let gestures: Vec<String> = stages_named(&harness, "budget_gesture")
        .iter()
        .map(|stage| stage["step"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(gestures, ["modes_swipe_left", "modes_swipe_right"]);
}

/// Короткие окна: каждая из двенадцати пар строк идёт секунды, а не минуты.
fn quick_pair_config(rows: Vec<StageBudgetRowSetting>) -> CameraConfig {
    let mut config = surface_config(StageBudgetSurface::CalibrateScreen, rows);
    config.scenarios.power.ab.window_ms = 200;
    config.scenarios.power.ab.warmup_ms = 60;
    config.scenarios.power.ab.window_close_grace_ms = 40;
    config.scenarios.viewfinder.stage_budget.display_hold_margin_ms = 5;
    config.scenarios.viewfinder.stage_budget.display_refresh_tolerance_hz = 15.0;
    config
}

/// Состояние окна не зависит от того, какая строка шла до него: любая пара из новых строк подряд проходит, то есть в
/// первом окне второй строки рендер уже в её состоянии (иначе окно отвергнуто `state_not_applied`).
#[test]
fn the_state_of_the_previous_row_never_leaks_into_the_next_one() {
    use StageBudgetRowSetting::{ColorScopes, Display120Hz, UiOverlay, WholeProduct};
    let rows = [ColorScopes, UiOverlay, WholeProduct, Display120Hz];
    for first in rows {
        for second in rows {
            if first == second {
                continue;
            }
            let config = quick_pair_config(vec![first, second]);
            let (_harness, result) =
                run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);
            let table = table_of(&result);
            assert_eq!(
                result["status"],
                "passed",
                "{} затем {}: {result}",
                first.name(),
                second.name()
            );
            assert!(
                table.iter().take(2).all(|line| !line.contains("rejected")),
                "{} затем {}: {table:?}",
                first.name(),
                second.name()
            );
        }
    }
}

/// Строка с частотой оборвана на первом окне (палец не держится): жест выхода не раньше конца выданного удержания
/// плюс охранный срок, иначе два «пальца» от двух процессов `input` конфликтуют и свайп проглатывается.
#[test]
fn the_exit_gesture_waits_for_the_issued_hold_plus_the_guard_when_the_row_is_cut_short() {
    let mut config = surface_config(
        StageBudgetSurface::CalibrateScreen,
        vec![StageBudgetRowSetting::Display120Hz],
    );
    config.scenarios.viewfinder.stage_budget.display_hold_guard_ms = 1_500;
    let (harness, result, times) = run_surface_timed(
        config,
        SurfaceFaults::default(),
        Driver::IgnoresHolds,
        PowerPlug::Unplugged,
    );

    assert_eq!(result["status"], "failed", "строка отвергнута по частоте: {result}");
    let seen = |name: &str, nth_from_end: usize| {
        times
            .iter()
            .rev()
            .filter(|(stage, _)| stage == name)
            .nth(nth_from_end)
            .map(|(_, at)| *at)
            .unwrap_or_else(|| panic!("стадии {name} нет: {times:?}"))
    };
    let hold_at = seen("budget_hold_touch", 0);
    let exit_at = seen("budget_gesture", 0);
    let holds = stages_named(&harness, "budget_hold_touch");
    assert_eq!(holds.len(), 1, "строка оборвана на первом окне");
    let hold_ms = holds[0]["gesture"]["hold_ms"].as_u64().expect("hold_ms");
    let earliest = hold_at + Duration::from_millis(hold_ms + 1_500 - 100);
    assert!(
        exit_at >= earliest,
        "жест выхода выдан через {:?} после удержания, ждали не меньше {} мс",
        exit_at.saturating_duration_since(hold_at),
        hold_ms + 1_500
    );
    let gestures: Vec<String> = stages_named(&harness, "budget_gesture")
        .iter()
        .map(|stage| stage["step"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        gestures.last().map(String::as_str),
        Some("modes_swipe_right"),
        "выход всё же был"
    );
}

/// Что выдано окну, видно в подробностях: запрошенная частота и стадия удержания (hold_ms, новое ли нажатие).
#[test]
fn window_details_tell_what_was_requested_and_whether_a_hold_was_issued() {
    let config = surface_config(
        StageBudgetSurface::PhotoScreen,
        vec![StageBudgetRowSetting::Display120Hz],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);
    assert_eq!(result["status"], "passed", "{result}");
    let conditions: Vec<serde_json::Value> = stages_named(&harness, "detail_stage_budget")
        .iter()
        .flat_map(|line| line["items"].as_array().cloned().unwrap_or_default())
        .filter_map(|item| item.get("condition").cloned())
        .filter(|condition| !condition.is_null())
        .collect();
    assert_eq!(conditions.len(), 6, "{conditions:?}");
    for condition in conditions {
        let on = condition["stage"] == "on";
        assert_eq!(
            condition["issued"]["requested_hz"].as_f64(),
            Some(if on { 120.0 } else { 60.0 }),
            "{condition}"
        );
        assert_eq!(
            condition["issued"]["hold"].is_object(),
            on,
            "удержание только у окон A: {condition}"
        );
    }
}

/// Команды вида рендеру по порядку: основа и значения.
fn look_commands(harness: &Harness) -> Vec<ViewfinderCommand> {
    harness
        .control
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(
                command @ (ViewfinderCommand::SetLook(_) | ViewfinderCommand::SetToneBase(_)),
            ) => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn a_look_row_shows_each_look_on_render_reports_it_and_gives_the_product_look_back() {
    use look_contract::ToneBaseModel;
    for surface in [
        StageBudgetSurface::BareViewfinder,
        StageBudgetSurface::PhotoScreen,
        StageBudgetSurface::CalibrateScreen,
    ] {
        let config = surface_config(surface, vec![StageBudgetRowSetting::Look(LookRow::ToneBaseAces)]);
        let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

        assert_eq!(result["status"], "passed", "{}: {result}", surface.name());
        // Окна A B B A A B: рендер увидел основу ACES в A и legacy в B, и каждая смена положения слала обе команды.
        let bases: Vec<ToneBaseModel> = look_commands(&harness)
            .into_iter()
            .filter_map(|command| match command {
                ViewfinderCommand::SetToneBase(base) => Some(base),
                _ => None,
            })
            .collect();
        let product_base = look_commands(&harness)
            .iter()
            .rev()
            .find_map(|command| match command {
                ViewfinderCommand::SetToneBase(base) => Some(*base),
                _ => None,
            })
            .expect("последняя команда основы");
        assert_eq!(
            bases[..4],
            [
                ToneBaseModel::Aces,
                ToneBaseModel::Legacy,
                ToneBaseModel::Aces,
                ToneBaseModel::Legacy
            ],
            "{}: {bases:?}",
            surface.name()
        );
        assert_ne!(
            product_base,
            ToneBaseModel::Legacy,
            "после сценария вернулся вид продукта"
        );
        assert_ne!(product_base, ToneBaseModel::Aces, "{}", surface.name());
        // Отчёт говорит, какие виды были в окнах.
        let rows = result["report"]["look_rows"].as_array().expect("look_rows");
        assert_eq!(rows.len(), 1, "{result}");
        assert_eq!(rows[0]["row"], "look_tone_base_aces");
        assert_eq!(rows[0]["on"]["tone_base"], "aces", "{result}");
        assert_eq!(rows[0]["off"]["tone_base"], "legacy", "{result}");
        assert_ne!(
            rows[0]["on"]["recipe_fingerprint"],
            rows[0]["off"]["recipe_fingerprint"]
        );
        assert_eq!(rows[0]["on"]["wheels_active"], false);
        assert!(
            result["report"]["look"]["tone_base"].is_string(),
            "вид на входе: {result}"
        );
    }
}

#[test]
fn a_look_the_render_does_not_apply_rejects_the_window_as_state_not_applied() {
    let config = surface_config(
        StageBudgetSurface::PhotoScreen,
        vec![StageBudgetRowSetting::Look(LookRow::Wheels)],
    );
    let faults = SurfaceFaults {
        look_ignored: true,
        ..SurfaceFaults::default()
    };
    let (_harness, result) = run_surface(config, faults, Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "failed", "{result}");
    let table = table_of(&result);
    assert!(table[0].ends_with("| rejected: state_not_applied"), "{table:?}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("look_not_applied"),
        "{result}"
    );
}

#[test]
fn a_look_row_followed_by_a_plain_row_hands_the_look_back_before_measuring_it() {
    let config = surface_config(
        StageBudgetSurface::PhotoScreen,
        vec![
            StageBudgetRowSetting::Look(LookRow::Layer),
            StageBudgetRowSetting::UiOverlay,
        ],
    );
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    let table = table_of(&result);
    assert_eq!(table.len(), 2, "{table:?}");
    assert!(
        harness.router.camera_screen().capture_journal().is_empty(),
        "вид из сценария не сделал снимков"
    );
    // Последняя команда вида — вид продукта (нейтраль без библиотеки), а не нейтраль окна B строки `look_layer`.
    let last = look_commands(&harness).pop().expect("команды вида были");
    assert!(matches!(last, ViewfinderCommand::SetLook(_)), "{last:?}");
}

#[test]
fn the_calibrate_report_shows_zero_backdrop_blur_and_a_built_pyramid_fails_the_window() {
    let rows = vec![StageBudgetRowSetting::UiOverlay];
    let ok = run_surface(
        surface_config(StageBudgetSurface::CalibrateScreen, rows.clone()),
        SurfaceFaults::default(),
        Driver::Performs,
        PowerPlug::Unplugged,
    );
    assert_eq!(ok.1["status"], "passed", "{}", ok.1);
    assert_eq!(ok.1["report"]["backdrop_pyramid_rebuilds"], 0, "{}", ok.1);

    let faults = SurfaceFaults {
        backdrop_blur_built: true,
        ..SurfaceFaults::default()
    };
    let (_harness, bad) = run_surface(
        surface_config(StageBudgetSurface::CalibrateScreen, rows),
        faults,
        Driver::Performs,
        PowerPlug::Unplugged,
    );
    assert_eq!(bad["status"], "failed", "{bad}");
    assert!(
        bad["error"]
            .as_str()
            .unwrap_or_default()
            .contains("backdrop_blur_built"),
        "{bad}"
    );
}

#[test]
fn all_six_look_rows_on_the_calibrate_section_fit_one_logcat_line_and_name_distinct_windows() {
    let rows: Vec<StageBudgetRowSetting> = LookRow::ALL.into_iter().map(StageBudgetRowSetting::Look).collect();
    let config = surface_config(StageBudgetSurface::CalibrateScreen, rows);
    let (harness, result) = run_surface(config, SurfaceFaults::default(), Driver::Performs, PowerPlug::Unplugged);

    assert_eq!(result["status"], "passed", "{result}");
    let look_rows = result["report"]["look_rows"].as_array().expect("look_rows");
    assert_eq!(look_rows.len(), 6);
    for row in look_rows {
        let (on, off) = (&row["on"]["recipe_fingerprint"], &row["off"]["recipe_fingerprint"]);
        if row["row"] == "look_layer" {
            // Без библиотеки вид продукта — нейтраль: «продукт против нейтрали» окна не различает. Сценарий этого не
            // скрывает, а отдаёт одинаковые отпечатки, и разбор в `scripts/` падает с «строка мерила бы ноль».
            assert_eq!(on, off, "{row}");
        } else {
            assert_ne!(on, off, "{row}");
        }
    }
    // Колёса: окно A их включает, окно B — нет; тон — так же. Строка действительно меняет ветку шейдера.
    let wheels = look_rows
        .iter()
        .find(|row| row["row"] == "look_wheels")
        .expect("look_wheels");
    assert_eq!(
        (
            wheels["on"]["wheels_active"].as_bool(),
            wheels["off"]["wheels_active"].as_bool()
        ),
        (Some(true), Some(false))
    );
    let tone = look_rows
        .iter()
        .find(|row| row["row"] == "look_tone")
        .expect("look_tone");
    assert_eq!(
        (
            tone["on"]["tone_active"].as_bool(),
            tone["off"]["tone_active"].as_bool()
        ),
        (Some(true), Some(false))
    );
    assert_fits_logcat(&harness, &result);
}
