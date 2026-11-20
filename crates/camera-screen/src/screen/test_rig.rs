//! Стенд тестов фасада: экран на 1080 × 2412 px (`layout.md` раздел 3), синтетические касания в pt и кадры по меткам времени.

use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_draw::text::TextRun;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};
use ui_kit::view::{LabelPainter, PxPerPt};

use super::{CameraScreen, ScreenTouch};
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::Effect;
use crate::test_support::live_facts;

pub(super) const WIDTH_PX: f32 = 1080.0;
pub(super) const HEIGHT_PX: f32 = 2412.0;
/// 1 pt = ширина / 440 px (решение 82).
pub(super) const PX_PER_PT: f32 = WIDTH_PX / 440.0;
/// Период кадра 60 Гц, мс.
pub(super) const FRAME_MS: i64 = 16;

pub(super) fn ms(value: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(value * 1_000_000)
}

pub(super) fn request(at_ms: i64, size: (f32, f32)) -> UiFrameRequest {
    UiFrameRequest {
        present_time: ms(at_ms),
        logical_size: PxSize::new(size.0, size.1),
        px_per_pt: size.0 / 440.0,
        backdrop: ui_draw::frame_source::BackdropContinuity::Live,
    }
}

pub(super) struct Rig {
    pub(super) screen: CameraScreen,
    pub(super) now_ms: i64,
    pub(super) last: DrawList,
    /// Что рендер говорит о фоне под UI в следующих кадрах (сессия 13).
    pub(super) backdrop: BackdropContinuity,
}

impl Rig {
    /// Экран с живой камерой, первый кадр построен, эффекты первого кадра (кадрирование) отброшены.
    pub(super) fn new() -> Self {
        Self::with(ScreenConfig::default(), MotionPreference::Full)
    }

    /// Экран с каталогом зума и активным объективом `role`; эффекты создания (адоптирование) отброшены.
    pub(super) fn with_zoom(role: camera_contract::ids::LensRole, motion: MotionPreference) -> Self {
        let mut rig = Self::with(ScreenConfig::default(), motion);
        rig.screen
            .handle_action(ScreenAction::CameraFactsChanged(crate::test_support::zoom_facts(role)));
        rig.frame();
        rig.screen.drain_effects();
        rig
    }

    pub(super) fn with(config: ScreenConfig, motion: MotionPreference) -> Self {
        let screen = CameraScreen::new(config, UiKitTokens::default(), motion).expect("экран");
        let mut rig = Self {
            screen,
            now_ms: 0,
            last: DrawList::new(ui_draw::light::LightDirection::from_angle_degrees(45.0)),
            backdrop: BackdropContinuity::Live,
        };
        rig.frame();
        rig.screen.handle_action(ScreenAction::CameraFactsChanged(live_facts()));
        rig.frame();
        rig.screen.drain_effects();
        rig
    }

    /// Один кадр на текущем времени.
    pub(super) fn frame(&mut self) -> &DrawList {
        let mut frame_request = request(self.now_ms, (WIDTH_PX, HEIGHT_PX));
        frame_request.backdrop = self.backdrop;
        self.last = self.screen.produce_frame(&frame_request).expect("кадр");
        &self.last
    }

    /// Пройти `duration_ms` кадрами по 60 Гц.
    pub(super) fn run(&mut self, duration_ms: i64) -> &DrawList {
        let end = self.now_ms + duration_ms;
        while self.now_ms < end {
            self.now_ms += FRAME_MS;
            self.frame();
        }
        &self.last
    }

    pub(super) fn touch(&mut self, phase: TouchPhase, point: Point, at_ms: i64) {
        self.touch_pointer(0, phase, point, at_ms);
    }

    /// Касание конкретным пальцем (для щипка).
    pub(super) fn touch_pointer(&mut self, pointer: i32, phase: TouchPhase, point: Point, at_ms: i64) {
        self.screen.handle_touch(ScreenTouch::Sample(TouchSample {
            pointer: PointerId::new(pointer),
            phase,
            position: SurfacePoint {
                x_px: (point.x * f64::from(PX_PER_PT)) as f32,
                y_px: (point.y * f64::from(PX_PER_PT)) as f32,
            },
            time: ms(at_ms),
        }));
    }

    /// Короткий тап: касание, отрыв через 50 мс, кадр.
    pub(super) fn tap(&mut self, point: Point) {
        let start = self.now_ms;
        self.touch(TouchPhase::Began, point, start);
        self.touch(TouchPhase::Ended, point, start + 50);
        self.now_ms = start + 60;
        self.frame();
    }

    /// Ведение пальца из `from` в `to` за `duration_ms` шагами по 16 мс, отрыв, кадр.
    pub(super) fn drag(&mut self, from: Point, to: Point, duration_ms: i64) {
        let start = self.now_ms;
        self.touch(TouchPhase::Began, from, start);
        let steps = (duration_ms / FRAME_MS).max(1);
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let point = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.touch(TouchPhase::Moved, point, start + step * FRAME_MS);
        }
        self.touch(TouchPhase::Ended, to, start + steps * FRAME_MS + 1);
        self.now_ms = start + steps * FRAME_MS + 10;
        self.frame();
    }

    pub(super) fn effects(&mut self) -> Vec<Effect> {
        self.screen.drain_effects()
    }

    /// Геометрия экрана из раскладки состояния.
    pub(super) fn frame_center(&self) -> Point {
        let layout = self.screen.state().layout().expect("раскладка");
        layout.frame_rect(self.screen.state().settings().aspect).center()
    }

    /// Центр затвора в pt (по раскладке виджетов).
    pub(super) fn shutter_center(&self) -> Point {
        self.center_of(|geometry| geometry.shutter())
    }

    fn center_of(&self, rect: impl Fn(&crate::widget::geometry::ScreenGeometry) -> ui_core::geometry::Rect) -> Point {
        let layout = *self.screen.state().layout().expect("раскладка");
        let tokens = UiKitTokens::default();
        let geometry = crate::widget::geometry::ScreenGeometry::new(
            &layout,
            &self.screen.config.widgets,
            &self.screen.config.zoom,
            &tokens,
        );
        rect(&geometry).center()
    }

    /// Рамка виджета в pt (по раскладке виджетов этого экрана).
    pub(super) fn rect_with(
        &self,
        rect: impl Fn(&crate::widget::geometry::ScreenGeometry) -> ui_core::geometry::Rect,
    ) -> ui_core::geometry::Rect {
        let layout = *self.screen.state().layout().expect("раскладка");
        let tokens = UiKitTokens::default();
        let geometry = crate::widget::geometry::ScreenGeometry::new(
            &layout,
            &self.screen.config.widgets,
            &self.screen.config.zoom,
            &tokens,
        );
        rect(&geometry)
    }

    /// Точка раскладки виджетов в pt (например, центр заголовка карточки).
    pub(super) fn point_with(&self, point: impl Fn(&crate::widget::geometry::ScreenGeometry) -> Point) -> Point {
        let layout = *self.screen.state().layout().expect("раскладка");
        let tokens = UiKitTokens::default();
        let geometry = crate::widget::geometry::ScreenGeometry::new(
            &layout,
            &self.screen.config.widgets,
            &self.screen.config.zoom,
            &tokens,
        );
        point(&geometry)
    }

    pub(super) fn center_with(
        &self,
        rect: impl Fn(&crate::widget::geometry::ScreenGeometry) -> ui_core::geometry::Rect,
    ) -> Point {
        self.center_of(rect)
    }
}

/// Рамка затвора на поднятом месте: центр на `raised_shutter_offset_pt` выше обычного. В рабочем коде затвор
/// едет сдвигом узла, рамка нужна только проверкам.
pub(super) fn raised_shutter(geometry: &crate::widget::geometry::ScreenGeometry) -> ui_core::geometry::Rect {
    let shutter = geometry.shutter();
    ui_core::geometry::Rect::from_xywh(
        shutter.min_x(),
        shutter.min_y() - geometry.config().card.raised_shutter_offset_pt,
        shutter.size.width,
        shutter.size.height,
    )
}

/// Центры членов стекла (px) и их форма.
pub(super) fn glass_members(list: &DrawList) -> Vec<(ShapeGeometry, f32, f32)> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Glass(group) => Some(group),
            _ => None,
        })
        .flat_map(|group| {
            group
                .members
                .iter()
                .map(|m| (m.geometry, m.transform.tx, m.transform.ty))
        })
        .collect()
}

pub(super) fn text_count(list: &DrawList) -> usize {
    list.items
        .iter()
        .filter(|item| matches!(item, DrawItem::Text(_)))
        .count()
}

/// Преобразования всех иконок данного вида.
pub(super) fn icon_transforms(list: &DrawList, icon: ui_draw::icons::IconId) -> Vec<ui_draw::transform::Affine2> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Icon(i) if i.icon == icon => Some(i.transform),
            _ => None,
        })
        .collect()
}

/// Глубина затемнения при снимке: чёрная полупрозрачная фигура (подложка вне кадра — непрозрачная, не в счёт).
pub(super) fn blink_darkness(list: &DrawList) -> f32 {
    list.items
        .iter()
        .find_map(|item| match item {
            DrawItem::Shape(shape)
                if shape.fill == ui_draw::color::DisplayColor::BLACK
                    && shape.opacity.get() > 0.0
                    && shape.opacity.get() < 1.0 =>
            {
                Some(shape.opacity.get())
            }
            _ => None,
        })
        .unwrap_or(0.0)
}

impl Rig {
    /// Кадры с произвольным шагом до момента `end_ms`.
    pub(super) fn run_until(&mut self, end_ms: i64, step_ms: i64) -> &DrawList {
        while self.now_ms < end_ms {
            self.now_ms = (self.now_ms + step_ms).min(end_ms);
            self.frame();
        }
        &self.last
    }

    /// Применить действие и сразу построить кадр, на котором оно будет синхронизировано.
    pub(super) fn act(&mut self, action: ScreenAction) -> &DrawList {
        self.screen.handle_action(action);
        self.frame()
    }
}

/// Диски «солнца» рамки фокуса (круг Ø 9 pt, фигура, а не значок: `ui-kit` рисует «солнце» диском и лучами).
pub(super) fn sun_discs(list: &DrawList) -> Vec<(f32, f32)> {
    let disc_px = 9.0 * PX_PER_PT;
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Shape(shape)
                if matches!(shape.geometry, ShapeGeometry::Circle { diameter } if (diameter - disc_px).abs() < 1.0) =>
            {
                Some((shape.transform.tx, shape.transform.ty))
            }
            _ => None,
        })
        .collect()
}

/// pt → px этого стенда.
pub(super) fn px(points: f64) -> f32 {
    (points * f64::from(PX_PER_PT)) as f32
}

pub(super) fn near(actual: f32, expected: f32, tolerance: f32) -> bool {
    (actual - expected).abs() <= tolerance
}

/// Прогон текста так же, как его раскладывает экран: по равенству прогонов видно, что написано.
pub(super) fn run_of(text: &str, role: TextRoleKind) -> TextRun {
    let tokens = UiKitTokens::default();
    LabelPainter::new(&tokens.typography, PxPerPt::new(PX_PER_PT).expect("масштаб"))
        .layout(text, role)
        .expect("раскладка")
}

/// Преобразования всех нарисованных экземпляров этого текста этой ролью (с заметной прозрачностью).
pub(super) fn texts_of(list: &DrawList, text: &str, role: TextRoleKind) -> Vec<ui_draw::draw_list::TextItem> {
    let expected = run_of(text, role);
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(drawn) if drawn.run == expected && drawn.opacity.get() > 0.05 => Some(drawn.clone()),
            _ => None,
        })
        .collect()
}

/// Сколько раз в списке нарисован этот текст этой ролью.
pub(super) fn text_count_of(list: &DrawList, text: &str, role: TextRoleKind) -> usize {
    texts_of(list, text, role).len()
}

pub(super) fn icons_of(list: &DrawList, icon: IconId) -> Vec<ui_draw::draw_list::IconItem> {
    list.items
        .iter()
        .filter_map(|item| match item {
            // Хвост пружины оставляет у ушедшего содержимого прозрачность ~0,001: «видимым» считаем > 0,05.
            DrawItem::Icon(drawn) if drawn.icon == icon && drawn.opacity.get() > 0.05 => Some(*drawn),
            _ => None,
        })
        .collect()
}

/// Размер (px) пилюли выбранного режима: единственная капсула-фигура с ободком (капсула-стекло — не фигура).
pub(super) fn mode_pill_size_px(list: &DrawList) -> (f32, f32) {
    list.items
        .iter()
        .find_map(|item| match item {
            DrawItem::Shape(shape) if shape.stroke.is_some() => match shape.geometry {
                ShapeGeometry::Capsule { size } => Some((size.width, size.height)),
                _ => None,
            },
            _ => None,
        })
        .expect("пилюля режима в списке рисования")
}
