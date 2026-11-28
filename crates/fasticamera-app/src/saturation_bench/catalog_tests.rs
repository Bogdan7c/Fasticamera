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
            "display_photo_shapes_only",
            "display_photo_text_only",
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
        find_entry("display_photo_shapes_only").unwrap().kind,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::ShapesOnly,
        }
    );
    assert_eq!(
        find_entry("display_photo_text_only").unwrap().kind,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::TextOnly,
        }
    );
    // Группы стекла `glass_ablation` больше нет (14p): после 14m и 14p она мерила бы пустоту.
    assert!(expand_name("glass_ablation").is_err());
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

#[test]
fn look_cost_group_lists_the_ten_documented_looks_off_reference_before_its_variant() {
    use crate::saturation_bench::compare_pairs::{PAIRS, reference_of};
    let names: Vec<&str> = group_entries(WorkloadGroup::LookCost)
        .iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(
        names,
        [
            "look_neutral",
            "look_product",
            "look_tone_base_legacy",
            "look_tone_base_agx",
            "look_tone_base_aces",
            "look_tone_base_filmic",
            "look_without_wheels",
            "look_wheels_sample",
            "look_without_tone",
            "look_tone_sample",
            "look_without_primaries",
            "look_primaries_sample",
        ]
    );
    // Каждая пара «вкл против выкл» этой группы: оба конца в группе, эталон стоит РАНЬШЕ варианта.
    let look_pairs: Vec<_> = PAIRS.iter().filter(|pair| pair.variant.starts_with("look_")).collect();
    assert_eq!(
        look_pairs.len(),
        7,
        "agx/aces/filmic против legacy, product/neutral, колёса, тон, полоски"
    );
    for pair in look_pairs {
        let variant = names
            .iter()
            .position(|name| *name == pair.variant)
            .expect("вариант в группе");
        let reference = names
            .iter()
            .position(|name| *name == pair.reference)
            .expect("эталон в группе");
        assert!(
            reference < variant,
            "{} должен идти раньше {}",
            pair.reference,
            pair.variant
        );
        assert_eq!(
            reference_of(pair.reference),
            None,
            "эталон {} сам не вариант",
            pair.reference
        );
    }
}

#[test]
fn look_cost_entries_carry_the_measured_look_named_like_the_entry() {
    for entry in group_entries(WorkloadGroup::LookCost) {
        let WorkloadKind::ViewfinderLook { look } = entry.kind else {
            panic!("{}: вид работы {:?}", entry.name, entry.kind);
        };
        let expected = format!("look_{}", look.0.name());
        assert_eq!(entry.name, expected, "имя работы совпадает с именем вида замера");
    }
}

#[test]
fn look_parity_group_is_every_base_times_every_case_in_base_order() {
    use look_contract::ToneBaseModel;
    let entries = group_entries(WorkloadGroup::LookParity);
    assert_eq!(entries.len(), 16);
    let mut combinations = std::collections::HashSet::new();
    for entry in &entries {
        let WorkloadKind::LookParity { look } = entry.kind else {
            panic!("{}: вид работы {:?}", entry.name, entry.kind);
        };
        assert!(
            combinations.insert((look.base, look.case)),
            "{}: повтор случая",
            entry.name
        );
    }
    let bases: Vec<_> = entries
        .iter()
        .step_by(4)
        .map(|entry| match entry.kind {
            WorkloadKind::LookParity { look } => look.base,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        bases,
        [
            ToneBaseModel::Agx,
            ToneBaseModel::Aces,
            ToneBaseModel::Filmic,
            ToneBaseModel::Legacy
        ]
    );
    assert_eq!(entries[0].name, "look_parity_agx_product");
    assert_eq!(entries[15].name, "look_parity_legacy_tone");
}

#[test]
fn look_parity_primaries_group_is_the_four_bases_on_the_primaries_sample_with_parity_names() {
    use crate::saturation_bench::look_variant::ParityCase;
    use look_contract::ToneBaseModel;
    let entries = group_entries(WorkloadGroup::LookParityPrimaries);
    let names: Vec<&str> = entries.iter().map(|entry| entry.name).collect();
    assert_eq!(
        names,
        [
            "look_parity_agx_primaries",
            "look_parity_aces_primaries",
            "look_parity_filmic_primaries",
            "look_parity_legacy_primaries",
        ]
    );
    let bases: Vec<_> = entries
        .iter()
        .map(|entry| match entry.kind {
            WorkloadKind::LookParity { look } => {
                assert_eq!(look.case, ParityCase::PrimariesSample, "{}", entry.name);
                look.base
            }
            other => panic!("{}: вид работы {other:?}", entry.name),
        })
        .collect();
    assert_eq!(
        bases,
        [
            ToneBaseModel::Agx,
            ToneBaseModel::Aces,
            ToneBaseModel::Filmic,
            ToneBaseModel::Legacy
        ]
    );
    assert_eq!(
        WorkloadGroup::by_name("look_parity_primaries"),
        Some(WorkloadGroup::LookParityPrimaries)
    );
    assert_eq!(expand_name("look_parity_primaries").unwrap().len(), 4);
    assert!(
        group_entries(WorkloadGroup::LookParity)
            .iter()
            .all(|entry| !entry.name.ends_with("_primaries")),
        "прежняя группа паритета не растёт"
    );
}

#[test]
fn look_groups_are_registered_by_name_and_do_not_extend_the_default_list() {
    assert_eq!(WorkloadGroup::by_name("look_cost"), Some(WorkloadGroup::LookCost));
    assert_eq!(WorkloadGroup::by_name("look_parity"), Some(WorkloadGroup::LookParity));
    assert_eq!(expand_name("look_cost").unwrap().len(), 12);
    assert_eq!(expand_name("look_parity").unwrap().len(), 16);
    assert!(baseline_names().iter().all(|name| !name.starts_with("look_")));
}
