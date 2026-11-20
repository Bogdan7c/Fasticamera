//! Нарушения «раскрытия» переходного кадра при смене объектива (сессия 13): то, что `black_frames_while_picture_expected`
//! само по себе не ловит.
//!
//! Смена объектива держит на экране последний кадр старой камеры (размытый), пока новая камера не готова по цвету и
//! яркости. Ядро заявки фичи — **раскрытие по условию** (`holds_revealed_by_condition`). Любой другой исход держания —
//! дефект, даже если счётчик чёрных кадров «там, где ждали картинку» остался нулём:
//! * раскрытие **по сроку** (`max_hold_ms`, 1500 мс меньше порога первого живого кадра 2000 мс): условие готовности
//!   цвета и яркости не сработало, переход «прошёл» только потому, что вышло время;
//! * держание кончилось **честным чёрным** (потока нет / кадры идут, показать нечего): переход закончился чёрным
//!   экраном, а такие кадры считаются допустимыми только вне сценария смены объектива.
//!
//! Структура — приращения тех же счётчиков непрерывности рендера за окно (шаг или весь прогон); чистая функция от
//! двух снимков счётчиков, без часов.

use render_loop::viewfinder::ContinuityCounters;
use serde::Serialize;

/// Приращения счётчиков «плохих» исходов держания за окно. Нулевая структура — все переходы раскрылись по условию.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RevealFaults {
    /// Удержания, закончившиеся по сроку, а не по условию готовности.
    pub revealed_by_timeout: u64,
    /// Удержания, закончившиеся честным чёрным: потока не было слишком долго.
    pub ended_without_stream: u64,
    /// Удержания, закончившиеся честным чёрным: поток шлёт кадры, а показать нечего.
    pub ended_without_presentable_frame: u64,
    /// Чёрные кадры «камера недоступна» (после честного чёрного).
    pub black_camera_unavailable: u64,
    /// Чёрные кадры «камера стримит, показать нечего» (после честного чёрного).
    pub black_picture_unavailable: u64,
}

impl RevealFaults {
    /// Приращения от снимка `start` до снимка `end` (счётчики не убывают; убыль, если вдруг, даёт 0).
    pub fn between(start: &ContinuityCounters, end: &ContinuityCounters) -> Self {
        Self {
            revealed_by_timeout: end
                .holds_revealed_by_timeout
                .saturating_sub(start.holds_revealed_by_timeout),
            ended_without_stream: end
                .holds_ended_without_stream
                .saturating_sub(start.holds_ended_without_stream),
            ended_without_presentable_frame: end
                .holds_ended_without_presentable_frame
                .saturating_sub(start.holds_ended_without_presentable_frame),
            black_camera_unavailable: end
                .black_frames_camera_unavailable
                .saturating_sub(start.black_frames_camera_unavailable),
            black_picture_unavailable: end
                .black_frames_picture_unavailable
                .saturating_sub(start.black_frames_picture_unavailable),
        }
    }

    /// Ни одного плохого исхода.
    pub fn is_clean(&self) -> bool {
        *self == Self::default()
    }

    /// Человекочитаемое описание только ненулевых полей («раскрытий по сроку: 1, …»); для пустой структуры — пусто.
    pub fn describe(&self) -> String {
        [
            ("раскрытий по сроку вместо условия готовности", self.revealed_by_timeout),
            (
                "держаний, закончившихся честным чёрным без потока",
                self.ended_without_stream,
            ),
            (
                "держаний, закончившихся честным чёрным без годного кадра",
                self.ended_without_presentable_frame,
            ),
            ("чёрных кадров «камера недоступна»", self.black_camera_unavailable),
            ("чёрных кадров «картинка недоступна»", self.black_picture_unavailable),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(label, count)| format!("{label}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
    }
}

#[cfg(test)]
#[path = "zoom_reveal_tests.rs"]
mod tests;
