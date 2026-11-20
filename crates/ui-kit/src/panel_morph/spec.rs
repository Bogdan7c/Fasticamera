//! Описание панели-морфа: откуда растёт, куда, какой пружиной.

use ui_core::geometry::Rect;

use super::error::PanelError;
use crate::tokens::MotionToken;

/// Радиус вложенной формы по правилу концентричности (`glass.md` раздел 2, Apple 356, 3:49):
/// радиус родителя минус отступ; не меньше нуля.
///
/// Пример: панель с радиусом 40 pt и отступом пункта 8 pt → радиус пункта 32 pt.
pub fn concentric_radius_pt(parent_radius_pt: f64, inset_pt: f64) -> f64 {
    (parent_radius_pt - inset_pt).max(0.0)
}

/// Как панель появляется на экране.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMotionKind {
    /// Панель вырастает из рамки источника (кнопки) в рамку цели и сворачивается обратно.
    GrowFromSource,
    /// Панель проявляется «на месте»: рамка и радиус постоянны (равны цели), меняются только
    /// материализация стекла и содержимое (шторка iOS 27, нижняя карточка). Источника нет.
    InPlace,
}

/// На сколько pt сдвинуто содержимое закрытой панели (оно «въезжает» снизу к 0 при открытии).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentSlide {
    /// Расстояние берётся из токена `panel.content_slide_distance_pt`.
    FromTokens,
    /// Своё расстояние, pt (iOS 27: пункты шторки въезжают на 4,9 pt).
    Points(f64),
}

/// Описание панели. Рамки — в pt, в одних координатах со сценой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelMorphSpec {
    /// Рамка источника (кнопка, из которой растёт панель; у подменю — рамка пункта).
    /// Радиус источника — капсула/круг, то есть половина меньшей стороны.
    pub source: Rect,
    /// Рамка открытой панели.
    pub target: Rect,
    /// Радиус источника в закрытом состоянии, pt. `None` — капсула/круг (половина меньшей стороны).
    /// Нужен, когда источник — не кнопка, а панель со своим радиусом (шторка → карточка).
    pub source_corner_radius_pt: Option<f64>,
    /// Радиус скругления открытой панели, pt. Для вложенной панели берётся из
    /// [`concentric_radius_pt`], для самостоятельной — из раскладки.
    pub target_corner_radius_pt: f64,
    /// Имя пружины движения. Умолчание — `camera.sheet` (меню и панель); для морфа пункта в
    /// подменю экран передаёт [`MotionToken::Smooth`].
    pub spring: MotionToken,
    /// Как панель появляется: растёт из источника или проявляется на месте.
    pub motion_kind: PanelMotionKind,
    /// Расстояние въезда содержимого.
    pub content_slide: ContentSlide,
}

impl PanelMorphSpec {
    /// Панель с пружиной `camera.sheet`.
    pub fn new(source: Rect, target: Rect, target_corner_radius_pt: f64) -> Self {
        Self {
            source,
            target,
            source_corner_radius_pt: None,
            target_corner_radius_pt,
            spring: MotionToken::CameraSheet,
            motion_kind: PanelMotionKind::GrowFromSource,
            content_slide: ContentSlide::FromTokens,
        }
    }

    /// Панель, проявляющаяся на месте: рамка всё время равна `target`, радиус — `corner_radius_pt`.
    /// Рамка источника совпадает с целью (закрытая панель «стоит» там же, но стекло не материализовано).
    pub fn in_place(target: Rect, corner_radius_pt: f64, spring: MotionToken) -> Self {
        Self {
            source: target,
            target,
            source_corner_radius_pt: None,
            target_corner_radius_pt: corner_radius_pt,
            spring,
            motion_kind: PanelMotionKind::InPlace,
            content_slide: ContentSlide::FromTokens,
        }
    }

    /// Та же панель, но радиус источника задан явно, pt (`[measured: C]` шторка 54 → карточка 52).
    /// Без вызова радиус источника — половина меньшей стороны (круглая кнопка).
    pub fn with_source_corner_radius_pt(self, radius_pt: f64) -> Self {
        Self {
            source_corner_radius_pt: Some(radius_pt),
            ..self
        }
    }

    /// Та же панель с другим расстоянием въезда содержимого, pt.
    pub fn with_content_slide_pt(self, distance_pt: f64) -> Self {
        Self {
            content_slide: ContentSlide::Points(distance_pt),
            ..self
        }
    }

    /// Та же панель с другой пружиной.
    pub fn with_spring(self, spring: MotionToken) -> Self {
        Self { spring, ..self }
    }

    pub(super) fn validate(&self) -> Result<(), PanelError> {
        for (which, frame) in [("source", self.source), ("target", self.target)] {
            let finite = [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]
                .iter()
                .all(|value| value.is_finite());
            if !finite || frame.size.width <= 0.0 || frame.size.height <= 0.0 {
                return Err(PanelError::InvalidFrame { which, frame });
            }
        }
        if !self.target_corner_radius_pt.is_finite() || self.target_corner_radius_pt < 0.0 {
            return Err(PanelError::InvalidRadius(self.target_corner_radius_pt));
        }
        if let Some(radius) = self.source_corner_radius_pt
            && (!radius.is_finite() || radius < 0.0)
        {
            return Err(PanelError::InvalidRadius(radius));
        }
        if let ContentSlide::Points(distance) = self.content_slide
            && (!distance.is_finite() || distance < 0.0)
        {
            return Err(PanelError::InvalidContentSlide(distance));
        }
        Ok(())
    }
}
