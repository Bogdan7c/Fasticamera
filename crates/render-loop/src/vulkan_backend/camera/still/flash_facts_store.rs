//! Что движок снимков знает о свете для кадра со вспышкой: планы движка камеры, белая светодиода, свет сцены.
//!
//! Данные приходят от видоискателя лентой ([`super::feed::StillFeed`]) и нужны не в момент прихода, а когда
//! кадр со вспышкой дойдёт до кольца (сотни миллисекунд позже): поэтому хранятся здесь, ограниченно.
//! Привязка к потоку — по `Arc::ptr_eq` (решение 42): оценки одной камеры другой не годятся.

use std::collections::VecDeque;
use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::{FlashFramePlan, FlashShotId};
use camera_contract::frame_passport::StreamPassport;

use crate::vulkan_backend::camera::flash_facts::{AmbientLevel, TorchWhite};

/// Сколько планов кадров со вспышкой помнить: следующая предвспышка заменяет указание раньше, чем прежний
/// кадр дойдёт до проявления, но одновременно ждут единицы снимков.
const MAX_REMEMBERED_PLANS: usize = 4;

/// Сколько уровней света сцены по кадрам помнить. Кадр плана — это кадр ДО запроса фонарика, он старше
/// кадра со вспышкой на десятки кадров (предвспышка, пауза, результат ≈ 1 с при 30–60 к/с); 32 уровней
/// (замеры идут по расписанию статистики, не на каждом кадре) покрывают это с запасом.
const MAX_REMEMBERED_AMBIENT_LEVELS: usize = 32;

/// Хранилище фактов о свете (см. модуль).
#[derive(Default)]
pub struct FlashFactsStore {
    plans: VecDeque<FlashFramePlan>,
    torch_white: Option<TorchWhite>,
    /// Уровни света сцены без вспышки по кадрам, старые первыми.
    ambient_levels: VecDeque<AmbientLevel>,
}

impl FlashFactsStore {
    /// Запоминает присланное лентой. План того же снимка заменяет прежний; старейший план вытесняется.
    pub fn record(
        &mut self,
        plans: Vec<FlashFramePlan>,
        torch_whites: Vec<TorchWhite>,
        ambient_levels: Vec<AmbientLevel>,
    ) {
        for plan in plans {
            self.plans.retain(|known| known.shot != plan.shot);
            self.plans.push_back(plan);
            while self.plans.len() > MAX_REMEMBERED_PLANS {
                self.plans.pop_front();
            }
        }
        // Нужна белая по САМОМУ СВЕЖЕМУ кадру с фонариком (по метке сенсора, а не порядку прихода).
        for white in torch_whites {
            let newer = self.torch_white.as_ref().is_none_or(|known| known.frame <= white.frame);
            if newer {
                self.torch_white = Some(white);
            }
        }
        // Уровень привязан к кадру (план называет кадр, свет которого считал): «последний» уровень мог бы
        // оказаться кадром уже под светодиодом. Тот же кадр того же потока заменяется, лишнее вытесняется.
        for level in ambient_levels {
            self.ambient_levels
                .retain(|known| !(known.frame == level.frame && Arc::ptr_eq(&known.stream, &level.stream)));
            self.ambient_levels.push_back(level);
            while self.ambient_levels.len() > MAX_REMEMBERED_AMBIENT_LEVELS {
                self.ambient_levels.pop_front();
            }
        }
    }

    /// План кадра со вспышкой снимка `shot`.
    pub fn plan_for(&self, shot: FlashShotId) -> Option<FlashFramePlan> {
        self.plans.iter().find(|plan| plan.shot == shot).copied()
    }

    /// Последняя белая светодиода для кадров потока `stream`.
    pub fn torch_white_for(&self, stream: &Arc<StreamPassport>) -> Option<&TorchWhite> {
        self.torch_white
            .as_ref()
            .filter(|white| Arc::ptr_eq(&white.stream, stream))
    }

    /// Уровень света сцены без вспышки кадра `frame` потока `stream`. `None` — такого кадра не измеряли
    /// или он уже вытеснен: подставлять другой кадр нельзя (см. `FlashFramePlan::ambient_frame`).
    pub fn ambient_level_of(&self, stream: &Arc<StreamPassport>, frame: SensorTimestamp) -> Option<&AmbientLevel> {
        self.ambient_levels
            .iter()
            .find(|level| level.frame == frame && Arc::ptr_eq(&level.stream, stream))
    }

    /// Забыть всё (surface отпущена: поток закончился; планы тоже — следующий снимок получит новые).
    pub fn clear(&mut self) {
        self.plans.clear();
        self.torch_white = None;
        self.ambient_levels.clear();
    }
}
