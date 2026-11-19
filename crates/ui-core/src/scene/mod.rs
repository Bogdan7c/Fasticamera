//! Сцена экрана: дерево, жесты, анимации и часы под одной крышей.
//!
//! [`UiScene`] — то, чем владеет экран (`camera-screen`). Она сшивает готовые механизмы `ui-core`:
//! касание оболочки в **пикселях** → pt ([`crate::input::PointScale`]) → путь хит-теста дерева →
//! арена жестов; кадр → часы → таймеры жестов → аниматор свойств → дерево.
//!
//! # Владение и инварианты
//! * Дерево, арена, аниматор и часы принадлежат сцене; `&mut UiTree` наружу **не выдаётся**. Так
//!   нельзя удалить элемент мимо сцены и оставить жесты и анимации на мёртвом идентификаторе:
//!   удаление существует только как [`UiScene::remove_subtree`]. Свойства, которые ездят пружиной
//!   (прозрачность, масштаб, поворот), задаются через [`UiScene::set_property`], которое снимает
//!   текущую анимацию: иначе следующий кадр молча перезаписал бы ручное значение.
//! * Время — снаружи: метки касаний и время vsync. Внутри нет `Instant::now()`.
//!
//! # Порядок внутри кадра
//! 1. **Вход** (касания, дыры в потоке) применяется сразу при вызове [`UiScene::handle_touch`] — то
//!    есть *до* анимаций кадра. Экран забирает события ([`UiScene::drain_gesture_events`]) и
//!    реагирует (запускает/перенацеливает анимации с `now` = время события).
//! 2. [`UiScene::begin_frame`]: часы → таймеры жестов (долгий тап срабатывает и при неподвижном
//!    пальце). События таймеров экран тоже забирает.
//! 3. [`UiScene::apply_animations`]: аниматор пишет значения кадра в дерево — после реакции на жесты
//!    этого кадра, поэтому анимация, начатая жестом, видна в том же кадре.
//!
//! [`UiScene::advance_frame`] делает шаги 2–3 подряд (событие таймера внутри него обработается на
//! следующем кадре — задержка не больше кадра). Экран, которому важен нулевой лаг на таймерных
//! событиях, вызывает `begin_frame`, реагирует, затем `apply_animations`.
//!
//! # Рецепт: панель едет за пальцем и доезжает пружиной (визуальный сдвиг)
//! * Касание (`Began` распознавателя `Press`, одновременного с паном): `freeze_property(node, TranslationX)` —
//!   едущая панель замирает там, где её видно, **сразу при касании**, а не после гистерезиса пана
//!   (как прокрутка iOS: палец лёг — движение встало). Значение из `FreezeOutcome` — база ведения.
//! * `Began` пана: база = текущий сдвиг (после захвата он уже неподвижен).
//! * `Changed`: `set_property(node, TranslationX, база + from_recognition.dx)` — ведение 1:1.
//! * `Ended`: `animate` с целью 0, `StartVelocity::Explicit(velocity.dx)` и `now = event.time` —
//!   пружина стартует со скоростью пальца, без скачка положения.
//!
//! Сдвиг — визуальное свойство: `frame` и вёрстка не меняются, хит-тест и `visual_bounds` его
//! учитывают, дети двигаются вместе с родителем.
//!
//! # Смена размера surface
//! [`UiScene::resize_surface`] пересчитывает масштаб pt, размер экрана и вёрстку и **отменяет
//! идущие жесты** (как дыру в потоке): положения пальцев, накопленные в старом масштабе, нельзя
//! смешивать с новыми — скорость и смещения были бы мусором.

mod config;
mod error;
mod frame;
mod input;
mod tree_access;

pub use config::{SceneSettings, SurfaceGeometry};
pub use error::SceneError;
pub use frame::FrameReport;
pub use input::InputOutcome;
pub use tree_access::SubtreeRemoval;

use crate::animator::PropertyAnimator;
use crate::gesture::GestureArena;
use crate::input::{PointScale, PointScaleRule};
use crate::time::AnimationClock;
use crate::tree::UiTree;

/// Сцена экрана. `I` — тип намерения экрана, который арена возвращает в событиях жестов.
pub struct UiScene<I: Clone> {
    tree: UiTree,
    gestures: GestureArena<I>,
    animator: PropertyAnimator,
    clock: AnimationClock,
    scale_rule: PointScaleRule,
    scale: PointScale,
}

impl<I: Clone> UiScene<I> {
    /// Сцена с пустым деревом (только корень-экран) для surface `surface`.
    pub fn new(surface: SurfaceGeometry, settings: SceneSettings) -> Result<Self, SceneError> {
        let scale = PointScale::for_surface_width(settings.point_scale, surface.width_px)?;
        let tree = UiTree::new(surface.screen_size_pt(scale.pixels_per_point()), surface.safe_area)?;
        Ok(Self {
            tree,
            gestures: GestureArena::new(settings.gestures)?,
            animator: PropertyAnimator::new(settings.animator, settings.motion)?,
            clock: AnimationClock::new(settings.motion),
            scale_rule: settings.point_scale,
            scale,
        })
    }

    /// Дерево только для чтения.
    pub fn tree(&self) -> &UiTree {
        &self.tree
    }

    /// Текущий масштаб «пиксели ↔ pt».
    pub fn point_scale(&self) -> PointScale {
        self.scale
    }

    /// Аниматор только для чтения (цели, выборки, счётчики).
    pub fn animator(&self) -> &PropertyAnimator {
        &self.animator
    }

    /// Reduce Motion: действует на анимации, начатые после смены.
    pub fn set_motion(&mut self, motion: crate::time::MotionPreference) {
        self.clock.set_motion(motion);
        self.animator.set_motion(motion);
    }

    pub fn motion(&self) -> crate::time::MotionPreference {
        self.clock.motion()
    }

    /// Время последнего кадра; `None` до первого.
    pub fn frame_time(&self) -> Option<crate::time::MonotonicNanos> {
        self.clock.now()
    }

    /// Пересчитывает всё под новый размер surface (поворот, другое окно); идущие жесты отменяются.
    ///
    /// Если вёрстка вернула ошибку, масштаб, размер экрана и отмена жестов уже применены, а рамки
    /// остались прежними (вёрстка применяется только целиком).
    pub fn resize_surface(&mut self, surface: SurfaceGeometry) -> Result<(), SceneError> {
        let scale = PointScale::for_surface_width(self.scale_rule, surface.width_px)?;
        self.tree
            .resize_screen(surface.screen_size_pt(scale.pixels_per_point()), surface.safe_area)?;
        self.scale = scale;
        self.gestures.handle_stream_gap();
        self.tree.layout()?;
        Ok(())
    }
}
