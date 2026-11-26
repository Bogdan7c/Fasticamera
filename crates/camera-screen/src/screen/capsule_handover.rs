//! Передача эстафеты капсулы режимов между её двумя домами при смене PHOTO ↔ CALIBRATE.
//!
//! # Зачем
//! Капсула режимов живёт в двух деревьях сцены (решение 84): нижний ряд PHOTO и нижний ряд раздела калибровки. У
//! каждого дома своё состояние прокрутки (`SnapCarousel`), а палец тащит только тот, что виден. Режим переключается
//! при отпускании пальца, и в этот же кадр показывается другой дом. Скрытый дом всё это время стоял в покое на
//! режиме, который был ДО жеста, поэтому, увидев смену режима, он начал бы анимацию выбора от СВОЕГО покоя: подписи
//! отскакивали бы назад и ехали второй раз («двойной прыжок» при перетаскивании). Тап этого не показывает только
//! потому, что у обоих домов покой совпадает.
//!
//! # Почему передача, а не сброс
//! Сброс скрытого дома на покой дал бы тот же скачок, только в другую сторону: зритель только что видел подписи в
//! движении (отпущены с какой-то скоростью). Поэтому принимающий дом встаёт на положение и скорость уходящего и
//! дальше едет к выбранному режиму своей пружиной, наследуя скорость.
//!
//! # Владелец
//! `Stage` решает, какой дом показан, поэтому передачу делает он: до пересборки раздела снимается движение
//! уходящего раздела (раздел может исчезнуть при смене режима), после смены видимого дома оно отдаётся принимающему.

use crate::widget::Ctx;
use crate::widget::calibration::CalibrationSection;
use crate::widget::error::WidgetError;
use crate::widget::mode_carousel::ModeCarouselWidget;
use ui_kit::carousel::CarouselMotion;

/// Какой дом капсулы становится видимым.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArrivingHome {
    /// Нижний ряд экрана PHOTO.
    Photo,
    /// Нижний ряд раздела калибровки.
    Section,
}

/// Движение капсулы раздела (`None` — раздела нет или капсула в покое), снятое ДО возможной пересборки (уходящий раздел может быть снят при смене режима).
#[derive(Debug, Clone, Copy)]
pub(super) struct CapsuleHandover {
    section_motion_before: Option<CarouselMotion>,
}

impl CapsuleHandover {
    /// Снять движение капсулы текущего раздела (если он есть) на момент `ctx.now`.
    pub(super) fn capture(section: Option<&CalibrationSection>, ctx: &Ctx<'_>) -> Self {
        Self {
            section_motion_before: section.and_then(|section| section.capsule_motion(ctx)),
        }
    }

    /// Отдать эстафету принимающему дому `arriving`. Нет источника (раздела не было или капсула уходящего дома в
    /// покое) или принимающего (раздел не построен) — ничего не делает: принимающий дом остаётся на своём покое.
    pub(super) fn hand_over(
        self,
        arriving: ArrivingHome,
        photo: &mut ModeCarouselWidget,
        section: Option<&mut CalibrationSection>,
        ctx: &Ctx<'_>,
    ) -> Result<(), WidgetError> {
        match arriving {
            ArrivingHome::Photo => match self.section_motion_before {
                Some(motion) => photo.adopt_motion(motion, ctx),
                None => Ok(()),
            },
            ArrivingHome::Section => match section {
                Some(section) => match photo.motion_snapshot(ctx) {
                    Some(motion) => section.adopt_capsule_motion(motion, ctx),
                    None => Ok(()),
                },
                None => Ok(()),
            },
        }
    }
}
