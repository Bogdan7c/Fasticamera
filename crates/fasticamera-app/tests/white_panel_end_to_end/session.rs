//! Сеанс панели белого: стенд «палец → экран → движок → рендер → пиксели» с фальшивым источником «телефон», доской
//! показаний и накопленными кадрами камеры. Сценарии читаются как действия человека: тап, долгое нажатие, протяжка,
//! а проверки — как результат: что держит движок, что нарисовал экран и какие пиксели получились против эталонного
//! прогона (белое названо с первого кадра, заявок нет).

use std::sync::Arc;

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{
    HeldWhite, SceneIlluminant, WhiteConfidence, WhiteOrigin, WhiteSourceId, WhiteSourceOutcome,
};
use camera_contract::white_reading::WhiteReadingBoard;
use camera_screen::tools::ToolId;
use camera_screen::{PanelState, ScreenConfig, WhiteAxis};
use fasticamera::white_memory::WhiteMemoryHandoff;
use render_loop::config::WhiteBalanceConfig;
use ui_core::geometry::Point;
use ui_draw::draw_list::DrawList;

use super::engine_stand::{LIGHT_CHANGES_AT, StandOptions};
use super::fake_source::{FakeSourceControl, Reaction, registry_with};
use super::stand::{CameraPort, PanelStand};
use super::white_engine::{held_setting, lamp_outcome, setting_of};
use super::white_rig::{FrameImage, GRAY_PIXEL, rig_or_skip, scene_with_light_change};

/// Ширина изображения видоискателя стенда, пикселей (2 × 2 ячейки поверхности 96 × 160 → 32 × 24).
const IMAGE_WIDTH: usize = 32;
pub const PHONE: WhiteSourceId = WhiteSourceId::PHONE;
/// Лампа, которую источник называет при открытии камеры (белое до всех действий человека).
pub const OPENING_KELVIN: f64 = 2800.0;
/// Сколько кадров камеры (вместе с кадрами UI) гоним, пока белое «доезжает» до картинки.
pub const SETTLE_FRAMES: i64 = 20;
/// Сколько последних кадров сравниваем с эталоном.
pub const COMPARED_TAIL: usize = 4;
/// Кадры UI, за которые карточка белого доезжает до места (пружины успокаиваются за ≈ 0,7 с).
pub const CARD_SETTLE_UI_FRAMES: usize = 45;

pub struct WhiteSession {
    pub stand: PanelStand,
    pub source: Arc<FakeSourceControl>,
    pub readings: WhiteReadingBoard,
    images: Vec<FrameImage>,
    screen_config: ScreenConfig,
    /// Где сейчас палец на шкале (между `press_ruler_and_move` и `release_ruler`).
    ruler_finger: Option<Point>,
}

fn no_hints(_frame: i64) -> CaptureColorHints {
    CaptureColorHints::default()
}

/// Ответ источника «свет такой-то через `frames` кадров».
pub fn lamp_after(frames: u32, kelvin: f64) -> Reaction {
    Reaction::Answer {
        outcome: lamp_outcome(kelvin),
        after_frames: frames,
    }
}

impl WhiteSession {
    /// Сеанс, где источник при открытии называет лампу 2800 К и дальше отвечает так же быстро.
    pub fn open() -> Option<Self> {
        Self::open_with(lamp_after(2, OPENING_KELVIN), WhiteMemoryHandoff::default())
    }

    /// Сеанс с заданной реакцией источника и памятью белого; камера открыта, кадры идут.
    pub fn open_with(reaction: Reaction, memory: WhiteMemoryHandoff) -> Option<Self> {
        Self::open_with_config(reaction, memory, ScreenConfig::default())
    }

    /// Сеанс как [`WhiteSession::open`], но кнопка белого стоит с `side` от затвора.
    pub fn open_on_side(side: camera_screen::WhiteButtonSide) -> Option<Self> {
        let mut screen_config = ScreenConfig::default();
        screen_config.white.button_side = side;
        Self::open_with_config(
            lamp_after(2, OPENING_KELVIN),
            WhiteMemoryHandoff::default(),
            screen_config,
        )
    }

    fn open_with_config(reaction: Reaction, memory: WhiteMemoryHandoff, screen_config: ScreenConfig) -> Option<Self> {
        let source = FakeSourceControl::new(reaction);
        let readings = WhiteReadingBoard::new();
        let mut stand = PanelStand::start_with_screen_config(
            StandOptions {
                sources: registry_with(PHONE, &source),
                memory,
            },
            CameraPort::LiveEngine,
            readings.clone(),
            screen_config.clone(),
        )?;
        stand.resume();
        let mut session = Self {
            stand,
            source,
            readings,
            images: Vec::new(),
            screen_config,
            ruler_finger: None,
        };
        session.frames(12);
        Some(session)
    }

    /// `count` пар «кадр UI, кадр камеры»; пиксели камеры копятся.
    pub fn frames(&mut self, count: i64) {
        let produced = self.stand.alternate(count, &no_hints);
        self.images.extend(produced);
    }

    /// Только кадры камеры, без кадров UI: экран не успевает узнать, что движок сделал за это время (гонка фактов).
    pub fn camera_frames_only(&mut self, count: i64) {
        let produced = self.stand.run_camera_frames(count, &no_hints);
        self.images.extend(produced);
    }

    /// `count` пар «кадр UI, кадр камеры»; после КАЖДОЙ пары `probe` читает список рисования экрана. Так видны
    /// вещи, которые живут во времени: мерцание (прозрачность) и тряска (позиция).
    pub fn watch<T>(&mut self, count: i64, probe: impl Fn(&DrawList) -> T) -> Vec<T> {
        let mut seen = Vec::new();
        for _ in 0..count {
            self.frames(1);
            seen.push(probe(self.stand.ui_list()));
        }
        seen
    }

    /// Серая плитка последнего показанного кадра камеры (RGB, 32 × 24 пикселя изображения видоискателя).
    pub fn gray_tile_of_the_last_frame(&self) -> [f32; 3] {
        let image = self
            .images
            .last()
            .and_then(Option::as_ref)
            .expect("последний кадр показан");
        self.gray_tile_of(image)
    }

    /// Серая плитка произвольного кадра того же стенда (например, эталонного).
    pub fn gray_tile_of(&self, image: &[[f32; 4]]) -> [f32; 3] {
        let pixel = image[GRAY_PIXEL.1 * IMAGE_WIDTH + GRAY_PIXEL.0];
        [pixel[0], pixel[1], pixel[2]]
    }

    /// Номер следующего кадра камеры (сколько уже показано/скрыто).
    pub fn frame_count(&self) -> usize {
        self.images.len()
    }

    pub fn held(&self) -> HeldWhite {
        self.stand.status().three_a.white.held
    }

    /// Кнопка света `index` из конфига экрана: белое, которое она ставит.
    pub fn light_setting(&self, index: usize) -> WhiteBalanceSetting {
        self.screen_config.white.lights[index]
            .setting()
            .expect("число кнопки годится")
    }

    pub fn panel(&self) -> PanelState {
        self.stand.screen().panel()
    }

    // ---- палец ----

    pub fn white_button(&self) -> Point {
        self.stand.white_targets().button.center()
    }

    /// Тап по кнопке белого: панель открывается (или закрывается) и доезжает до места.
    pub fn tap_button(&mut self) {
        self.stand.tap(self.white_button());
        self.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
    }

    /// Открыть карточку белого и убедиться, что она открылась.
    pub fn open_card(&mut self) {
        self.tap_button();
        assert_eq!(
            self.panel(),
            PanelState::Submenu(ToolId::White),
            "карточка белого открылась"
        );
    }

    /// Долгое нажатие на кнопку белого (800 мс времени экрана; порог 500 мс).
    pub fn long_press_button(&mut self) {
        self.stand.hold(self.white_button(), 800);
        self.stand.ui_frames(3);
    }

    pub fn tap_light(&mut self, index: usize) {
        let target = self.stand.white_targets().card.expect("карточка открыта").light_buttons[index];
        self.stand.tap(target);
        self.stand.ui_frames(3);
    }

    /// Тап «Auto» в открытой карточке: слежение за белым телефона (WB44).
    pub fn tap_auto(&mut self) {
        let target = self.stand.white_targets().card.expect("карточка открыта").auto_button;
        self.stand.tap(target);
        self.stand.ui_frames(3);
    }

    pub fn tap_phone(&mut self) {
        let target = self.stand.white_targets().card.expect("карточка открыта").phone_button;
        self.stand.tap(target);
        self.stand.ui_frames(3);
    }

    /// Тап по кадру видоискателя (центр кадра).
    pub fn tap_frame(&mut self) {
        let center = super::touch_targets::viewfinder_center(self.stand.screen());
        self.stand.tap(center);
        self.stand.ui_frames(30);
    }

    /// Выбрать ось шкалы тапом по её числу в карточке.
    pub fn select_axis(&mut self, axis: WhiteAxis) {
        let card = self.stand.white_targets().card.expect("карточка открыта");
        let number = match axis {
            WhiteAxis::Temperature => card.temperature_number,
            WhiteAxis::Tint => card.tint_number,
        };
        self.stand.tap(number);
        self.stand.ui_frames(3);
    }

    /// Протяжка шкалы: палец лёг в центр зоны шкалы и идёт на `step_pt` pt за кадр UI `steps` раз (влево — минус),
    /// замирает на 12 кадров (инерции нет: шкала успокаивается ровно там, где стоит палец) и отрывается.
    pub fn drag_ruler(&mut self, step_pt: f64, steps: usize) {
        self.press_ruler_and_move(step_pt, steps);
        self.release_ruler();
    }

    /// Палец лёг на шкалу и прошёл `steps` шагов, но ещё не оторван (нужно «пальцем на шкале» между кадрами).
    pub fn press_ruler_and_move(&mut self, step_pt: f64, steps: usize) {
        let from = self
            .stand
            .white_targets()
            .card
            .expect("карточка открыта")
            .ruler_zone
            .center();
        self.stand.touch_down(from);
        self.ruler_finger = Some(from);
        for _ in 0..steps {
            self.move_ruler_finger(step_pt);
            self.stand.ui_frames(1);
        }
    }

    /// Ещё один шаг пальца по шкале без кадра UI (касание ждёт ближайшего кадра).
    pub fn move_ruler_finger(&mut self, step_pt: f64) {
        let finger = self.ruler_finger.as_mut().expect("палец на шкале");
        finger.x += step_pt;
        let at = *finger;
        self.stand.touch_move(at);
    }

    /// Палец замер на месте и оторвался.
    pub fn release_ruler(&mut self) {
        let at = self.ruler_finger.take().expect("палец на шкале");
        for _ in 0..12 {
            self.stand.touch_move(at);
            self.stand.ui_frames(1);
        }
        self.stand.touch_up(at);
        self.stand.ui_frames(2);
    }

    // ---- ожидание движка ----

    /// Ждёт, пока движок держит белое с происхождением `origin`.
    pub fn wait_origin(&self, what: &str, origin: WhiteOrigin) -> HeldWhite {
        self.stand
            .wait_status(what, |status| status.three_a.white.held.origin == origin)
            .three_a
            .white
            .held
    }

    /// Ждёт условия вне статуса движка (файл на диске и т. п.).
    pub fn wait_until(what: &str, condition: impl Fn() -> bool) {
        super::engine_stand::Stand::wait_until(what, condition);
    }

    // ---- пиксели ----

    /// Пиксели последних кадров против эталона `setting`: сначала `SETTLE_FRAMES` кадров, чтобы белое доехало, потом
    /// сравнение побитово.
    pub fn assert_picture_is(&mut self, setting: WhiteBalanceSetting, what: &str) {
        self.frames(SETTLE_FRAMES);
        let total = self.images.len();
        let reference = reference_images(setting, total as i64);
        let first_compared = total - COMPARED_TAIL;
        for (offset, (actual, expected)) in self.images[first_compared..]
            .iter()
            .zip(&reference[first_compared..])
            .enumerate()
        {
            let index = first_compared + offset;
            assert!(actual.is_some(), "{what}: кадр {index} показан");
            assert_eq!(actual, expected, "{what}: кадр {index} побитово как у эталона");
        }
    }
}

/// Ответ источника «свет — ровно эти числа» (Кельвины и оттенок) через `frames` кадров.
pub fn answer_with(setting: WhiteBalanceSetting, frames: u32) -> Reaction {
    Reaction::Answer {
        outcome: WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(setting),
            confidence: WhiteConfidence::FULL,
        },
        after_frames: frames,
    }
}

/// Белое из чисел.
pub fn white_of(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

/// Эталонный прогон: белая названа с первого кадра, заявок нет.
pub fn reference_images(setting: WhiteBalanceSetting, frames: i64) -> Vec<FrameImage> {
    let (mut rig, journal) = rig_or_skip(
        scene_with_light_change(200, LIGHT_CHANGES_AT),
        held_setting(setting),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    )
    .expect("GPU есть (стенд уже поднимался)");
    let images = rig.drive(0..frames, &no_hints, &mut |_| {});
    rig.finish(&journal);
    images
}

/// Белое, которое источник назвал при открытии.
pub fn opening_setting() -> WhiteBalanceSetting {
    setting_of(OPENING_KELVIN)
}
