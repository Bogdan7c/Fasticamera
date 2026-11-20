//! Именованные пружины движения и сборка конфигов прокрутки из них.
//!
//! Правило каталога (`components.md`, раздел «Токены»): элемент ссылается на **имя** пружины,
//! а не на `duration`/`bounce`. Все числа живут здесь, с меткой происхождения.
//!
//! # Reduce Motion
//! Правило одно и живёт в `ui-core` ([`Spring::for_motion`]): пружина с перелётом теряет его,
//! длительность сохраняется (`motion.md` раздел 3: «упругость выключается»). Измеренные пружины
//! камеры и так без перелёта, меняются только `snappy`/`bouncy`. Токены правило не копируют:
//! [`MotionTokens::spring`] применяет его к готовой пружине, а движок прокрутки получает
//! предпочтение движения при создании и сам решает судьбу пружин ([`ui_core::scroll::ScrollAxis::with_motion`]).

use serde::{Deserialize, Serialize};
use ui_core::scroll::{ScrollConfig, SnapSelection};
use ui_core::spring::{Spring, SpringSpec};
use ui_core::time::MotionPreference;

use super::error::TokenError;

/// Пружина в терминах Apple `Spring(duration:bounce:)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringToken {
    /// Длительность в секундах (ощущаемое время, `motion.md` 1.1).
    pub duration_s: f64,
    /// Упругость: 0 — без перелёта, > 0 — перелёт, < 0 — перезатухание.
    pub bounce: f64,
}

impl SpringToken {
    fn spec(self) -> SpringSpec {
        SpringSpec::DurationBounce {
            duration_s: self.duration_s,
            bounce: self.bounce,
        }
    }
}

/// Имя пружины движения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MotionToken {
    /// Стандартная Apple `.smooth`.
    Smooth,
    /// Стандартная Apple `.snappy`.
    Snappy,
    /// Стандартная Apple `.bouncy`.
    Bouncy,
    /// Прилипание карусели после свайпа (`camera.snap`).
    CameraSnap,
    /// Открытие/закрытие меню и панели (`camera.sheet`).
    CameraSheet,
    /// Прилипание шкалы к упору после броска.
    ScaleSnap,
    /// Возврат из-за края («резинка»).
    EdgeReturn,
    /// Короткое проявление на месте без перелёта: открытие карточки со значка, растворение ряда.
    CardAppear,
    /// Морф/закрытие панели без перелёта: меню → карточка, закрытие карточки, подъём затвора.
    CardMorph,
}

impl MotionToken {
    /// Все токены — для проверок и тестов полноты.
    pub const ALL: [MotionToken; 9] = [
        MotionToken::Smooth,
        MotionToken::Snappy,
        MotionToken::Bouncy,
        MotionToken::CameraSnap,
        MotionToken::CameraSheet,
        MotionToken::ScaleSnap,
        MotionToken::EdgeReturn,
        MotionToken::CardAppear,
        MotionToken::CardMorph,
    ];

    /// Имя для сообщений об ошибках.
    pub const fn name(self) -> &'static str {
        match self {
            MotionToken::Smooth => "smooth",
            MotionToken::Snappy => "snappy",
            MotionToken::Bouncy => "bouncy",
            MotionToken::CameraSnap => "camera.snap",
            MotionToken::CameraSheet => "camera.sheet",
            MotionToken::ScaleSnap => "scale.snap",
            MotionToken::EdgeReturn => "edge.return",
            MotionToken::CardAppear => "card.appear",
            MotionToken::CardMorph => "card.morph",
        }
    }
}

/// Роль прокрутки: какая пружина прилипания и какая политика выбора точки нужны элементу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapRole {
    /// Карусель: листает на соседний элемент (`AdjacentOnly`), пружина `camera.snap`.
    Carousel,
    /// Шкала: ближайший упор к проекции броска, пружина `scale.snap`.
    Scale,
}

/// Все пружины движения и базовая прокрутка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MotionTokens {
    /// `[doc]` `motion.md` 1.5: `.smooth` = duration 0,5, bounce 0.
    pub smooth: SpringToken,
    /// `[doc]` `motion.md` 1.5: `.snappy` = duration 0,5, bounce 0,15.
    pub snappy: SpringToken,
    /// `[doc]` `motion.md` 1.5: `.bouncy` = duration 0,5, bounce 0,3.
    pub bouncy: SpringToken,
    /// `[measured]` `motion.md` 4.1: прилипание карусели, duration ≈ 0,85, bounce ≈ −0,13.
    pub camera_snap: SpringToken,
    /// `[measured]` `motion.md` 4.1: открытие меню, duration ≈ 0,45, bounce ≈ −0,11.
    pub camera_sheet: SpringToken,
    /// `[estimate]` — подгонка, не Apple: прилипание шкалы, `.smooth`-подобное (`motion.md` 4.2).
    pub scale_snap: SpringToken,
    /// `[estimate]` — подгонка, не Apple: возврат из-за края, как в умолчании `ui-core`.
    pub edge_return: SpringToken,
    /// `[measured: C t=47.93–48.12]` iOS 27: перекрёстное растворение карточки 0,15–0,2 с, без перелёта.
    pub card_appear: SpringToken,
    /// `[measured: C t=37.8–38.1, 62.93–63.25]` iOS 27: шторка → карточка и закрытие ≈ 0,3 с, без перелёта.
    pub card_morph: SpringToken,
    /// Замедление, «резинка» и критерий покоя — умолчания `ui-core` (там же их метки).
    /// Поля `snap_spring`, `edge_return_spring` и `snap_selection` здесь **перекрываются**
    /// ролью ([`MotionTokens::scroll_config`]).
    pub scroll: ScrollConfig,
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self {
            smooth: SpringToken {
                duration_s: 0.5,
                bounce: 0.0,
            },
            snappy: SpringToken {
                duration_s: 0.5,
                bounce: 0.15,
            },
            bouncy: SpringToken {
                duration_s: 0.5,
                bounce: 0.3,
            },
            camera_snap: SpringToken {
                duration_s: 0.85,
                bounce: -0.13,
            },
            camera_sheet: SpringToken {
                duration_s: 0.45,
                bounce: -0.11,
            },
            scale_snap: SpringToken {
                duration_s: 0.5,
                bounce: 0.0,
            },
            edge_return: SpringToken {
                duration_s: 0.4,
                bounce: 0.0,
            },
            card_appear: SpringToken {
                duration_s: 0.2,
                bounce: 0.0,
            },
            card_morph: SpringToken {
                duration_s: 0.3,
                bounce: 0.0,
            },
            scroll: ScrollConfig::default(),
        }
    }
}

impl MotionTokens {
    /// Значения токена как заданы (без учёта Reduce Motion: оно применяется к готовой пружине).
    pub fn token(&self, token: MotionToken) -> SpringToken {
        match token {
            MotionToken::Smooth => self.smooth,
            MotionToken::Snappy => self.snappy,
            MotionToken::Bouncy => self.bouncy,
            MotionToken::CameraSnap => self.camera_snap,
            MotionToken::CameraSheet => self.camera_sheet,
            MotionToken::ScaleSnap => self.scale_snap,
            MotionToken::EdgeReturn => self.edge_return,
            MotionToken::CardAppear => self.card_appear,
            MotionToken::CardMorph => self.card_morph,
        }
    }

    /// Готовая пружина `ui-core` по имени токена с учётом Reduce Motion.
    pub fn spring(&self, token: MotionToken, motion: MotionPreference) -> Result<Spring, TokenError> {
        let spring = self
            .token(token)
            .spec()
            .build()
            .map_err(|source| TokenError::InvalidSpring {
                name: token.name(),
                source,
            })?;
        Ok(spring.for_motion(motion))
    }

    /// Конфиг прокрутки для роли элемента: базовая прокрутка + пружины и политика роли.
    /// Reduce Motion сюда не входит: его получает движок прокрутки при создании
    /// ([`ui_core::scroll::ScrollAxis::with_motion`]) и может сменить на лету.
    pub fn scroll_config(&self, role: SnapRole) -> Result<ScrollConfig, TokenError> {
        let (snap_token, snap_selection) = match role {
            SnapRole::Carousel => (MotionToken::CameraSnap, SnapSelection::AdjacentOnly),
            SnapRole::Scale => (MotionToken::ScaleSnap, SnapSelection::NearestToProjection),
        };
        let config = ScrollConfig {
            snap_spring: self.token(snap_token).spec(),
            edge_return_spring: self.token(MotionToken::EdgeReturn).spec(),
            snap_selection,
            ..self.scroll.clone()
        };
        config.validate()?;
        Ok(config)
    }

    /// Проверка всех токенов в обоих режимах движения.
    pub fn validate(&self) -> Result<(), TokenError> {
        for motion in [MotionPreference::Full, MotionPreference::Reduced] {
            for token in MotionToken::ALL {
                self.spring(token, motion)?;
            }
        }
        self.scroll_config(SnapRole::Carousel)?;
        self.scroll_config(SnapRole::Scale)?;
        Ok(())
    }
}
