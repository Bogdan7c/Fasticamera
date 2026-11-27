//! Подписи прибора для экрана (решения 326, 327): читающая сторона доски рендера.
//!
//! Положения подписей (буквы целей вектороскопа, позже имена треугольников охвата CIE) считает поток рендера по
//! решённому правилу прибора (`scope-model`); здесь приложение только читает последнее значение доски. Доска и словарь —
//! `scope_contract::labels`.
//!
//! # Дорога к экрану
//! [`ScopeLabelsWatcher`] читает доску на каждом кадре UI и отдаёт значение ТОЛЬКО когда оно изменилось
//! ([`ScreenAction::ScopeLabelsChanged`], как упоры ручек у `LookWhiteStopsWatcher`). До первого сообщения экран
//! считает, что подписей нет, поэтому «не публиковалось» само по себе не отправляется. Закрытие приборов и смена запроса
//! (подписи другого прибора) — изменения, и экран их получает.
//!
//! # Отказ чтения
//! Счётчика «доска нечитаема» нет: замок доски держится на замену одного значения целиком, отравленный замок безопасен
//! (см. `ScopeLabelsBoard`).

use scope_contract::{ScopeLabelsBoard, ScopeLabelsState};

/// Читатель доски подписей. Клон дёшев и делит доску.
#[derive(Debug, Clone)]
pub struct ScopeLabelsReader {
    board: ScopeLabelsBoard,
}

impl ScopeLabelsReader {
    /// Читатель той же доски, в которую пишет рендер.
    pub fn new(board: ScopeLabelsBoard) -> Self {
        Self { board }
    }

    /// Последнее значение: [`ScopeLabelsState::NotPublished`] до первой публикации.
    pub fn latest_labels(&self) -> ScopeLabelsState {
        self.board.latest()
    }
}

/// Наблюдатель за подписями для экрана: помнит последнее ОТПРАВЛЕННОЕ значение и отдаёт только изменения.
#[derive(Debug, Clone)]
pub struct ScopeLabelsWatcher {
    reader: ScopeLabelsReader,
    /// Что экран уже знает. Начинается с `NotPublished`: так экран выглядит до первого сообщения.
    last_sent: ScopeLabelsState,
}

impl ScopeLabelsWatcher {
    /// Наблюдатель за доской, в которую пишет рендер.
    pub fn new(board: ScopeLabelsBoard) -> Self {
        Self {
            reader: ScopeLabelsReader::new(board),
            last_sent: ScopeLabelsState::NotPublished,
        }
    }

    /// Наблюдатель без источника: доска пуста навсегда, экрану ничего не отправляется (сборки без рендера, тесты).
    pub fn silent() -> Self {
        Self::new(ScopeLabelsBoard::new())
    }

    /// Значение, если оно изменилось с прошлого вызова; `None` — экран уже знает текущее.
    pub fn changed(&mut self) -> Option<ScopeLabelsState> {
        let latest = self.reader.latest_labels();
        if latest == self.last_sent {
            return None;
        }
        self.last_sent = latest.clone();
        Some(latest)
    }
}

#[cfg(test)]
mod tests {
    use scope_contract::{
        LabelPoint, ScopeImageSize, ScopeKind, ScopeLabel, ScopeLabels, ScopeMark, ScopeRequest, ScopeScan,
        TargetColor, VectorscopeAxes, VectorscopeZoom,
    };

    use super::*;
    use crate::camera_wiring::CameraLinks;
    use render_loop::viewfinder::WhiteLinks;

    fn request(zoom: VectorscopeZoom) -> ScopeRequest {
        ScopeRequest {
            kind: ScopeKind::Vectorscope,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: zoom,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: scope_contract::HistogramMode::default(),
            scan: ScopeScan::Upright,
            image_size: ScopeImageSize::try_new(300, 300).expect("размер в допуске"),
        }
    }

    fn red_at(x: f32) -> ScopeLabelsState {
        ScopeLabelsState::Published(ScopeLabels {
            request: request(VectorscopeZoom::Normal),
            labels: vec![ScopeLabel {
                mark: ScopeMark::VectorscopeTarget(TargetColor::Red),
                at: LabelPoint::try_new(x, 0.2).expect("точка в картинке"),
            }],
        })
    }

    #[test]
    fn the_reader_is_not_published_before_the_first_publish_and_returns_what_render_published() {
        let board = ScopeLabelsBoard::new();
        let reader = ScopeLabelsReader::new(board.clone());
        assert_eq!(reader.latest_labels(), ScopeLabelsState::NotPublished);

        board.publish(red_at(0.4));
        assert_eq!(reader.latest_labels(), red_at(0.4));
        board.publish(ScopeLabelsState::Withdrawn);
        assert_eq!(reader.clone().latest_labels(), ScopeLabelsState::Withdrawn);
    }

    #[test]
    fn the_watcher_reports_each_change_once_and_stays_quiet_while_nothing_changes() {
        let board = ScopeLabelsBoard::new();
        let mut watcher = ScopeLabelsWatcher::new(board.clone());
        assert_eq!(
            watcher.changed(),
            None,
            "до первой публикации экран и так считает, что подписей нет"
        );

        board.publish(red_at(0.4));
        assert_eq!(watcher.changed(), Some(red_at(0.4)));
        assert_eq!(watcher.changed(), None, "то же значение второй раз не отправляется");
        board.publish(red_at(0.4));
        assert_eq!(
            watcher.changed(),
            None,
            "повторная публикация того же значения — не изменение"
        );

        // Подписи того же запроса, но в другом месте (другой охват экрана) — изменение.
        board.publish(red_at(0.45));
        assert_eq!(watcher.changed(), Some(red_at(0.45)));
        board.publish(ScopeLabelsState::Withdrawn);
        assert_eq!(
            watcher.changed(),
            Some(ScopeLabelsState::Withdrawn),
            "закрытие приборов убирает буквы"
        );
        assert_eq!(watcher.changed(), None);
    }

    #[test]
    fn a_silent_watcher_never_reports_anything() {
        let mut watcher = ScopeLabelsWatcher::silent();
        for _ in 0..5 {
            assert_eq!(watcher.changed(), None);
        }
    }

    #[test]
    fn what_the_render_side_of_the_links_publishes_the_app_side_reads() {
        let links = CameraLinks::default();
        let reader = ScopeLabelsReader::new(links.scope_labels.clone());
        let render: WhiteLinks = links
            .render_side(crate::auto_exposure_link::ExposureControllerKind::Vendor)
            .white;
        assert_eq!(reader.latest_labels(), ScopeLabelsState::NotPublished);

        render.scope_labels.publish(red_at(0.4));
        assert_eq!(reader.latest_labels(), red_at(0.4));
    }
}
