//! Миниатюра снимка сквозь настоящий маршрутизатор, настоящий экран и настоящую службу записи (14g): нажатие →
//! «рендер» отдаёт миниатюру и итог → в списке рисования экрана картинка с пикселями ЭТОГО снимка. Платформа и
//! поток рендера — подделки (`still_test_support`, `RenderClock`).

use camera_contract::geometry::PixelSize;
use camera_screen::shutter::ShotRequestId;
use photo_output::service::StillService;
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::plan::StillId;
use still_contract::thumbnail::StillThumbnail;
use ui_draw::draw_list::{DrawItem, ImageItem};

use super::*;
use crate::photo_config::PhotoConfig;
use crate::still_test_support::{FakeRenderPort, RenderMode, facts_for, readback_image, start_still_runtime};

/// Размер миниатюры в тестах: маленький, но не квадрат (путаница ширины с высотой была бы видна).
const THUMBNAIL_SIZE: PixelSize = PixelSize::new(4, 2);

/// Пиксели миниатюры: у разных `seed` разные байты.
fn pixels(seed: u8) -> Vec<u8> {
    (0..32_u8)
        .map(|index| seed.wrapping_add(index.wrapping_mul(3)))
        .collect()
}

fn thumbnail_of(id: StillId, seed: u8) -> StillThumbnail {
    StillThumbnail::new(id, THUMBNAIL_SIZE, pixels(seed)).expect("размер сошёлся")
}

fn images(list: &DrawList) -> Vec<&ImageItem> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

/// Стенд: экран с настоящей службой записи, поток рендера — часы тестов.
struct ThumbnailRig {
    harness: Harness,
    render: Arc<FakeRenderPort>,
    clock: RenderClock,
    /// Служба должна пережить тест (её `Drop` останавливает рабочий поток).
    _service: StillService,
}

impl ThumbnailRig {
    fn start() -> Self {
        let render = FakeRenderPort::new(RenderMode::Accepting);
        let still = start_still_runtime(&PhotoConfig::default(), &render);
        let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
            CameraConfig::default(),
        ));
        harness.router.camera_screen = CameraScreenService::with_capture(instant_capture_config(), still.backend);
        harness.router.install_camera_screen();
        start_streaming(&mut harness);
        let mut clock = RenderClock::new();
        clock.frames(&harness, 2);
        Self {
            harness,
            render,
            clock,
            _service: still.service,
        }
    }

    /// Нажатие клавиши громкости (с отпусканием): возвращает номер снимка службы и момент нажатия, мс.
    fn press(&mut self) -> (StillId, i64) {
        let pressed_ms = self.clock.now_ms;
        let known = self.render.plans().len();
        volume_key(&mut self.harness, KeyPhase::Pressed, KeyRepeat::Initial, pressed_ms);
        self.clock.frames(&self.harness, 3);
        volume_key(
            &mut self.harness,
            KeyPhase::Released,
            KeyRepeat::Initial,
            self.clock.now_ms,
        );
        self.clock.frames(&self.harness, 3);
        let plans = self.render.plans();
        assert_eq!(plans.len(), known + 1, "нажатие дошло до рендера планом: {plans:?}");
        (plans[known].id, pressed_ms)
    }

    /// «Рендер» снял кадр; ждём, пока затвор экрана освободится (фаза `Secured` дошла до экрана).
    fn secure(&mut self, id: StillId) {
        self.render.emit(
            id,
            DevelopEvent::Secured {
                facts: facts_for(id),
                raw: None,
            },
        );
        self.frames_until(
            "затвор освободился после «кадр захвачен»",
            |report| report.state.shutter_activity == crate::camera_screen_source::ShutterActivity::Idle,
        );
    }

    /// «Рендер» отдаёт миниатюру и итог «картинка прочитана»: так движок снимков шлёт их в настоящем порядке.
    fn finish_with_thumbnail(&mut self, id: StillId, seed: u8) {
        self.render.emit_thumbnail(id, thumbnail_of(id, seed));
        self.render.wait_readback_target(id);
        self.render.emit(
            id,
            DevelopEvent::PrimaryReadback {
                id,
                image: readback_image(),
            },
        );
    }

    /// Крутит кадры, пока отчёт экрана не удовлетворит условию.
    fn frames_until(
        &mut self,
        what: &str,
        mut condition: impl FnMut(&crate::camera_screen_source::ScreenReport) -> bool,
    ) {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            self.clock.frames(&self.harness, 1);
            if self
                .harness
                .router
                .camera_screen()
                .report()
                .is_some_and(|report| condition(&report))
            {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "не дождались: {what}");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Крутит кадры, пока в миниатюре не окажется снимок `request` (с картинкой или без).
    fn frames_until_thumbnail_of(&mut self, request: ShotRequestId) {
        self.frames_until(
            "миниатюра указывает на снимок",
            |report| matches!(&report.thumbnail, ThumbnailState::Last { shot, .. } if *shot == request),
        );
    }

    /// Кадры подряд без пауз на `duration_ms` и последний список рисования.
    fn draw_for(&mut self, duration_ms: i64) -> DrawList {
        let mut last = self.clock.frame_list(&self.harness);
        for _ in 0..(duration_ms / FRAME_MS) {
            last = self.clock.frame_list(&self.harness);
        }
        last
    }

    fn screen_request(&self, index: usize) -> ShotRequestId {
        self.harness.router.camera_screen().capture_journal()[index].request
    }

    fn ledger_counters(&self) -> crate::still_screen_ledger::LedgerCounters {
        self.harness
            .router
            .camera_screen()
            .capture_diagnostics()
            .real
            .expect("настоящий режим")
            .screen
    }
}

#[test]
fn the_pixels_the_render_made_for_a_shot_end_up_in_the_screens_draw_list_after_saved() {
    let mut rig = ThumbnailRig::start();
    let (id, _pressed_ms) = rig.press();
    let request = rig.screen_request(0);
    rig.secure(id);

    rig.finish_with_thumbnail(id, 10);
    rig.frames_until_thumbnail_of(request);
    let list = rig.draw_for(300);

    let shown = images(&list);
    assert_eq!(
        shown.len(),
        1,
        "одна картинка в круге миниатюры: {:?}",
        list.items.len()
    );
    assert_eq!(
        shown[0].picture.rgba8(),
        pixels(10).as_slice(),
        "те же байты, что отдал рендер"
    );
    let size = shown[0].picture.size();
    assert_eq!((size.width, size.height), (THUMBNAIL_SIZE.width, THUMBNAIL_SIZE.height));
    assert_eq!(shown[0].opacity.get(), 1.0, "растворение закончилось");
    let counters = rig.ledger_counters();
    assert_eq!((counters.thumbnails_held, counters.thumbnails_delivered), (1, 1));
}

#[test]
fn of_two_shots_with_reversed_outcomes_the_one_saved_last_is_shown() {
    let mut rig = ThumbnailRig::start();
    let (first, _) = rig.press();
    rig.secure(first);
    let (second, _) = rig.press();
    rig.secure(second);
    let (first_request, second_request) = (rig.screen_request(0), rig.screen_request(1));

    // Второй снимок сохраняется раньше первого.
    rig.finish_with_thumbnail(second, 50);
    rig.frames_until_thumbnail_of(second_request);
    rig.finish_with_thumbnail(first, 90);
    rig.frames_until_thumbnail_of(first_request);
    let list = rig.draw_for(400);

    let shown = images(&list);
    assert_eq!(shown.len(), 1, "растворение закончилось: одна картинка");
    assert_eq!(
        shown[0].picture.rgba8(),
        pixels(90).as_slice(),
        "показан снимок, сохранённый ПОСЛЕДНИМ"
    );
}

#[test]
fn a_failed_shot_leaves_the_previous_picture_and_its_own_thumbnail_is_thrown_away() {
    let mut rig = ThumbnailRig::start();
    let (first, _) = rig.press();
    rig.secure(first);
    rig.finish_with_thumbnail(first, 10);
    rig.frames_until_thumbnail_of(rig.screen_request(0));
    rig.draw_for(300);
    let (second, _) = rig.press();
    rig.secure(second);

    // Миниатюра второго снимка пришла, но сам снимок проваливается.
    rig.render.emit_thumbnail(second, thumbnail_of(second, 200));
    rig.render.emit(
        second,
        DevelopEvent::Failed {
            id: second,
            failure: DevelopFailure::Gpu("тест".to_owned()),
        },
    );
    crate::still_test_support::wait_until("итог провала у экрана", || {
        rig.ledger_counters().failed_delivered == 1
    });
    rig.clock.frames(&rig.harness, 3);
    let list = rig.draw_for(400);

    let shown = images(&list);
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].picture.rgba8(),
        pixels(10).as_slice(),
        "картинка первого снимка осталась"
    );
    let counters = rig.ledger_counters();
    assert_eq!(
        (counters.thumbnails_dropped_failed, counters.thumbnails_delivered),
        (1, 1)
    );
}

#[test]
fn saved_while_the_screen_produced_no_frames_shows_the_picture_and_starts_the_fade_at_the_first_frame_back() {
    let mut rig = ThumbnailRig::start();
    let (id, pressed_ms) = rig.press();
    let request = rig.screen_request(0);
    rig.secure(id);

    // Поверхность потеряна (приложение свернуто): кадров экран не строит, но стоит установленным.
    rig.finish_with_thumbnail(id, 33);
    crate::still_test_support::wait_until(
        "итог снимка лежит в очереди экрана",
        || rig.ledger_counters().thumbnails_delivered == 1,
    );
    assert_eq!(
        rig.harness.router.camera_screen().report().expect("отчёт").thumbnail,
        ThumbnailState::Empty,
        "без кадров экран очередь не разбирает"
    );
    let minutes_gone_ms: i64 = 10 * 60 * 1_000;
    rig.clock.now_ms += minutes_gone_ms;

    // Первый кадр после возврата: очередь применена, растворение только началось (картинка ещё прозрачна).
    let first_frame = rig.clock.frame_list(&rig.harness);
    let first_frame_time = rig.clock.time();
    assert!(
        images(&first_frame).is_empty(),
        "в момент старта растворения картинка прозрачна"
    );
    let report = rig.harness.router.camera_screen().report().expect("отчёт");
    let ThumbnailState::Last {
        shot,
        picture: Some(picture),
    } = &report.thumbnail
    else {
        panic!("после возврата миниатюра с картинкой: {:?}", report.thumbnail);
    };
    assert_eq!(*shot, request);
    assert_eq!(picture.rgba8(), pixels(33).as_slice());
    let timing = report.thumbnail_timing.expect("замер «нажатие → картинка»");
    assert_eq!(
        timing.shown_at,
        Some(first_frame_time),
        "растворение стартовало на кадре возврата, а не при приходе итога"
    );
    assert_eq!(timing.pressed_at, MonotonicNanos::from_nanos(pressed_ms * 1_000_000));
    let expected_ms = (first_frame_time.as_nanos() - timing.pressed_at.as_nanos()) as f64 / 1_000_000.0;
    assert_eq!(timing.press_to_picture_ms(), Some(expected_ms));

    // Через 80 мс после возврата растворение в середине: доля 80/150, а не «уже закончено».
    rig.clock.now_ms += 80 - FRAME_MS;
    let later = rig.clock.frame_list(&rig.harness);
    let shown = images(&later);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].picture.rgba8(), pixels(33).as_slice());
    assert!(
        (shown[0].opacity.get() - 80.0 / 150.0).abs() < 1e-3,
        "{}",
        shown[0].opacity.get()
    );
}
