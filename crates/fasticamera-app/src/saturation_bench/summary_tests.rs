use super::*;
use crate::power_measurement::ThermalSnapshot;
use crate::saturation_bench::report::{
    EndReasonName, FailureKind, Measurement, NotRunReason, RenderActivity, RowOutcome, WorkloadFailure,
};

fn row(name: &'static str, outcome: RowOutcome) -> WorkloadRow {
    WorkloadRow {
        name,
        index: 1,
        total: 1,
        outcome,
        scene: None,
        ui: None,
        digest: None,
        extras: crate::saturation_bench::extras::WorkloadExtras::default(),
        thermal_start: None,
        thermal_end: None,
        started_at_ms: None,
    }
}

fn measurement(fps: f64) -> Measurement {
    Measurement {
        end_reason: EndReasonName::Completed,
        warmup_frames: 1,
        measured_frames: 10,
        drained_frames: 0,
        window_ms: 1_000.0,
        frames_per_second: Some(fps),
        frame_cost_ms: Some(1_000.0 / fps),
        passes: Vec::new(),
        unmeasured_pass_frames: 0,
        mismatched_pass_frames: 0,
        timeline_fps: vec![fps],
        cost: None,
    }
}

#[test]
fn three_significant_digits_at_every_magnitude() {
    assert_eq!(format_sig3(397.4), "397");
    assert_eq!(format_sig3(3837.2), "3837", "целая часть не режется");
    assert_eq!(format_sig3(45.67), "45.7");
    assert_eq!(format_sig3(7.891), "7.89");
    assert_eq!(format_sig3(0.2614), "0.261");
    assert_eq!(format_sig3(0.0), "0");
}

#[test]
fn table_row_is_short_and_carries_name_fps_cost_and_verdict() {
    let measured = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(200.0),
        },
    );
    assert_eq!(table_row(&measured), "viewfinder|200|5.00|ok");
    let failed = row(
        "still",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::BuildFailed, "очень длинная причина ".repeat(100)),
        },
    );
    assert_eq!(
        table_row(&failed),
        "still|-|-|build_failed",
        "причина — в подробностях, не в таблице"
    );
    let skipped = row(
        "compositor",
        RowOutcome::NotRun {
            reason: NotRunReason::Cancelled,
        },
    );
    assert_eq!(table_row(&skipped), "compositor|-|-|not_run");
}

#[test]
fn summary_names_the_sensor_source_the_look_and_what_render_did() {
    let header = SummaryHeader {
        validation: ValidationState::Disabled,
        sensor: SensorSource::Fallback {
            reason: "движок камеры недоступен".to_owned(),
        },
        look: LookSummary {
            preset: "As Eye".to_owned(),
            tone_base: look_contract::ToneBaseModel::Agx,
        },
        camera: CameraSilence::ClosedByScenario,
        requested: vec!["viewfinder"],
        warmup_ms: 3_000,
        measure_ms: 10_000,
        frames_in_flight: 2,
        render_tolerance_frames: 5,
    };
    let run = RunSummary {
        rows: vec![row(
            "viewfinder",
            RowOutcome::Measured {
                measurement: measurement(100.0),
            },
        )],
        cancelled: false,
    };
    let silence = RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity {
            frames_presented: 50,
            ..RenderActivity::default()
        },
        delta: RenderActivity {
            frames_presented: 50,
            ..RenderActivity::default()
        },
    };
    let summary = build_summary(&header, &run, &silence);
    assert_eq!(summary["table"][0], "viewfinder|100|10.0|ok");
    assert_eq!(summary["sensor"]["source"], "fallback");
    assert_eq!(summary["look"]["preset"], "As Eye");
    assert_eq!(summary["look"]["tone_base"], "agx");
    assert_eq!(summary["camera"], "closed_by_scenario");
    assert_eq!(summary["validation"], "disabled");
    assert_eq!(
        summary["render"]["delta"]["frames_presented"], 50,
        "показы рендера — факт в результате"
    );
    assert_eq!(summary["measured"], 1);
    assert_eq!(summary["requested_count"], 1);
    assert_eq!(
        summary["requested_missing"],
        serde_json::json!([]),
        "у каждой работы есть строка таблицы"
    );
    assert_eq!(summary["render_tolerance_frames"], 5);
}

#[test]
fn detail_item_keeps_the_typed_failure_and_thermal_words() {
    let mut failed = row(
        "still",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::ObjectsNotReleased, "Image 3->5"),
        },
    );
    failed.thermal_start = Some(ThermalSnapshot {
        status: Some(shell_contract::thermal::ThermalStatus::Light),
        headroom: Some(0.5),
        error: None,
    });
    failed.thermal_end = Some(ThermalSnapshot {
        status: None,
        headroom: None,
        error: Some("нет".to_owned()),
    });
    let item = detail_item(&failed);
    assert_eq!(item["outcome"]["failure"]["kind"], "objects_not_released");
    assert!(
        item["outcome"]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("Image 3->5")
    );
    assert_eq!(item["thermal_words"]["start"], "light");
    assert_eq!(item["thermal_words"]["end"], "unknown");
}

fn digest_of(combined: &str) -> DigestOutcome {
    DigestOutcome::Computed {
        combined: combined.to_owned(),
        parts: Vec::new(),
        frame_note: None,
    }
}

fn header_for(requested: Vec<&'static str>) -> SummaryHeader {
    SummaryHeader {
        validation: ValidationState::Disabled,
        sensor: SensorSource::Fallback {
            reason: "тест".to_owned(),
        },
        look: LookSummary {
            preset: "As Eye".to_owned(),
            tone_base: look_contract::ToneBaseModel::Agx,
        },
        camera: CameraSilence::ClosedByScenario,
        requested,
        warmup_ms: 3_000,
        measure_ms: 10_000,
        frames_in_flight: 2,
        render_tolerance_frames: 5,
    }
}

fn quiet_silence() -> RenderSilence {
    RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity::default(),
        delta: RenderActivity::default(),
    }
}

#[test]
fn digest_row_shows_the_combined_digest_a_dash_when_unavailable_and_leaves_the_table_untouched() {
    let mut with_digest = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(200.0),
        },
    );
    with_digest.digest = Some(digest_of("0123456789abcdef"));
    assert_eq!(digest_row(&with_digest), "viewfinder|0123456789abcdef");
    assert_eq!(
        table_row(&with_digest),
        "viewfinder|200|5.00|ok",
        "формат таблицы не менялся"
    );

    let mut unavailable = with_digest.clone();
    unavailable.digest = Some(DigestOutcome::Unavailable {
        reason: "нет доступа".to_owned(),
    });
    assert_eq!(digest_row(&unavailable), "viewfinder|-");
    let mut absent = with_digest;
    absent.digest = None;
    assert_eq!(digest_row(&absent), "viewfinder|-");
}

#[test]
fn summary_carries_digests_for_the_baseline_but_moves_them_to_detail_when_the_line_would_overflow() {
    let names: Vec<&'static str> = crate::saturation_bench::catalog::baseline_names()
        .into_iter()
        .map(|name| &*Box::leak(name.into_boxed_str()))
        .collect();
    let rows_for = |names: &[&'static str]| -> Vec<WorkloadRow> {
        names
            .iter()
            .map(|name| {
                let mut one = row(
                    name,
                    RowOutcome::Measured {
                        measurement: measurement(100.0),
                    },
                );
                one.digest = Some(digest_of("0123456789abcdef"));
                one
            })
            .collect()
    };
    let baseline = RunSummary {
        rows: rows_for(&names),
        cancelled: false,
    };
    let summary = build_summary(&header_for(names.clone()), &baseline, &quiet_silence());
    assert_eq!(
        summary["digests"].as_array().map(Vec::len),
        Some(14),
        "14 работ: дайджесты в строке результата"
    );
    assert_eq!(summary["digests"][0], "viewfinder|0123456789abcdef");
    assert!(summary.get("digests_in_detail_only").is_none());
    assert!(serde_json::to_string(&summary).unwrap().len() <= SUMMARY_BUDGET_WITH_DIGESTS_BYTES);

    let all: Vec<&'static str> = crate::saturation_bench::catalog::entries()
        .map(|entry| entry.name)
        .collect();
    let everything = RunSummary {
        rows: rows_for(&all),
        cancelled: false,
    };
    let all_count = all.len();
    let big = build_summary(&header_for(all), &everything, &quiet_silence());
    assert!(
        big.get("digests").is_none(),
        "весь каталог не влезает: дайджестов в строке нет"
    );
    assert_eq!(big["digests_in_detail_only"], true);
    assert_eq!(
        big["table"].as_array().map(Vec::len),
        Some(all_count),
        "таблица на месте"
    );
}

#[test]
fn detail_item_carries_the_digest_parts_and_the_frame_note() {
    let mut one = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(100.0),
        },
    );
    one.digest = Some(DigestOutcome::Computed {
        combined: "0123456789abcdef".to_owned(),
        parts: vec![crate::saturation_bench::digest_report::DigestPartRow {
            label: "image",
            digest: "00000000000000ab".to_owned(),
        }],
        frame_note: Some("last_frame=awb_statistics".to_owned()),
    });
    let item = detail_item(&one);
    assert_eq!(item["digest"]["state"], "computed");
    assert_eq!(item["digest"]["combined"], "0123456789abcdef");
    assert_eq!(item["digest"]["parts"][0]["label"], "image");
    assert_eq!(item["digest"]["frame_note"], "last_frame=awb_statistics");
}

/// Строка работы с измерением и дополнительными фактами.
fn row_with_extras(name: &'static str, extras: crate::saturation_bench::extras::WorkloadExtras) -> WorkloadRow {
    let mut one = row(
        name,
        RowOutcome::Measured {
            measurement: measurement(100.0),
        },
    );
    one.extras = extras;
    one
}

/// Паритет, который прошёл: измерено, худшая ошибка 4,2e-6.
fn passed_parity() -> crate::saturation_bench::parity_report::ParityOutcome {
    use crate::saturation_bench::parity_report::{ParityOutcome, ParityRecord};
    ParityOutcome::Measured(ParityRecord {
        allowance: "standard",
        tolerance: 1.0e-5,
        worst_math_excess: 4.2e-6,
        worst_max_abs: 4.8e-4,
        worst_scene: "hue_sweep",
        scenes_measured: 7,
        fp16_math_excess: 4.2e-6,
        preferred_math_excess: 3.0e-6,
        preferred_format: "rgb10a2",
        violations: Vec::new(),
    })
}

#[test]
fn parity_rows_exist_for_every_parity_work_even_one_that_never_measured() {
    let run = RunSummary {
        rows: vec![
            row_with_extras(
                "look_parity_agx_bands",
                crate::saturation_bench::extras::WorkloadExtras {
                    parity: Some(passed_parity()),
                    ..Default::default()
                },
            ),
            // Работа паритета без числа (измерение не дошло): строка есть и честно говорит «unavailable».
            row_with_extras("look_parity_agx_tone", Default::default()),
            // Не работа паритета: в строках паритета её нет.
            row_with_extras("viewfinder", Default::default()),
        ],
        cancelled: false,
    };
    assert_eq!(
        parity_rows(&run),
        [
            "look_parity_agx_bands|4.20e-6|4.80e-4|1e-5|pass".to_owned(),
            "look_parity_agx_tone|-|-|-|unavailable".to_owned(),
        ]
    );
    let summary = build_summary(
        &header_for(vec!["look_parity_agx_bands", "look_parity_agx_tone", "viewfinder"]),
        &run,
        &quiet_silence(),
    );
    assert_eq!(summary["parity"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        summary["parity_expected"], 2,
        "число обязательных строк паритета — по списку запрошенных работ, не по пришедшим строкам"
    );
}

#[test]
fn expected_counts_come_from_the_requested_list_so_a_dropped_row_cannot_hide() {
    // Запрошены три работы паритета и два прибора, а в итоге пришла одна строка паритета и ни одной строки приборов.
    let run = RunSummary {
        rows: vec![row_with_extras(
            "look_parity_agx_bands",
            crate::saturation_bench::extras::WorkloadExtras {
                parity: Some(passed_parity()),
                ..Default::default()
            },
        )],
        cancelled: false,
    };
    let requested = vec![
        "look_parity_agx_bands",
        "look_parity_agx_tone",
        "look_parity_aces_bands",
        "scopes_waveform",
        "scopes_parade",
    ];
    let summary = build_summary(&header_for(requested), &run, &quiet_silence());
    assert_eq!(summary["parity_expected"], 3);
    assert_eq!(
        summary["parity"].as_array().map(Vec::len),
        Some(1),
        "пришла одна строка"
    );
    assert_eq!(summary["scope_pictures_expected"], 2);
    assert_eq!(summary["scope_pictures"].as_array().map(Vec::len), Some(0));
}

#[test]
fn scope_picture_rows_report_the_real_size_per_scopes_work_and_a_dash_for_a_missing_one() {
    use crate::saturation_bench::extras::{PictureSizePx, WorkloadExtras};
    let run = RunSummary {
        rows: vec![
            row_with_extras(
                "scopes_waveform",
                WorkloadExtras {
                    scope_picture: Some(PictureSizePx {
                        width: 720,
                        height: 480,
                    }),
                    ..Default::default()
                },
            ),
            row_with_extras("scopes_parade", Default::default()),
            row_with_extras("viewfinder", Default::default()),
        ],
        cancelled: false,
    };
    assert_eq!(scope_picture_rows(&run), ["scopes_waveform|720x480", "scopes_parade|-"]);
    let summary = build_summary(
        &header_for(vec!["scopes_waveform", "scopes_parade", "viewfinder"]),
        &run,
        &quiet_silence(),
    );
    assert_eq!(summary["scope_pictures"][0], "scopes_waveform|720x480");
    // У прогона без приборов и паритета этих полей нет совсем.
    let plain = RunSummary {
        rows: vec![row_with_extras("viewfinder", Default::default())],
        cancelled: false,
    };
    let plain_summary = build_summary(&header_for(vec!["viewfinder"]), &plain, &quiet_silence());
    assert!(plain_summary.get("parity").is_none() && plain_summary.get("scope_pictures").is_none());
}

#[test]
fn full_look_parity_and_scopes_groups_fit_the_result_line_with_their_parity_and_picture_rows() {
    use crate::saturation_bench::catalog::{WorkloadGroup, group_entries};
    use crate::saturation_bench::extras::{PictureSizePx, WorkloadExtras};
    for group in [WorkloadGroup::LookParity, WorkloadGroup::ScopesAblation] {
        let names: Vec<&'static str> = group_entries(group).iter().map(|entry| entry.name).collect();
        let rows = names
            .iter()
            .map(|name| {
                row_with_extras(
                    name,
                    WorkloadExtras {
                        parity: name.starts_with("look_parity").then(passed_parity),
                        scope_picture: name.starts_with("scopes").then_some(PictureSizePx {
                            width: 720,
                            height: 480,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let run = RunSummary { rows, cancelled: false };
        let summary = build_summary(&header_for(names.clone()), &run, &quiet_silence());
        let bytes = serde_json::to_string(&summary).unwrap().len();
        assert!(
            bytes <= SUMMARY_BUDGET_WITH_DIGESTS_BYTES + 1_000,
            "{group:?}: сводка {bytes} байт не влезает в одну запись logcat (~4000)"
        );
        let carried = summary.get("parity").or_else(|| summary.get("scope_pictures"));
        assert_eq!(
            carried.and_then(|rows| rows.as_array()).map(Vec::len),
            Some(names.len())
        );
    }
}

/// Шапка с самыми длинными значениями: длинное кириллическое имя пресета (40 символов — предел `PresetName`), причина
/// запасного сенсора на весь предел строки сводки (160 байт) и счётчики рендера в 10 цифр (10¹⁰ кадров — больше трёх
/// лет при 60 Гц: за прогон стенда процесса столько не бывает).
fn worst_case_header(requested: Vec<&'static str>) -> SummaryHeader {
    SummaryHeader {
        validation: ValidationState::Disabled,
        sensor: SensorSource::Fallback {
            reason: "ж".repeat(80)
        },
        look: LookSummary {
            preset: "Ж".repeat(40),
            tone_base: look_contract::ToneBaseModel::Filmic,
        },
        requested,
        ..header_for(Vec::new())
    }
}

fn worst_case_silence() -> RenderSilence {
    let ten_digits = 9_999_999_999;
    let huge = RenderActivity {
        frames_presented: ten_digits,
        isp_frames: ten_digits,
        recomposed_frames: ten_digits,
        ui_frames_drawn: ten_digits,
    };
    RenderSilence::Observed {
        before: huge,
        after: huge,
        delta: huge,
    }
}

/// Строки прогона с самыми длинными ячейками: провал с длинным словом, паритет без числа, прибор с размером.
fn worst_case_rows(names: &[&'static str]) -> RunSummary {
    use crate::saturation_bench::extras::{PictureSizePx, WorkloadExtras};
    use crate::saturation_bench::parity_report::ParityOutcome;
    let rows = names
        .iter()
        .map(|name| {
            row_with_extras(
                name,
                WorkloadExtras {
                    parity: name.starts_with("look_parity").then(|| ParityOutcome::Unavailable {
                        reason: "тест".to_owned(),
                    }),
                    scope_picture: name.starts_with("scopes").then_some(PictureSizePx {
                        width: 2048,
                        height: 2048,
                    }),
                    ..Default::default()
                },
            )
        })
        .collect();
    RunSummary { rows, cancelled: false }
}

#[test]
fn worst_case_header_fits_its_reserve() {
    let summary = build_summary(
        &worst_case_header(Vec::new()),
        &worst_case_rows(&[]),
        &worst_case_silence(),
    );
    let bytes = serde_json::to_string(&summary).unwrap().len();
    assert!(
        bytes <= SUMMARY_HEADER_RESERVE_BYTES,
        "шапка {bytes} байт больше запаса {SUMMARY_HEADER_RESERVE_BYTES}: оценка до запуска стала бы врать"
    );
}

#[test]
fn the_estimate_is_never_below_the_real_summary_of_any_group() {
    use crate::saturation_bench::catalog::{WorkloadGroup, group_entries};
    for group in WorkloadGroup::ALL {
        let entries: Vec<&CatalogEntry> = group_entries(group).to_vec();
        let names: Vec<&'static str> = entries.iter().map(|entry| entry.name).collect();
        let summary = summary_without_digests(
            &worst_case_header(names.clone()),
            &worst_case_rows(&names),
            &worst_case_silence(),
        );
        let real = serde_json::to_string(&summary).unwrap().len();
        let estimate = worst_case_summary_bytes(&entries);
        assert!(
            real <= estimate,
            "{group:?}: настоящая сводка {real} байт больше оценки {estimate}"
        );
    }
}

#[test]
fn each_14o_group_fits_one_result_line_but_three_groups_at_once_are_refused_before_the_run() {
    use crate::saturation_bench::catalog::{WorkloadGroup, group_entries};
    let budget = summary_budget_bytes();
    let mut together: Vec<&CatalogEntry> = Vec::new();
    for group in [
        WorkloadGroup::Baseline,
        WorkloadGroup::LookCost,
        WorkloadGroup::LookParity,
    ] {
        let entries: Vec<&CatalogEntry> = group_entries(group).to_vec();
        let estimate = worst_case_summary_bytes(&entries);
        assert!(
            estimate <= budget,
            "{group:?}: оценка {estimate} байт больше бюджета {budget} — группу не прогнать"
        );
        together.extend(entries);
    }
    // Ровно этот список на телефоне в 14o дал «отчёт 4532 байт не помещается» после десяти минут замера.
    assert!(worst_case_summary_bytes(&together) > budget);
}

#[test]
fn a_requested_work_without_a_table_row_is_named_in_the_summary() {
    let header = header_for(vec!["viewfinder", "compositor"]);
    let run = RunSummary {
        rows: vec![row(
            "viewfinder",
            RowOutcome::Measured {
                measurement: measurement(100.0),
            },
        )],
        cancelled: false,
    };
    let summary = build_summary(&header, &run, &quiet_silence());
    assert_eq!(summary["requested_count"], 2);
    assert_eq!(summary["requested_missing"], serde_json::json!(["compositor"]));
}
