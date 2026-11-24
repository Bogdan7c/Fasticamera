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
    assert_eq!(all_names(), expected);
    for name in expected {
        assert_eq!(find_entry(name).expect("имя из каталога").name, name);
    }
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
            rhythm: BackdropRhythm::NewFrameEveryFrame
        }
    );
    assert_eq!(
        held,
        WorkloadKind::Display {
            screen: DisplayScreen::Photo,
            rhythm: BackdropRhythm::SameFrame
        }
    );
}
