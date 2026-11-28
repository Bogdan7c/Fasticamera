//! Состояние раздела калибровки: что выбрано на экране (не значения ручек!).
//!
//! Значения ручек, активный пресет и «что отличается от пресета» экран НЕ хранит: они приходят из `LookFacts`
//! (владелец — библиотека пресетов, решение 178). Здесь только выбор самого экрана: какой прибор показан, какая
//! полоса выбрана, держится ли «чистяк», открыто ли меню, ждём ли ответ клавиатуры, взведено ли удаление.

use look_contract::{HueBandId, UserPresetId, WheelZone};
use scope_contract::{
    CieDiagram, HistogramMode, ScopeKind, ScopeLabel, ScopeLabelsState, ScopeRequest, VectorscopeAxes, VectorscopeZoom,
};
use shell_contract::text_input::TextRequestId;

use super::config::CalibrationConfig;
use super::layout::{CalibrationLayout, CalibrationLayoutError};
use super::scope_labels_view::ScopeLabelsView;
use super::view_fault::ControlViewFault;

/// Что именем пресета делаем после ввода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Naming {
    /// Сохранить текущий вид как новый пресет.
    NewPreset,
    /// Переименовать свой пресет.
    Rename(UserPresetId),
}

/// Запрос текста, который уже получил номер у приложения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingText {
    /// Номер запроса: ответ вернётся с ним же.
    pub id: TextRequestId,
    /// Зачем спросили.
    pub naming: Naming,
}

/// Ход ввода имени. Номер запроса выдаёт брокер приложения, поэтому экран узнаёт его отдельным действием
/// (`TextRequestIssued`) — так же, как номер снимка узнаётся из исполнителя эффектов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextFlow {
    /// Ничего не спрашиваем.
    #[default]
    Idle,
    /// Эффект «спроси имя» выдан, номер ещё не пришёл.
    ///
    /// `previous_dialog` — диалог, который был открыт к моменту нового запроса. Пока новый запрос не получил номер,
    /// старый диалог ЖИВ: отказанная отправка (`TextRequestNotSent`) брокер не считает вытеснением, и ответ на
    /// старый диалог ещё придёт. Поэтому его номер держим, чтобы вернуть (найдено ревью 14b).
    AwaitingId {
        /// Зачем спросили новым запросом.
        naming: Naming,
        /// Ранее открытый диалог, ответ на который ещё может прийти.
        previous_dialog: Option<PendingText>,
    },
    /// Диалог открыт, ждём ответ с этим номером.
    Pending(PendingText),
}

/// Двойное подтверждение удаления пресета (решение владельца «удалить» + придуманная защита от случайного тапа).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeleteConfirmation {
    /// Не взведено.
    #[default]
    Idle,
    /// Первый тап был: второй тап по этому же пресету удалит его, любое другое действие снимает взвод.
    Armed(UserPresetId),
}

/// Держится ли «чистяк» (решение 189): основа без вида, пока палец на кнопке.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanViewState {
    /// Показан вид с правками.
    #[default]
    ShowingEdited,
    /// Кнопка держится: на превью и приборе основа без вида.
    HoldingBase,
}

/// Какая нижняя карточка раздела калибровки открыта (решения 283, 292). Единственный владелец ответа «что открыто
/// снизу»: карточек две, и вместе они не открываются (модальная завеса не даёт нажать вторую, а редьюсер отвергает
/// такое действие сам).
///
/// Пока открыта ЛЮБАЯ карточка, нижний ряд (CLEAN, капсула режимов, кнопка меню) скрыт и не ловит касаний
/// (решение 292); тап по завесе закрывает карточку; смена режима закрывает её; поворот телефона её не закрывает.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionCard {
    /// Нижней карточки нет.
    #[default]
    Closed,
    /// Карточка пресетов: ряд имён и действия New / Rename / Save / Delete / Reset (кнопка меню, решение 283).
    Presets,
    /// Карточка белого: оси, шкала, «Phone», свет — без «Auto» (капсула белого в полоске прибора, решения 290–294).
    White,
}

/// Какой вид органов управления показывает колонка ручек (решения 204, 343): строки-шкалы, цветовые колёса или
/// полоски по каналам R, G, B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ControlViewKind {
    /// Группы строк-линеек TONE / COLOR / BANDS (прокручиваются).
    #[default]
    Scales,
    /// Одно большое колесо цвета с вкладками LIFT · GAMMA · GAIN · OFFSET (без прокрутки).
    Wheels,
    /// Те же вкладки зон, что у колёс, и под ними три шкалы R, G, B выбранной зоны (без прокрутки, решение 343).
    Bars,
}

impl ControlViewKind {
    /// Виды в порядке показа в переключателе SCALES | WHEELS | BARS.
    pub const ALL: [ControlViewKind; 3] = [ControlViewKind::Scales, ControlViewKind::Wheels, ControlViewKind::Bars];
}

/// Состояние раздела калибровки. Меняется только редьюсером; снаружи — запросы по смыслу.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationState {
    scope_kind: ScopeKind,
    vectorscope_axes: VectorscopeAxes,
    /// Увеличение вектороскопа (кнопка «2×», решение 325); живёт, пока живёт раздел, как выбор осей.
    vectorscope_zoom: VectorscopeZoom,
    /// Режим гистограммы (переключатель RGB | PARADE | Y, решение 331); хранится при любой странице, свайп его не теряет.
    histogram_mode: HistogramMode,
    /// Шкала CIE (переключатель xy | u'v', решение 332); хранится при любой странице, как и режим гистограммы.
    cie_diagram: CieDiagram,
    selected_band: Option<HueBandId>,
    /// Вид колонки ручек. Живёт как `selected_band`: переживает поворот и пересборку раздела (решение 204).
    control_view: ControlViewKind,
    /// Выбранная зона (вкладка LIFT · GAMMA · GAIN · OFFSET): ОБЩАЯ у видов WHEELS и BARS (решение 343), поэтому
    /// имя осталось «колесо» — переименование было бы косметикой. Переживает пересборку раздела (решение 205).
    selected_wheel: WheelZone,
    /// Смещение прокрутки списка шкал, pt: запоминается, когда список успокоился, чтобы вид SCALES после
    /// переключения на WHEELS и обратно (и после пересборки раздела) вернулся на то же место, а не на верх.
    scales_scroll_offset_pt: f64,
    clean_view: CleanViewState,
    card: SectionCard,
    text: TextFlow,
    delete: DeleteConfirmation,
    /// Раскладка под текущую поверхность и положение; `None` — режим без раздела или поверхности ещё нет.
    layout: Option<CalibrationLayout>,
    /// Ошибка последней неудачной сборки раскладки; `None` — сборка удалась или не требовалась. Нужна, чтобы отказ
    /// `InvalidSurface` считался один раз на смену ошибки, а не на каждый тик и каждое действие (ревью 14b).
    layout_fault: Option<CalibrationLayoutError>,
    /// Почему вид колёс или полосок не поместился в колонку; `None` — поместился или вид шкал. Как
    /// `layout_fault`, нужен, чтобы отказ считался один раз на смену причины, а не на каждом кадре.
    view_fault: Option<ControlViewFault>,
    /// Запрос прибора, который уже ушёл рендеру и действует там (чтобы не слать то же самое дважды и
    /// знать, когда слать `Hide`).
    announced_scope: Option<ScopeRequest>,
    /// Подписи прибора от рендера (решения 326, 327): хранятся как пришли, рисуются только для текущего запроса.
    scope_labels: ScopeLabelsView,
}

impl CalibrationState {
    /// Начальное состояние: страница прибора, оси, режим гистограммы и шкала CIE — из конфига (решения 331, 332,
    /// 335), полоса не выбрана (до первых фактов).
    pub(crate) fn new(config: &CalibrationConfig) -> Self {
        Self {
            scope_kind: config.opening_scope_page,
            vectorscope_axes: config.vectorscope_axes,
            vectorscope_zoom: VectorscopeZoom::Normal,
            histogram_mode: config.histogram_mode,
            cie_diagram: config.cie_diagram,
            selected_band: None,
            control_view: ControlViewKind::default(),
            selected_wheel: WheelZone::Lift,
            scales_scroll_offset_pt: 0.0,
            clean_view: CleanViewState::default(),
            card: SectionCard::default(),
            text: TextFlow::default(),
            delete: DeleteConfirmation::default(),
            layout: None,
            layout_fault: None,
            view_fault: None,
            announced_scope: None,
            scope_labels: ScopeLabelsView::default(),
        }
    }

    /// Выбранный прибор.
    pub fn scope_kind(&self) -> ScopeKind {
        self.scope_kind
    }

    /// Выбранный стандарт осей вектороскопа.
    pub fn vectorscope_axes(&self) -> VectorscopeAxes {
        self.vectorscope_axes
    }

    /// Выбранное увеличение вектороскопа.
    pub fn vectorscope_zoom(&self) -> VectorscopeZoom {
        self.vectorscope_zoom
    }

    /// Выбранный режим гистограммы.
    pub fn histogram_mode(&self) -> HistogramMode {
        self.histogram_mode
    }

    /// Выбранная шкала диаграммы CIE.
    pub fn cie_diagram(&self) -> CieDiagram {
        self.cie_diagram
    }

    /// Выбранная полоса оттенка (под рядом точек показаны её шкалы HUE / SAT / LUM).
    pub fn selected_band(&self) -> Option<&HueBandId> {
        self.selected_band.as_ref()
    }

    /// Вид колонки ручек: шкалы или колёса.
    pub fn control_view(&self) -> ControlViewKind {
        self.control_view
    }

    /// Выбранная вкладка зоны LIFT / GAMMA / GAIN / OFFSET: общая у видов WHEELS и BARS (решение 343).
    pub fn selected_wheel(&self) -> WheelZone {
        self.selected_wheel
    }

    /// Где остановилась прокрутка списка шкал, pt (0 — верх). Виджет списка при сборке начинает отсюда.
    pub fn scales_scroll_offset_pt(&self) -> f64 {
        self.scales_scroll_offset_pt
    }

    /// Держится ли «чистяк».
    pub fn clean_view(&self) -> CleanViewState {
        self.clean_view
    }

    /// Какая нижняя карточка раздела открыта.
    pub fn card(&self) -> SectionCard {
        self.card
    }

    /// Ход ввода имени пресета.
    pub fn text_flow(&self) -> TextFlow {
        self.text
    }

    /// Взведено ли удаление и какого пресета.
    pub fn delete_confirmation(&self) -> DeleteConfirmation {
        self.delete
    }

    /// Раскладка раздела; `None` — сейчас не режим калибровки или нет размера поверхности.
    pub fn layout(&self) -> Option<&CalibrationLayout> {
        self.layout.as_ref()
    }

    /// Запрос прибора по текущему выбору и раскладке; `None` — раскладки нет, просить нечего.
    pub(crate) fn desired_scope_request(&self) -> Option<ScopeRequest> {
        let layout = self.layout.as_ref()?;
        Some(ScopeRequest {
            kind: self.scope_kind,
            vectorscope_axes: self.vectorscope_axes,
            vectorscope_zoom: self.vectorscope_zoom,
            cie_diagram: self.cie_diagram,
            histogram_mode: self.histogram_mode,
            scan: layout.scope_scan(),
            image_size: layout.scope_image_size_for(self.scope_kind),
        })
    }

    /// Принимает подписи прибора от рендера; `false` — те же, что были. Остальное состояние раздела не трогает.
    pub(crate) fn store_scope_labels(&mut self, state: ScopeLabelsState) -> bool {
        self.scope_labels.store(state)
    }

    /// Подписи, которые можно рисовать СЕЙЧАС: посчитанные для текущего желаемого запроса прибора. Нет раскладки
    /// (запрашивать нечего) или подписи чужого запроса — пусто.
    pub(crate) fn scope_labels_to_draw(&self) -> &[ScopeLabel] {
        match self.desired_scope_request() {
            Some(desired) => self.scope_labels.labels_for(&desired),
            None => &[],
        }
    }

    /// Запрос прибора, о котором уже знает рендер.
    pub(crate) fn announced_scope(&self) -> Option<ScopeRequest> {
        self.announced_scope
    }

    pub(crate) fn set_announced_scope(&mut self, request: Option<ScopeRequest>) {
        self.announced_scope = request;
    }

    pub(crate) fn set_layout(&mut self, layout: Option<CalibrationLayout>) {
        self.layout = layout;
    }

    /// Запоминает ошибку сборки раскладки (`None` — ошибки нет). Возвращает `true`, если это НОВАЯ ошибка, то есть
    /// отказ по ней ещё не считали.
    pub(crate) fn note_layout_fault(&mut self, fault: Option<CalibrationLayoutError>) -> bool {
        let is_new = fault.is_some() && self.layout_fault != fault;
        self.layout_fault = fault;
        is_new
    }

    /// Запоминает, что вид колёс или полосок не поместился (`None` — поместился). `true` — это НОВАЯ причина.
    pub(crate) fn note_view_fault(&mut self, fault: Option<ControlViewFault>) -> bool {
        let is_new = fault.is_some() && self.view_fault != fault;
        self.view_fault = fault;
        is_new
    }

    pub(crate) fn set_scope_kind(&mut self, kind: ScopeKind) {
        self.scope_kind = kind;
    }

    pub(crate) fn set_vectorscope_axes(&mut self, axes: VectorscopeAxes) {
        self.vectorscope_axes = axes;
    }

    pub(crate) fn set_vectorscope_zoom(&mut self, zoom: VectorscopeZoom) {
        self.vectorscope_zoom = zoom;
    }

    pub(crate) fn set_histogram_mode(&mut self, mode: HistogramMode) {
        self.histogram_mode = mode;
    }

    pub(crate) fn set_cie_diagram(&mut self, diagram: CieDiagram) {
        self.cie_diagram = diagram;
    }

    pub(crate) fn set_selected_band(&mut self, band: Option<HueBandId>) {
        self.selected_band = band;
    }

    pub(crate) fn set_control_view(&mut self, view: ControlViewKind) {
        self.control_view = view;
    }

    pub(crate) fn set_selected_wheel(&mut self, zone: WheelZone) {
        self.selected_wheel = zone;
    }

    pub(crate) fn set_scales_scroll_offset_pt(&mut self, offset_pt: f64) {
        self.scales_scroll_offset_pt = offset_pt;
    }

    pub(crate) fn set_clean_view(&mut self, clean_view: CleanViewState) {
        self.clean_view = clean_view;
    }

    pub(crate) fn set_card(&mut self, card: SectionCard) {
        self.card = card;
    }

    pub(crate) fn set_text_flow(&mut self, text: TextFlow) {
        self.text = text;
    }

    pub(crate) fn set_delete_confirmation(&mut self, delete: DeleteConfirmation) {
        self.delete = delete;
    }
}
