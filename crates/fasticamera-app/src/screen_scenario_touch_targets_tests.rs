//! Точки плана касаний смоука (`ScreenTouchPlan`) против целей НАСТОЯЩЕГО экрана (`CameraScreen::white_touch_targets`
//! и геометрия значков и кнопки белого): если раскладка сдвинется, смоук на телефоне промахнётся, а этот тест покраснеет на ПК.

use camera_screen::{
    CameraFacts, CameraScreen, FlashAvailability, PanelState, ScreenAction, ScreenConfig, ScreenTouch,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;
use ui_kit::tokens::UiKitTokens;

use crate::screen_scenario_config::{ScreenPoint, ScreenTouchPlan};

const SURFACE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};
const WIDTH_PT: f64 = 440.0;
const PX_PER_PT: f64 = 1080.0 / WIDTH_PT;
const FRAME_MS: i64 = 16;
/// Допуск совпадения точки плана с центром цели, pt: план округлён до 4 знаков доли (0,04 pt), остальное — расхождение
/// раскладки; значок и кнопка шире 30 pt, поэтому 1,5 pt — «в ту же точку», а не «куда-то внутри».
const TOLERANCE_PT: f64 = 1.5;

struct Driver {
    screen: CameraScreen,
    now_ms: i64,
}

impl Driver {
    fn new() -> Self {
        let screen = CameraScreen::new(ScreenConfig::default(), UiKitTokens::default(), MotionPreference::Full)
            .expect("экран собирается");
        let mut driver = Self { screen, now_ms: 0 };
        driver.screen.handle_action(ScreenAction::SurfaceResized(SURFACE));
        // Телефон со вспышкой: без неё ряд короче на один значок и точка EV из плана (рассчитана на три) не та.
        driver
            .screen
            .handle_action(ScreenAction::CameraFactsChanged(CameraFacts {
                flash: FlashAvailability::Available,
                ..crate::camera_screen_facts::facts_for_absent_engine()
            }));
        driver.run(2 * FRAME_MS);
        driver
    }

    fn run(&mut self, duration_ms: i64) {
        let end = self.now_ms + duration_ms;
        while self.now_ms < end {
            self.now_ms += FRAME_MS;
            let request = UiFrameRequest {
                present_time: MonotonicNanos::from_nanos(self.now_ms * 1_000_000),
                logical_size: PxSize::new(SURFACE.width_px as f32, SURFACE.height_px as f32),
                px_per_pt: PX_PER_PT as f32,
                backdrop: BackdropContinuity::Live,
            };
            self.screen.produce_frame(&request).expect("кадр");
        }
    }

    fn touch(&mut self, phase: TouchPhase, point: Point, at_ms: i64) {
        self.screen.handle_touch(ScreenTouch::Sample(TouchSample {
            pointer: PointerId::new(0),
            phase,
            position: SurfacePoint {
                x_px: (point.x * PX_PER_PT) as f32,
                y_px: (point.y * PX_PER_PT) as f32,
            },
            time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
        }));
    }

    fn tap(&mut self, point: Point) {
        let start = self.now_ms;
        self.touch(TouchPhase::Began, point, start);
        self.touch(TouchPhase::Ended, point, start + 50);
        self.run(FRAME_MS);
    }
}

/// Высота экрана в pt по той же ширине 440 pt.
fn height_pt() -> f64 {
    f64::from(SURFACE.height_px) / PX_PER_PT
}

/// Точка плана (доли экрана) в pt экрана.
fn in_pt(point: ScreenPoint) -> Point {
    Point::new(point.x * WIDTH_PT, point.y * height_pt())
}

fn assert_same_spot(name: &str, planned: ScreenPoint, real_center: Point) {
    let planned = in_pt(planned);
    assert!(
        (planned.x - real_center.x).abs() <= TOLERANCE_PT && (planned.y - real_center.y).abs() <= TOLERANCE_PT,
        "{name}: план {planned:?}, настоящая цель {real_center:?}"
    );
}

#[test]
fn plan_points_of_the_white_button_and_the_ev_icon_hit_the_real_screen() {
    let mut driver = Driver::new();
    let plan = ScreenTouchPlan::default();
    let button = driver
        .screen
        .white_touch_targets()
        .map(|targets| targets.button)
        .expect("раскладка есть");
    assert_same_spot("white_button", plan.white_button, button.center());
    assert_same_spot("white_hold", plan.white_hold.at, button.center());
    // Кнопка и затвор стоят в одном ряду: одна высота.
    let shutter = driver.screen.white_touch_targets().expect("цели").button.center().y;
    assert!((in_pt(plan.white_button).y - shutter).abs() <= TOLERANCE_PT);

    // Тапом открываем карточку белого и сверяем кнопку света и точку «тап по кадру».
    driver.tap(button.center());
    driver.run(700);
    let card = driver
        .screen
        .white_touch_targets()
        .and_then(|targets| targets.card)
        .expect("карточка белого открылась тапом по кнопке из плана");
    let light = card
        .light_buttons
        .get(plan.white_light_index)
        .copied()
        .expect("номер кнопки света из плана есть в списке");
    assert_same_spot("white_light", plan.white_light, light);
    // Ряд карточки: «Auto», «Phone», потом свет (WB48) — план жмёт свет, а не источник.
    assert!(
        card.auto_button.x < card.phone_button.x && card.phone_button.x < light.x,
        "порядок слева направо: Auto {:?}, Phone {:?}, свет {light:?}",
        card.auto_button,
        card.phone_button
    );
    // Кнопка белого поднялась вместе с затвором и осталась рабочей: точка плана в покое теперь пуста, цель — выше.
    let raised = driver.screen.white_touch_targets().expect("цели").button;
    assert!(
        raised.center().y < button.center().y - 100.0,
        "кнопка поднята: {raised:?}"
    );
    // Тап по кадру (проба плана) при открытой карточке белого только закрывает её.
    driver.tap(in_pt(plan.probe_tap));
    driver.run(700);
    assert_eq!(
        driver.screen.state().panel(),
        PanelState::Closed,
        "проба закрыла карточку"
    );

    // Значок EV из плана открывает карточку Exposure, «закрыть» из плана закрывает её: настоящие касания.
    driver.tap(in_pt(plan.ev_icon));
    driver.run(700);
    assert_eq!(
        driver.screen.state().panel(),
        PanelState::ExposurePanel,
        "ev_icon попал в значок EV"
    );
    driver.tap(in_pt(plan.ev_close));
    driver.run(700);
    assert_eq!(
        driver.screen.state().panel(),
        PanelState::Closed,
        "ev_close попал в кнопку закрытия"
    );
    // Точка-проба «тап по кадру» лежит над карточкой: не попадает ни в одну из её целей.
    let probe = in_pt(plan.probe_tap);
    assert!(
        probe.y < card.ruler_zone.origin.y && probe.y < card.phone_button.y && probe.y < card.auto_button.y,
        "проба {probe:?} должна лежать выше карточки"
    );
}
