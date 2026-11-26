//! Состояние раздела калибровки: что выбрано на экране (не значения ручек!).
//!
//! Значения ручек, активный пресет и «что отличается от пресета» экран НЕ хранит: они приходят из `LookFacts`
//! (владелец — библиотека пресетов, решение 178). Здесь только выбор самого экрана: какой прибор показан, какая
//! полоса выбрана, держится ли «чистяк», открыто ли меню, ждём ли ответ клавиатуры, взведено ли удаление.

use look_contract::{HueBandId, UserPresetId, WheelZone};
use scope_contract::{ScopeKind, ScopeRequest, VectorscopeAxes};
use shell_contract::text_input::TextRequestId;

use super::config::CalibrationConfig;
use super::layout::{CalibrationLayout, CalibrationLayoutError};
use super::wheels_frames::WheelsFramesError;

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

/// Какой вид органов управления показывает колонка ручек (решение 204): строки-шкалы или цветовые колёса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ControlViewKind {
    /// Группы строк-линеек TONE / COLOR / BANDS (прокручиваются).
    #[default]
    Scales,
    /// Одно большое колесо цвета с вкладками LIFT · GAMMA · GAIN · OFFSET (без прокрутки).
    Wheels,
}

impl ControlViewKind {
    /// Виды в порядке показа в переключателе SCALES | WHEELS.
    pub const ALL: [ControlViewKind; 2] = [ControlViewKind::Scales, ControlViewKind::Wheels];
}

/// Состояние раздела калибровки. Меняется только редьюсером; снаружи — запросы по смыслу.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationState {
    scope_kind: ScopeKind,
    vectorscope_axes: VectorscopeAxes,
    selected_band: Option<HueBandId>,
    /// Вид колонки ручек. Живёт как `selected_band`: переживает поворот и пересборку раздела (решение 204).
    control_view: ControlViewKind,
    /// Выбранное колесо цвета в виде колёс (решение 205); тоже переживает пересборку.
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
    /// Почему вид колёс не поместился в колонку (круг меньше допустимого); `None` — поместился или вид шкал. Как
    /// `layout_fault`, нужен, чтобы отказ считался один раз на смену причины, а не на каждом кадре.
    wheels_fault: Option<WheelsFramesError>,
    /// Запрос прибора, который уже ушёл рендеру и действует там (чтобы не слать то же самое дважды и
    /// знать, когда слать `Hide`).
    announced_scope: Option<ScopeRequest>,
}

impl CalibrationState {
    /// Начальное состояние: прибор Waveform, оси из конфига, полоса не выбрана (до первых фактов).
    pub(crate) fn new(config: &CalibrationConfig) -> Self {
        Self {
            scope_kind: ScopeKind::Waveform,
            vectorscope_axes: config.vectorscope_axes,
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
            wheels_fault: None,
            announced_scope: None,
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

    /// Выбранная полоса оттенка (под рядом точек показаны её шкалы HUE / SAT / LUM).
    pub fn selected_band(&self) -> Option<&HueBandId> {
        self.selected_band.as_ref()
    }

    /// Вид колонки ручек: шкалы или колёса.
    pub fn control_view(&self) -> ControlViewKind {
        self.control_view
    }

    /// Выбранное колесо цвета (вкладка LIFT / GAMMA / GAIN / OFFSET).
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
            scan: layout.scope_scan(),
            image_size: layout.scope_image_size_for(self.scope_kind),
        })
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

    /// Запоминает, что вид колёс не поместился (`None` — поместился). `true` — это НОВАЯ причина.
    pub(crate) fn note_wheels_fault(&mut self, fault: Option<WheelsFramesError>) -> bool {
        let is_new = fault.is_some() && self.wheels_fault != fault;
        self.wheels_fault = fault;
        is_new
    }

    pub(crate) fn set_scope_kind(&mut self, kind: ScopeKind) {
        self.scope_kind = kind;
    }

    pub(crate) fn set_vectorscope_axes(&mut self, axes: VectorscopeAxes) {
        self.vectorscope_axes = axes;
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
