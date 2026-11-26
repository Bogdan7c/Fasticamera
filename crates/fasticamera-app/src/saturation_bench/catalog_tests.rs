use super::*;

#[test]
fn catalog_has_every_documented_workload_exactly_once() {
    let expected = [
        "viewfinder",
        "scopes_waveform",
        "scopes_parade",
        "scopes_vectorscope_ycbcr",
        "scopes_vectorscope_oklab",
        "scopes_parade_gray",
        "scopes_vectorscope_gray",
        "compositor",
        "display_photo",
        "display_photo_recompose",
        "display_calibrate",
        "display_calibrate_recompose",
        "headroom_v4",
        "still",
    ];
    // Первые 14 записей каталога — это и есть группа baseline, порядок прогона по умолчанию не менялся.
    assert_eq!(all_names()[..expected.len()], expected);
    assert_eq!(
        baseline_names(),
        expected,
        "умолчание списка работ — ровно 14 работ 16a в том же порядке"
    );
    for name in expected {
        assert_eq!(find_entry(name).expect("имя из каталога").name, name);
    }
}

#[test]
fn ablation_groups_list_exactly_the_documented_workloads_in_catalog_order() {
    let names = |group| group_entries(group).iter().map(|entry| entry.name).collect::<Vec<_>>();
    assert_eq!(
        names(WorkloadGroup::UiAblation),
        [
            "display_calibrate_shapes_only",
            "display_calibrate_text_only",
            "display_calibrate_images_only",
            "display_photo_glass_only",
            "display_photo_no_glass",
        ]
    );
    assert_eq!(
        names(WorkloadGroup::IspAblation),
        [
            "viewfinder_awb_never",
            "viewfinder_awb_every_frame",
            "viewfinder_without_levels",
            "viewfinder_without_lens_shading",
            "viewfinder_without_white_balance",
            "viewfinder_without_demosaic",
            "viewfinder_without_color",
            "viewfinder_without_gamut_compression",
            "viewfinder_without_tone",
            "viewfinder_without_look_color",
            "viewfinder_without_look_lut",
            "viewfinder_without_output_gamut_compression",
            "viewfinder_without_oetf",
            "viewfinder_minimal",
        ]
    );
}

#[test]
fn every_catalog_name_is_unique_belongs_to_one_group_and_is_not_a_group_name() {
    let names = all_names();
    let unique: std::collections::HashSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "имена не повторяются");
    let grouped: usize = WorkloadGroup::ALL.iter().map(|group| group_entries(*group).len()).sum();
    assert_eq!(grouped, entries().count(), "каждая работа ровно в одной группе");
    for name in &names {
        assert!(
            WorkloadGroup::by_name(name).is_none(),
            "имя работы {name} совпало с именем группы"
        );
    }
}

#[test]
fn expanding_a_group_gives_its_workloads_a_work_name_gives_one_and_a_typo_is_an_error_listing_groups() {
    let group: Vec<&str> = expand_name("ui_ablation")
        .expect("группа")
        .iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(group.len(), 5);
    assert_eq!(group[0], "display_calibrate_shapes_only");
    let single = expand_name("viewfinder_minimal").expect("работа");
    assert_eq!(single.len(), 1);
    let error = expand_name("ui_ablations").expect_err("опечатка в имени группы");
    assert!(
        error.available.contains("isp_ablation") && error.available.contains("baseline"),
        "{error}"
    );
}

#[test]
fn ablation_entries_map_to_the_right_screen_filter_and_isp_variant() {
    assert_eq!(
        find_entry("display_photo_glass_only").unwrap().kind,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::GlassOnly,
        }
    );
    assert_eq!(
        find_entry("display_photo_no_glass").unwrap().kind,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::NoGlass,
        }
    );
    assert_eq!(
        find_entry("viewfinder_without_oetf").unwrap().kind,
        WorkloadKind::Viewfinder {
            variant: ViewfinderVariant::Without {
                stage: AblatedStage::Oetf
            }
        }
    );
    assert_eq!(
        find_entry("viewfinder").unwrap().kind,
        WorkloadKind::Viewfinder {
            variant: ViewfinderVariant::Product
        }
    );
}

#[test]
fn unknown_name_is_an_error_that_lists_the_catalog() {
    let error = find_entry("viewfinderr").expect_err("опечатка");
    assert_eq!(error.name, "viewfinderr");
    assert!(error.available.contains("display_calibrate_recompose"), "{error}");
}

#[test]
fn recompose_variants_differ_only_by_backdrop_rhythm() {
    let fresh = find_entry("display_photo").expect("есть").kind;
    let held = find_entry("display_photo_recompose").expect("есть").kind;
    assert_eq!(
        fresh,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::All,
        }
    );
    assert_eq!(
        held,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::SameFrame,
            filter: DrawFilter::All,
        }
    );
}

#[test]
fn tuning_group_has_unique_variants_and_does_not_extend_the_default_lists() {
    let entries = group_entries(WorkloadGroup::IspTuning);
    assert!(entries.len() > 10, "группа опытов не пуста");
    let kinds: std::collections::HashSet<_> = entries.iter().map(|entry| format!("{:?}", entry.kind)).collect();
    assert_eq!(
        kinds.len(),
        entries.len(),
        "две записи группы с одинаковыми настройками"
    );
    assert!(
        baseline_names().iter().all(|name| !name.contains("wg_")),
        "умолчание списка работ не расширено опытами"
    );
    assert!(matches!(
        find_entry("viewfinder_never_wg_4x32").unwrap().kind,
        WorkloadKind::Viewfinder {
            variant: ViewfinderVariant::Tuned(_)
        }
    ));
}
