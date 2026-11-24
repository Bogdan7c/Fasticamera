use camera_contract::white::WhiteSourceId;

use super::*;

#[test]
fn the_product_registry_holds_exactly_the_phone_source_built_from_the_render_config() {
    let registry = product_white_sources(&RenderLoopConfig::default());

    assert_eq!(registry.len(), 1);
    let sources = registry.instantiate().expect("сборка источников");
    assert_eq!(sources[0].id(), WhiteSourceId::PHONE);
}

#[test]
fn a_broken_phone_config_leaves_the_registry_empty_instead_of_crashing_the_app() {
    let mut config = RenderLoopConfig::default();
    config.viewfinder.white_balance.phone.stable_frames = 0;

    let registry = product_white_sources(&config);

    assert!(registry.is_empty(), "заявки получат «источника нет», приложение живёт");
}
