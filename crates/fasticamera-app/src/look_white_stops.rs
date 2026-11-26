//! Упоры ручек теплоты и оттенка для экрана калибровки (решения 288, 293): читающая сторона доски рендера.
//!
//! Считает упоры поток рендера (`render_loop`, кэш параметров ISP: там живёт профиль цвета камеры и реальная белая
//! сцены); здесь приложение только читает последнее значение доски. Доска и словарь — `look_contract::white_stops`.
//!
//! # Дорога к экрану
//! [`LookWhiteStopsWatcher`] читает доску на каждом кадре UI и отдаёт упоры ТОЛЬКО когда они изменились
//! ([`ScreenAction::LookWhiteStopsChanged`], как факты камеры у `CameraFactsWatcher`); экран рисует по ним отметки на
//! шкалах теплоты и оттенка. До первого сообщения экран считает упоры неизвестными, поэтому «неизвестно» сам по
//! себе не отправляется.
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

/// Наблюдатель за упорами для экрана: помнит последнее ОТПРАВЛЕННОЕ значение и отдаёт только изменения.
#[derive(Debug, Clone)]
pub struct LookWhiteStopsWatcher {
    reader: LookWhiteStopsReader,
    /// Что экран уже знает. Начинается с `Unknown`: так экран выглядит до первого сообщения.
    last_sent: LookWhiteStops,
}

impl LookWhiteStopsWatcher {
    /// Наблюдатель за доской, в которую пишет рендер.
    pub fn new(board: LookWhiteStopsBoard) -> Self {
        Self {
            reader: LookWhiteStopsReader::new(board),
            last_sent: LookWhiteStops::Unknown,
        }
    }

    /// Наблюдатель без источника: доска пуста навсегда, экрану ничего не отправляется (сборки без рендера, тесты).
    pub fn silent() -> Self {
        Self::new(LookWhiteStopsBoard::new())
    }

    /// Упоры, если они изменились с прошлого вызова; `None` — экран уже знает текущее значение. Возврат к
    /// `Unknown` тоже изменение (поток закончился, отметки надо убрать).
    pub fn changed(&mut self) -> Option<LookWhiteStops> {
        let latest = self.reader.latest_stops();
        if latest == self.last_sent {
            return None;
        }
        self.last_sent = latest;
        Some(latest)
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
    fn the_watcher_reports_each_change_once_and_stays_quiet_while_nothing_changes() {
        let board = LookWhiteStopsBoard::new();
        let mut watcher = LookWhiteStopsWatcher::new(board.clone());
        assert_eq!(
            watcher.changed(),
            None,
            "до первой публикации экран и так считает упоры неизвестными"
        );

        board.publish(tint_stop_at(0.46));
        assert_eq!(watcher.changed(), Some(tint_stop_at(0.46)));
        assert_eq!(watcher.changed(), None, "то же значение второй раз не отправляется");
        board.publish(tint_stop_at(0.46));
        assert_eq!(
            watcher.changed(),
            None,
            "повторная публикация того же значения — не изменение"
        );

        board.publish(tint_stop_at(0.5));
        assert_eq!(watcher.changed(), Some(tint_stop_at(0.5)));
        board.publish(LookWhiteStops::Unknown);
        assert_eq!(
            watcher.changed(),
            Some(LookWhiteStops::Unknown),
            "конец потока убирает отметки"
        );
        assert_eq!(watcher.changed(), None);
    }

    #[test]
    fn a_silent_watcher_never_reports_anything() {
        let mut watcher = LookWhiteStopsWatcher::silent();
        for _ in 0..5 {
            assert_eq!(watcher.changed(), None);
        }
    }

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
