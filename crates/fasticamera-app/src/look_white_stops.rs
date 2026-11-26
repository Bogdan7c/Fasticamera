//! Упоры ручек теплоты и оттенка для экрана калибровки (решения 288, 293): читающая сторона доски рендера.
//!
//! Считает упоры поток рендера (`render_loop`, кэш параметров ISP: там живёт профиль цвета камеры и реальная белая
//! сцены); здесь приложение только читает последнее значение доски. Доска и словарь — `look_contract::white_stops`.
//!
//! # Следующий шаг (не сделан здесь)
//! Подключение к экрану: факт `LookWhiteStops` нужно донести до `camera-screen` (источник фактов экрана при смене,
//! как `CameraFactsWatcher`), а экран нарисует по нему отметку упора на шкалах теплоты и оттенка. Это правка
//! `camera-screen`, поэтому отдельный шаг; ни `ScreenAction`, ни поле в `LookFacts` здесь не добавляются.
//!
//! # Отказ чтения
//! Счётчика «доска нечитаема» нет: замок доски держится на копирование одного значения, отравленный замок
//! безопасен и читается как обычный (см. `LookWhiteStopsBoard`), так что отказа чтения у доски не бывает.

use look_contract::{LookWhiteStops, LookWhiteStopsBoard};

/// Читатель доски упоров. Клон дёшев и делит доску.
#[derive(Debug, Clone)]
pub struct LookWhiteStopsReader {
    board: LookWhiteStopsBoard,
}

impl LookWhiteStopsReader {
    /// Читатель той же доски, в которую пишет рендер.
    pub fn new(board: LookWhiteStopsBoard) -> Self {
        Self { board }
    }

    /// Последнее значение: [`LookWhiteStops::Unknown`] до первой публикации и после конца потока,
    /// [`LookWhiteStops::NotApplicable`], если сдвиг белой не применяется вовсе, иначе посчитанные упоры.
    pub fn latest_stops(&self) -> LookWhiteStops {
        self.board.latest()
    }
}

#[cfg(test)]
mod tests {
    use look_contract::{KnobSideStops, KnobStop, KnobTravelStops, StopFraction};

    use super::*;
    use crate::camera_wiring::CameraLinks;
    use render_loop::viewfinder::WhiteLinks;

    fn tint_stop_at(fraction: f32) -> LookWhiteStops {
        let none = KnobSideStops {
            minus: KnobStop::None,
            plus: KnobStop::None,
        };
        LookWhiteStops::Known(KnobTravelStops {
            warmth: none,
            tint: KnobSideStops {
                plus: KnobStop::At(StopFraction::new(fraction).expect("доля в (0, 1]")),
                ..none
            },
        })
    }

    #[test]
    fn the_reader_is_unknown_before_the_first_publish_and_returns_what_render_published() {
        let board = LookWhiteStopsBoard::new();
        let reader = LookWhiteStopsReader::new(board.clone());
        assert_eq!(reader.latest_stops(), LookWhiteStops::Unknown);

        board.publish(tint_stop_at(0.34));
        assert_eq!(reader.latest_stops(), tint_stop_at(0.34));

        board.publish(LookWhiteStops::NotApplicable);
        assert_eq!(reader.clone().latest_stops(), LookWhiteStops::NotApplicable);
    }

    /// Рендерная половина связи пишет в ту же доску, что читает приложение.
    #[test]
    fn what_the_render_side_of_the_links_publishes_the_app_side_reads() {
        let links = CameraLinks::default();
        let reader = LookWhiteStopsReader::new(links.look_white_stops.clone());
        let render: WhiteLinks = links
            .render_side(crate::auto_exposure_link::ExposureControllerKind::Vendor)
            .white;
        assert_eq!(reader.latest_stops(), LookWhiteStops::Unknown);

        render.look_white_stops.publish(tint_stop_at(0.5));
        assert_eq!(reader.latest_stops(), tint_stop_at(0.5));
    }
}
