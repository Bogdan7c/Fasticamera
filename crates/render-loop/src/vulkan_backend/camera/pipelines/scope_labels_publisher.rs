//! Публикация подписей прибора на доску для приложения (решения 326, 327).
//!
//! Подписи считает эталон `scope-model` ([`ResolvedScope::labels`]) по тому же решённому правилу, по которому рисуется
//! сетка прибора, поэтому положения букв совпадают с рамками целей. Здесь — только публикация НА ИЗМЕНЕНИЕ: владелец
//! приборов ([`super::scope_pass::ScopePass`]) зовёт [`ScopeLabelsPublisher::publish_for`], когда правило прибора
//! решено заново (смена запроса, зума, осей, размера картинки или охвата экрана), и [`ScopeLabelsPublisher::withdraw`],
//! когда приборы закрыты. Доска пишется, только если значение отличается от уже опубликованного.
//!
//! # Состояния доски
//! * приборы закрыты → [`ScopeLabelsState::Withdrawn`];
//! * прибор решён → [`ScopeLabelsState::Published`] с запросом, для которого посчитано (у waveform список пуст, но
//!   запрос всё равно помечен: экран не примет чужие подписи за свои).

use scope_contract::{ScopeLabels, ScopeLabelsBoard, ScopeLabelsState, ScopeRequest};
use scope_model::{RenderSettings, ResolvedScope};

/// Писатель доски подписей: помнит, что опубликовал, и пишет только изменения.
#[derive(Debug)]
pub(super) struct ScopeLabelsPublisher {
    board: ScopeLabelsBoard,
    /// Что лежит на доске сейчас.
    published: ScopeLabelsState,
}

impl ScopeLabelsPublisher {
    /// Писатель новой сессии показа. Подписи, оставшиеся от прежней сессии (рисовальщик пересоздан, приборы закрыты
    /// вместе с ним), не доверяются: [`ScopeLabelsState::Published`] на доске заменяется на «закрыто», пока эта
    /// сессия не решит прибор заново.
    pub(super) fn new(board: ScopeLabelsBoard) -> Self {
        let mut publisher = Self {
            published: board.latest(),
            board,
        };
        if matches!(publisher.published, ScopeLabelsState::Published(_)) {
            publisher.publish_if_changed(ScopeLabelsState::Withdrawn);
        }
        publisher
    }

    /// Прибор решён для `request`: публикует его подписи, если они отличаются от опубликованных.
    pub(super) fn publish_for(&mut self, request: &ScopeRequest, rule: &ResolvedScope, settings: &RenderSettings) {
        self.publish_if_changed(ScopeLabelsState::Published(ScopeLabels {
            request: *request,
            labels: rule.labels(settings),
        }));
    }

    /// Приборы закрыты: подписывать нечего.
    pub(super) fn withdraw(&mut self) {
        self.publish_if_changed(ScopeLabelsState::Withdrawn);
    }

    fn publish_if_changed(&mut self, desired: ScopeLabelsState) {
        if desired != self.published {
            self.board.publish(desired.clone());
            self.published = desired;
        }
    }
}

#[cfg(test)]
mod tests {
    use isp_model::image::DisplayGamut;
    use scope_contract::{
        ScopeImageSize, ScopeKind, ScopeMark, ScopeScan, TargetColor, VectorscopeAxes, VectorscopeZoom,
    };
    use scope_model::ScopeModelSettings;

    use super::*;

    fn request(kind: ScopeKind, zoom: VectorscopeZoom) -> ScopeRequest {
        ScopeRequest {
            kind,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: zoom,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: scope_contract::HistogramMode::default(),
            scan: ScopeScan::Upright,
            image_size: ScopeImageSize::try_new(512, 512).expect("размер в допуске"),
        }
    }

    fn resolve(request: &ScopeRequest) -> ResolvedScope {
        ResolvedScope::resolve(request, &ScopeModelSettings::default(), DisplayGamut::Srgb).expect("правило решается")
    }

    fn publish(publisher: &mut ScopeLabelsPublisher, request: &ScopeRequest) {
        publisher.publish_for(request, &resolve(request), &RenderSettings::default());
    }

    /// Чужая запись на доске: если писатель переписал доску, она исчезнет.
    fn mark_board(board: &ScopeLabelsBoard) -> ScopeLabelsState {
        let marker = ScopeLabelsState::Withdrawn;
        board.publish(marker.clone());
        marker
    }

    #[test]
    fn a_resolved_vectorscope_puts_its_six_letters_on_the_board_tagged_with_the_request() {
        let board = ScopeLabelsBoard::new();
        let mut publisher = ScopeLabelsPublisher::new(board.clone());
        assert_eq!(
            board.latest(),
            ScopeLabelsState::NotPublished,
            "до решения прибора доска пуста"
        );

        let asked = request(ScopeKind::Vectorscope, VectorscopeZoom::Normal);
        publish(&mut publisher, &asked);

        let ScopeLabelsState::Published(published) = board.latest() else {
            panic!("ждали подписи на доске");
        };
        assert_eq!(published.request, asked);
        assert_eq!(published.labels.len(), 6);
        assert!(
            published
                .labels
                .iter()
                .any(|label| label.mark == ScopeMark::VectorscopeTarget(TargetColor::Red)),
            "{published:?}"
        );
    }

    #[test]
    fn the_board_is_written_only_when_the_request_changes_not_on_every_resolution() {
        let board = ScopeLabelsBoard::new();
        let mut publisher = ScopeLabelsPublisher::new(board.clone());
        let normal = request(ScopeKind::Vectorscope, VectorscopeZoom::Normal);
        publish(&mut publisher, &normal);

        // Повторное решение того же запроса доску не трогает: чужая запись остаётся.
        let marker = mark_board(&board);
        publish(&mut publisher, &normal);
        assert_eq!(board.latest(), marker, "то же значение второй раз не пишется");

        // Другой зум — другой запрос, значит доска переписана.
        let zoomed = request(ScopeKind::Vectorscope, VectorscopeZoom::Double);
        publish(&mut publisher, &zoomed);
        let ScopeLabelsState::Published(published) = board.latest() else {
            panic!("ждали подписи на доске");
        };
        assert_eq!(published.request, zoomed);
    }

    #[test]
    fn a_waveform_publishes_an_empty_list_that_is_still_tagged_with_its_request() {
        let board = ScopeLabelsBoard::new();
        let mut publisher = ScopeLabelsPublisher::new(board.clone());
        let waveform = request(ScopeKind::Waveform, VectorscopeZoom::Normal);
        publish(&mut publisher, &waveform);
        assert_eq!(
            board.latest(),
            ScopeLabelsState::Published(ScopeLabels {
                request: waveform,
                labels: Vec::new()
            })
        );
    }

    #[test]
    fn closing_the_scopes_publishes_withdrawn_once() {
        let board = ScopeLabelsBoard::new();
        let mut publisher = ScopeLabelsPublisher::new(board.clone());
        publish(
            &mut publisher,
            &request(ScopeKind::Vectorscope, VectorscopeZoom::Normal),
        );

        publisher.withdraw();
        assert_eq!(board.latest(), ScopeLabelsState::Withdrawn);

        let marker = ScopeLabelsState::NotPublished;
        board.publish(marker.clone());
        publisher.withdraw();
        assert_eq!(board.latest(), marker, "второе закрытие подряд доску не пишет");
    }

    #[test]
    fn a_new_session_does_not_trust_labels_left_by_the_previous_one() {
        let board = ScopeLabelsBoard::new();
        let mut previous = ScopeLabelsPublisher::new(board.clone());
        publish(&mut previous, &request(ScopeKind::Vectorscope, VectorscopeZoom::Normal));
        assert!(matches!(board.latest(), ScopeLabelsState::Published(_)));

        let _next = ScopeLabelsPublisher::new(board.clone());
        assert_eq!(board.latest(), ScopeLabelsState::Withdrawn);
    }

    #[test]
    fn a_new_session_leaves_a_board_that_was_never_published_alone() {
        let board = ScopeLabelsBoard::new();
        let _publisher = ScopeLabelsPublisher::new(board.clone());
        assert_eq!(board.latest(), ScopeLabelsState::NotPublished);
    }
}
