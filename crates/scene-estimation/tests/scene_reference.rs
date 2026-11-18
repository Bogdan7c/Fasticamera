//! Оценка света по независимой фикстуре: ColorChecker под разными источниками.
//!
//! Фикстура `test-assets/scene-estimation/awb-scenes.*` считана НЕ нашим кодом: спектры образцов и света
//! (colour-science), отклик камеры по DNG (colour-hdri), ожидаемые CCT/Duv света по Робертсону. Генератор —
//! `tools/isp-reference/make_awb_scene_fixtures.py`. Здесь проверяется, что наши таблица локуса, оценщик и
//! контроллер возвращают тот свет, под которым снята сцена.
//!
//! Измеренные числа (сессия 08, режим окна по моде, сцены `checker_*` и `dominant_green_*`, 6 источников):
//! |Δ майред| ≤ 1,35 (худший — чёрное тело 2800 К), |Δ Duv| ≤ 0,0013; допуски ниже — с запасом ×2.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

mod support;

use isp_model::color::temperature::Kelvin;
use scene_estimation::config::{EstimatorConfig, LocusTableConfig, ReductionConfig, SceneEstimationConfig};
use scene_estimation::controller::{IngestOutcome, WhiteBalanceController};
use scene_estimation::estimate::{EstimateOutcome, IlluminantEstimate};
use scene_estimation::estimator::estimate_scene_light;
use scene_estimation::executor::{CpuWhiteBalanceEstimator, EstimationRequest, FrameTag, WhiteBalanceEstimator};
use scene_estimation::locus_table::CameraLocusTable;
use support::{Scene, load_scenes};

/// Допуск по майредам: ≈ один шаг, различимый глазом, пополам (измерено ≤ 1,35).
const MIRED_TOLERANCE: f64 = 3.0;
/// Допуск по Duv (измерено ≤ 0,0013).
const DUV_TOLERANCE: f64 = 0.003;

struct Rig {
    scenes: Vec<Scene>,
    table: CameraLocusTable,
    config: SceneEstimationConfig,
}

fn rig() -> Rig {
    let (manifest, scenes) = load_scenes();
    let profile = support::camera_profile(&manifest);
    let config = SceneEstimationConfig::default();
    let table = CameraLocusTable::build(&profile, &config.locus, config.estimator.result_max_abs_duv).expect("таблица");
    Rig { scenes, table, config }
}

impl Rig {
    fn scene(&self, kind: &str, light: &str) -> &Scene {
        self.scenes
            .iter()
            .find(|scene| scene.kind == kind && scene.light == light)
            .unwrap_or_else(|| panic!("в фикстуре нет сцены {kind}_{light}"))
    }

    fn lights(&self) -> Vec<String> {
        let mut lights: Vec<String> = self.scenes.iter().map(|scene| scene.light.clone()).collect();
        lights.dedup();
        lights
    }

    /// Оценка через границу исполнителя (как её вызывает поток рендера).
    fn outcome(&self, scene: &Scene) -> EstimateOutcome {
        let mut executor = CpuWhiteBalanceEstimator::new(self.config.estimator).expect("исполнитель");
        executor
            .submit(EstimationRequest {
                frame: FrameTag {
                    stream_generation: 1,
                    sequence: 1,
                    sensor_timestamp_nanos: 0,
                },
                statistics: &scene.statistics,
                table: &self.table,
            })
            .expect("запрос принят");
        executor.poll().expect("ответ").outcome.expect("оценка")
    }

    fn estimate(&self, scene: &Scene) -> IlluminantEstimate {
        match self.outcome(scene) {
            EstimateOutcome::Estimated(estimate) => estimate,
            EstimateOutcome::NoCandidates { eligible_tiles } => {
                panic!("{}: нет кандидатов ({eligible_tiles} пригодных плиток)", scene.name)
            }
        }
    }
}

fn assert_matches_light(scene: &Scene, estimate: &IlluminantEstimate) {
    let mired_error = estimate.cct_duv.temperature.mired() - Kelvin(scene.expected_cct).mired();
    let duv_error = estimate.cct_duv.duv.0 - scene.expected_duv;
    assert!(
        mired_error.abs() <= MIRED_TOLERANCE,
        "{}: ошибка {mired_error:+.2} майред",
        scene.name
    );
    assert!(
        duv_error.abs() <= DUV_TOLERANCE,
        "{}: ошибка Duv {duv_error:+.4}",
        scene.name
    );
}

#[test]
fn every_light_is_recovered_from_the_colorchecker_scene() {
    let rig = rig();
    assert_eq!(rig.lights().len(), 6, "6 источников: 2 чёрных тела, D55, D65, F2, F11");
    for light in rig.lights() {
        let scene = rig.scene("checker", &light);
        let estimate = rig.estimate(scene);
        assert_matches_light(scene, &estimate);
        assert!(
            estimate.confidence >= rig.config.controller.min_confidence,
            "{}: уверенность {} ниже порога — контроллер отверг бы верную оценку",
            scene.name,
            estimate.confidence
        );
        assert!(
            !estimate.clamped.is_any(),
            "{}: свет внутри полосы, прижимать нечего",
            scene.name
        );
    }
}

#[test]
fn dark_black_patch_and_clipped_column_never_reach_the_estimate() {
    // 24 образца по 30 плиток = 720; чёрный образец ниже порога освещённости (−30), а столбец 30 тёмный и
    // столбец 31 пересвечен (в 720 они не входят): пригодных ровно 690.
    let rig = rig();
    let raw = estimate_scene_light(
        &rig.scene("checker", "d65").statistics,
        &rig.table,
        &rig.config.estimator,
    );
    assert_eq!(raw.eligible_tiles, 690);
}

#[test]
fn dominant_green_scene_shifts_the_estimate_no_more_than_a_few_mired_from_the_neutral_rich_scene() {
    let rig = rig();
    for light in rig.lights() {
        let neutral_rich = estimate_scene_light(
            &rig.scene("checker", &light).statistics,
            &rig.table,
            &rig.config.estimator,
        );
        let green_scene = rig.scene("dominant_green", &light);
        let dominant = estimate_scene_light(&green_scene.statistics, &rig.table, &rig.config.estimator);
        assert!(
            dominant.has_candidates(),
            "{light}: нейтральные плитки среди зелени найдены"
        );
        let shift = f64::from(dominant.mired - neutral_rich.mired);
        assert!(
            shift.abs() <= MIRED_TOLERANCE,
            "{light}: сдвиг {shift:+.2} майред при зелёной доминанте"
        );
        assert!(
            f64::from(dominant.duv - neutral_rich.duv).abs() <= DUV_TOLERANCE,
            "{light}: сдвиг Duv"
        );
    }
}

#[test]
fn scene_without_neutrals_has_low_confidence_so_the_controller_holds_the_white() {
    let rig = rig();
    let floor = rig.config.controller.min_confidence;
    for light in rig.lights() {
        let scene = rig.scene("no_neutrals", &light);
        let outcome = rig.outcome(scene);
        let mut controller = WhiteBalanceController::new(rig.config).expect("контроллер");
        // Белая уже известна по верному кадру; сцена без серого её не двигает.
        controller.ingest(&EstimateOutcome::Estimated(rig.estimate(rig.scene("checker", &light))));
        let before = controller.current_state().expect("белая из верной сцены");
        let verdict = controller.ingest(&outcome);
        match outcome {
            EstimateOutcome::Estimated(estimate) => {
                assert!(
                    estimate.confidence < floor,
                    "{light}: уверенность {} при отсутствии серых",
                    estimate.confidence
                );
                assert!(
                    matches!(verdict, IngestOutcome::HeldLowConfidence { .. }),
                    "{light}: {verdict:?}"
                );
            }
            EstimateOutcome::NoCandidates { .. } => assert_eq!(verdict, IngestOutcome::HeldNoCandidates),
        }
        assert_eq!(controller.current_state(), Some(before), "{light}: белая не сдвинулась");
    }
}

#[test]
fn single_colour_scene_gets_near_zero_confidence_even_when_its_hue_lies_inside_the_gray_band() {
    // Однотонная зелень (foliage) лежит у границы серой полосы (Duv 0,027–0,040 < 0,04): жёсткий порог её
    // пропускает, тяга к кривой сводит уверенность почти к нулю — белую такая сцена не двигает.
    let rig = rig();
    for light in rig.lights() {
        let scene = rig.scene("single_colour", &light);
        let verdict = match rig.outcome(scene) {
            EstimateOutcome::Estimated(estimate) => {
                assert!(
                    estimate.confidence < 0.05,
                    "{}: уверенность {}",
                    scene.name,
                    estimate.confidence
                );
                let mut controller = WhiteBalanceController::new(rig.config).expect("контроллер");
                controller.ingest(&EstimateOutcome::Estimated(estimate))
            }
            EstimateOutcome::NoCandidates { .. } => IngestOutcome::HeldNoCandidates,
        };
        assert!(
            matches!(
                verdict,
                IngestOutcome::HeldLowConfidence { .. } | IngestOutcome::HeldNoCandidates
            ),
            "{}: однотонная сцена не должна принимать оценку: {verdict:?}",
            scene.name
        );
    }
}

#[test]
fn estimate_flows_through_the_controller_into_the_scene_white() {
    let rig = rig();
    for light in rig.lights() {
        let scene = rig.scene("checker", &light);
        let mut controller = WhiteBalanceController::new(rig.config).expect("контроллер");
        assert_eq!(controller.ingest(&rig.outcome(scene)), IngestOutcome::Accepted);
        let white = controller.advance(0).expect("шаг").white.expect("белая определена");
        let mired_error = white.cct_duv.temperature.mired() - Kelvin(scene.expected_cct).mired();
        assert!(
            mired_error.abs() <= MIRED_TOLERANCE,
            "{light}: белая контроллера {mired_error:+.2} майред"
        );
    }
}

#[test]
fn plain_weighted_mean_is_dragged_warm_by_near_locus_colours_which_is_why_the_mode_window_is_the_default() {
    // Отклонение от «калитка по Duv + взвешенное среднее»: на ColorChecker среднее уезжает на десятки–сотни
    // майред (кожа, дерево, жёлтое выглядят как более тёплый «серый» вблизи кривой). Измерено в сессии 08:
    // D65 — 279 майред вместо 154, окно по моде — 154,1.
    let rig = rig();
    let scene = rig.scene("checker", "d65");
    let expected = Kelvin(scene.expected_cct).mired();
    let mean_config = EstimatorConfig {
        reduction: ReductionConfig::WeightedMean,
        ..rig.config.estimator
    };
    let mean = estimate_scene_light(&scene.statistics, &rig.table, &mean_config);
    let mode = estimate_scene_light(&scene.statistics, &rig.table, &rig.config.estimator);
    assert!(
        f64::from(mean.mired) - expected > 50.0,
        "среднее смещено: {}",
        mean.mired
    );
    assert!(
        (f64::from(mode.mired) - expected).abs() < MIRED_TOLERANCE,
        "окно по моде точно: {}",
        mode.mired
    );
}

#[test]
fn table_build_is_deterministic_for_the_same_profile() {
    let rig = rig();
    let (manifest, _) = load_scenes();
    let again = CameraLocusTable::build(&support::camera_profile(&manifest), &LocusTableConfig::default(), 0.02)
        .expect("таблица");
    assert_eq!(
        again.segments(),
        rig.table.segments(),
        "таблица — чистая функция профиля и конфига"
    );
}
