//! Числа ВИДА раздела калибровки: размеры колонки ручек, строк, кнопок, точек полос и прибора.
//!
//! Все числа здесь — `[invented]`/`[estimate]`: в эталоне Apple нет раздела калибровки (правило 96), поэтому
//! это подгонка под телефон, а не измерение. Единственное заимствованное из измеренного — вид кнопки «↻»
//! (диск и затемнение до 44 %, `[measured: C t=54.0]`): он берётся из `widgets.card`, а не дублируется здесь.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Сколько таблеток ряда можно завести: номер таблетки идёт в `ButtonId(u8)`, поэтому больше 255 не бывает.
pub const MAX_CHIP_POOL: usize = 255;

/// Наибольшее число единиц шкалы ручки на 1,0: больше — число положений линейки выходит за предел модели ui-kit.
pub const MAX_KNOB_UNITS: u32 = 1000;

/// Самый мелкий допустимый путь пальца на единицу шкалы ручки, pt: меньше — соседние единицы не различить пальцем
/// (`[estimate]`, решение 251; ниже половины точки шаг теряется в дрожи пальца).
pub const MIN_KNOB_POINTS_PER_UNIT: f64 = 0.5;

/// Числа вида раздела калибровки (pt, если не сказано иное).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationWidgetsConfig {
    /// Радиус скругления стеклянных панелей (колонка ручек, прибор). `[invented]`.
    pub panel_corner_radius_pt: f32,
    /// Отступ содержимого колонки от краёв панели. `[invented]`.
    pub panel_padding_pt: f64,
    /// Высота шапки колонки: переключатель основы и его «↻». `[invented]`.
    pub header_height_pt: f64,
    /// Высота капсулы CLEAN внизу (зона касания расширяется до 44 pt отдельно). `[invented]`.
    pub clean_button_height_pt: f64,
    /// Ширина капсулы CLEAN (значка «глаз» в наборе нет, решение 276). `[invented]`.
    pub clean_button_width_pt: f64,
    /// Прозрачность заливки кнопки «чистяк» в покое. `[invented]`.
    pub clean_button_fill_alpha: f32,
    /// Прозрачность заливки кнопки «чистяк», пока её держат. `[invented]`.
    pub clean_button_held_fill_alpha: f32,
    /// Промежуток между переключателем основы и «↻» шапки (карусель кончается перед зоной касания «↻», если она шире). `[invented]`.
    pub header_gap_pt: f64,
    /// Промежуток между значением и «↻» в строке ручки. `[invented]`.
    pub value_gap_pt: f64,
    /// Высота заголовка группы (TONE / COLOR / BANDS). `[invented]`.
    pub group_header_height_pt: f64,
    /// Высота строки ручки целиком: заголовок со значением и «↻», под ними линейка. `[invented]`.
    pub row_height_pt: f64,
    /// Высота верхней линии строки (заголовок, значение, «↻»). `[invented]`.
    pub row_title_height_pt: f64,
    /// Высота строки переключателя вида SCALES | WHEELS под шапкой (она же зона касания сегментов). `[invented]`.
    pub view_switcher_height_pt: f64,
    /// Высота капсулы сегмента переключателя вида и вкладки колеса (строка выше на зазор до зоны касания). `[invented]`.
    pub segment_height_pt: f64,
    /// Промежуток между сегментами переключателя вида и между вкладками колёс. `[invented]`.
    pub segment_gap_pt: f64,
    /// Высота строки вкладок LIFT · GAMMA · GAIN · OFFSET. `[invented]`.
    pub wheel_tabs_height_pt: f64,
    /// Высота строки «название колеса и ↻ колеса целиком». `[invented]`.
    pub wheel_title_height_pt: f64,
    /// Высота линейки кольца яркости (под строкой с названием и значением, как у остальных ручек). `[invented]`.
    pub wheel_ring_ruler_height_pt: f64,
    /// Вертикальный промежуток между блоками вида колёс (вкладки, название, круг, шкала кольца). `[invented]`.
    pub wheel_block_gap_pt: f64,
    /// Наибольший диаметр круга колеса (решение 205: ≈ 170 pt портрет); меньше — по месту. `[estimate]`.
    pub wheel_circle_max_diameter_pt: f64,
    /// Наименьший диаметр круга: меньше круг не строится (раздел отказывает сборкой, а не рисует крошку). `[estimate]`.
    pub wheel_circle_min_diameter_pt: f64,
    /// Толщина цветного кольца оттенков вокруг круга толчка. `[estimate]`.
    pub wheel_ring_thickness_pt: f64,
    /// Прозрачность тёмной подложки круга толчка (чёрная заливка). `[estimate]`.
    pub wheel_pad_fill_alpha: f32,
    /// Диаметр метки-кольца «колесо правлено» на вкладке. `[invented]`.
    pub wheel_tab_marker_diameter_pt: f64,
    /// Толщина метки-кольца вкладки. `[invented]`.
    pub wheel_tab_marker_width_pt: f64,
    /// Диаметр кнопки «↻» в строке. `[invented]`.
    pub reset_button_diameter_pt: f64,
    /// Сторона зоны касания «↻» колеса: HIG ≥ 44 pt. Зона растёт ВНУТРЬ колонки, а не за её край. `[estimate]`.
    pub reset_touch_side_pt: f64,
    /// Сколько зона касания «↻» строки забирает над строкой (решение 279): зона — полоса заголовка плюс эта полоса.
    /// Ровно столько же линейка строки не ловит у своего низа, поэтому «↻» нижней строки и линейка верхней не
    /// делят ни одной точки. `[invented]` 8 pt: 28 + 8 = 36 pt высоты зоны «↻» при ширине 44 pt.
    pub row_reset_reach_above_pt: f64,
    /// Высота ряда цветных точек полос. `[invented]`.
    pub band_dots_height_pt: f64,
    /// Диаметр цветной точки полосы. `[invented]`.
    pub band_dot_diameter_pt: f64,
    /// Промежуток между точками полос. `[invented]`.
    pub band_dot_gap_pt: f64,
    /// Толщина кольца вокруг выбранной точки. `[invented]`.
    pub band_ring_width_pt: f64,
    /// Зазор между точкой и её кольцом. `[invented]`.
    pub band_ring_gap_pt: f64,
    /// Диаметр метки «полоса отличается от пресета» под точкой. `[invented]`.
    pub band_marker_diameter_pt: f64,
    /// Толщина жёлтого кольца «у полосы есть правки» (её значения не нейтральны), решение 200. `[estimate]`.
    pub band_edited_ring_width_pt: f64,
    /// Зазор между точкой и кольцом «есть правки» (кольцо выбора лежит снаружи него). `[estimate]`.
    pub band_edited_ring_gap_pt: f64,
    /// Сколько целых единиц шкалы на значение 1,0 ручки (решения 234, 251): на экране числа −N…+N, в словаре и
    /// файле — прежние −1…+1, шаг линейки 1/N. `[estimate]` 200: −200…+200 (слово владельца 14l: «делений
    /// больше, а не размер»; решение 250, «4 pt на единицу», отменено).
    pub knob_units_per_full_scale: u32,
    /// Полный ход пальца по линейке ручки, от −N до +N, pt. Путь на единицу = ход / (2·N), см.
    /// [`CalibrationWidgetsConfig::knob_points_per_unit`]. `[estimate]` 400: как был до решения 250 (1 pt на единицу
    /// при 200 единицах).
    pub knob_full_travel_pt: f64,
    /// Через сколько шагов значения стоит риска (и щелчок отдачи) на линейке ручки. `[estimate]` 20: риска через
    /// 20 единиц, то есть те же 20 pt пальца, что и раньше.
    pub knob_tick_every_steps: u32,
    /// Каждая какая единица шагов значения крупная риска (в шагах значения, а не в рисках). `[estimate]` 100.
    pub knob_major_every_steps: u32,
    /// Магнит нуля на линейке ручки (решение 284): половина зоны, pt пути пальца. Пока палец ведёт ручку, значение
    /// дойдя до нуля стоит нулём, пока палец проходит `2 ×` это число, и снова идёт за пальцем без скачка
    /// (`ui_kit::scale::ZeroMagnet`). `0` — магнит выключен. `[estimate]` 6 pt: подбор на телефоне.
    pub knob_zero_magnet_pt: f64,
    /// «Картинка под пальцем» (решение 273): прозрачность остального, пока палец ведёт ручку или колесо
    /// (0 — остальное исчезает совсем). Остальное: панель колонки, шапка, переключатель, прочие строки. `[estimate]`.
    pub finger_focus_others_opacity: f32,
    /// Поля подложки под ведомым органом (вокруг его рамки), pt. `[estimate]`.
    pub finger_focus_backing_padding_pt: f64,
    /// Радиус скругления подложки под ведомым органом. `[estimate]`.
    pub finger_focus_backing_corner_radius_pt: f32,
    /// Порог распознавания пана на линейке ручки, pt (у остальных панов 10). Меньше порог — меньше «пустого» хода
    /// перед началом движения; путь до распознавания не теряется (значение считается от точки касания).
    /// `[estimate]` 5: достаточно, чтобы отличить горизонтальную протяжку от вертикальной прокрутки списка.
    pub knob_pan_hysteresis_pt: f64,
    /// Диаметр метки значения пресета на линейке (точка под рисками). `[estimate]`.
    pub preset_marker_diameter_pt: f64,
    /// Зазор между рисками и меткой значения пресета. `[estimate]`.
    pub preset_marker_gap_pt: f64,
    /// Прозрачность метки значения пресета (белая точка). `[estimate]`.
    pub preset_marker_alpha: f32,
    /// На сколько шагов линейка может разойтись с фактами, не двигаясь к ним. `[estimate]`.
    pub follow_tolerance_steps: f64,
    /// Прозрачность недоступной строки: фактов о виде нет или у строки полосы не выбрана полоса. `[invented]`.
    pub disabled_row_opacity: f32,
    /// Диаметр точки-страницы под прибором. `[invented]`.
    pub scope_page_dot_diameter_pt: f64,
    /// Промежуток между точками-страницами. `[invented]`.
    pub scope_page_dot_gap_pt: f64,
    /// Отступ содержимого полоски прибора (имя, оси, белое) от её боковых краёв. `[invented]`.
    pub scope_label_inset_pt: f64,
    /// Ширина ячейки точки-страницы в полоске: зона касания точки — вся ячейка высотой с полоску (44 pt), соседние
    /// ячейки встык и не перекрываются. `[estimate]`.
    pub scope_page_dot_cell_width_pt: f64,
    /// Ширина одного сегмента переключателя осей вектороскопа в полоске (Y'CbCr | OKLab). `[estimate]`.
    pub scope_axes_segment_width_pt: f64,
    /// Промежуток между частями полоски: имя, точки, оси, место белого. `[estimate]`.
    pub scope_strip_gap_pt: f64,
    /// Ширина места под капсулу белого («5600 K», решение 290) справа в полоске на каждой странице. Сама кнопка
    /// появится отдельным пакетом; место зарезервировано здесь, чтобы оси встали левее неё. `[estimate]`.
    pub scope_strip_trailing_width_pt: f64,
    /// Прозрачность волосяной линии между полоской и картинкой прибора (белая). `[estimate]`.
    pub scope_strip_hairline_alpha: f32,
    /// Отступ чисел уровней от правого края поля уровней (к картинке). `[estimate]`.
    pub scope_level_label_inset_pt: f64,
    /// Порог свайпа по прибору, pt: ОДИН для распознавания и для листания (решение 280). Короче — не свайп; признанный
    /// свайп листает страницу сразу. Между «признан» и «листает» ничего нет. `[estimate]` 20, подбор на телефоне.
    pub scope_swipe_min_pt: f64,
    /// Запас пилюли режима по бокам подписи в разделе. `[estimate]`.
    pub mode_pill_padding_pt: f64,
    /// Высота «таблетки» в ряду имён пресетов и в ряду действий (NEW / RENAME / SAVE / DELETE); общая для карточки
    /// Styles и карточек меню раздела. `[estimate]` (зона касания расширяется до 44 pt отдельно).
    pub chip_height_pt: f64,
    /// Поля таблетки слева и справа от подписи. `[estimate]`.
    pub chip_padding_pt: f64,
    /// Промежуток между соседними таблетками ряда. `[estimate]`.
    pub chip_gap_pt: f64,
    /// Прозрачность заливки таблетки (белая поверх стекла карточки). `[estimate]`.
    pub chip_fill_alpha: f32,
    /// Сколько таблеток ряд умеет держать (узлы сцены заводятся заранее). Больше имён — лишние не показываются, их
    /// число идёт в счётчик переполнения диагностики, а не теряется молча. `[estimate]`.
    pub chip_pool_size: usize,
    /// На сколько pt ряд действий пресета (NEW / RENAME / SAVE / DELETE) опущен против места крупного значения в
    /// карточке PHOTO: иначе зона касания первой таблетки (44 pt) заходит под кнопку «×» карточки. `[estimate]`.
    pub menu_card_actions_drop_pt: f64,
    /// На сколько pt ряд имён пресетов опущен против места ряда вариантов в карточке PHOTO: иначе зоны касания имён и
    /// действий перекрываются на несколько pt. `[estimate]`.
    pub menu_card_names_drop_pt: f64,
    /// Сколько секунд висит плашка проблемы, пока её не закроют. `[estimate]`.
    pub problem_banner_seconds: f64,
    /// Зазор между плашкой проблемы и тем, к чему она прижата (плашка AE/AF LOCK, нижний ряд раздела). `[estimate]`.
    pub problem_banner_gap_pt: f64,
}

impl Default for CalibrationWidgetsConfig {
    fn default() -> Self {
        Self {
            panel_corner_radius_pt: 26.0,
            panel_padding_pt: 12.0,
            header_height_pt: 56.0,
            clean_button_height_pt: 32.0,
            clean_button_width_pt: 64.0,
            clean_button_fill_alpha: 0.16,
            clean_button_held_fill_alpha: 0.4,
            header_gap_pt: 8.0,
            value_gap_pt: 8.0,
            group_header_height_pt: 30.0,
            row_height_pt: 68.0,
            row_title_height_pt: 28.0,
            view_switcher_height_pt: 36.0,
            segment_height_pt: 28.0,
            segment_gap_pt: 4.0,
            wheel_tabs_height_pt: 36.0,
            wheel_title_height_pt: 26.0,
            wheel_ring_ruler_height_pt: 32.0,
            wheel_block_gap_pt: 4.0,
            wheel_circle_max_diameter_pt: 170.0,
            wheel_circle_min_diameter_pt: 48.0,
            wheel_ring_thickness_pt: 10.0,
            wheel_pad_fill_alpha: 0.35,
            wheel_tab_marker_diameter_pt: 7.0,
            wheel_tab_marker_width_pt: 1.5,
            reset_button_diameter_pt: 26.0,
            reset_touch_side_pt: 44.0,
            row_reset_reach_above_pt: 8.0,
            band_dots_height_pt: 44.0,
            band_dot_diameter_pt: 24.0,
            band_dot_gap_pt: 10.0,
            band_ring_width_pt: 2.0,
            band_ring_gap_pt: 3.0,
            band_marker_diameter_pt: 5.0,
            band_edited_ring_width_pt: 1.5,
            band_edited_ring_gap_pt: 1.5,
            knob_units_per_full_scale: 200,
            knob_full_travel_pt: 400.0,
            knob_tick_every_steps: 20,
            knob_major_every_steps: 100,
            knob_zero_magnet_pt: 6.0,
            finger_focus_others_opacity: 0.0,
            finger_focus_backing_padding_pt: 6.0,
            finger_focus_backing_corner_radius_pt: 16.0,
            knob_pan_hysteresis_pt: 5.0,
            preset_marker_diameter_pt: 4.0,
            preset_marker_gap_pt: 2.0,
            preset_marker_alpha: 0.9,
            follow_tolerance_steps: 0.01,
            disabled_row_opacity: 0.35,
            scope_page_dot_diameter_pt: 6.0,
            scope_page_dot_gap_pt: 8.0,
            scope_label_inset_pt: 8.0,
            scope_page_dot_cell_width_pt: 22.0,
            scope_axes_segment_width_pt: 48.0,
            scope_strip_gap_pt: 6.0,
            scope_strip_trailing_width_pt: 68.0,
            scope_strip_hairline_alpha: 0.18,
            scope_level_label_inset_pt: 4.0,
            scope_swipe_min_pt: 20.0,
            mode_pill_padding_pt: 20.0,
            chip_height_pt: 32.0,
            chip_padding_pt: 16.0,
            chip_gap_pt: 8.0,
            chip_fill_alpha: 0.16,
            chip_pool_size: 16,
            menu_card_actions_drop_pt: 11.0,
            menu_card_names_drop_pt: 15.0,
            problem_banner_seconds: 3.0,
            problem_banner_gap_pt: 8.0,
        }
    }
}

/// Ошибка чисел вида раздела.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CalibrationWidgetsConfigError {
    #[error("размер {field} должен быть конечным и больше нуля: {value}")]
    NonPositive { field: &'static str, value: f64 },
    #[error("размер {field} должен быть конечным и не меньше нуля: {value}")]
    Negative { field: &'static str, value: f64 },
    #[error("доля {field} должна быть конечной и в 0..=1: {value}")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("единиц шкалы ручки на 1,0 должно быть от 1 до {MAX_KNOB_UNITS}: {value}")]
    KnobUnitsOutOfRange { value: u32 },
    #[error(
        "шаг ручки мельче {MIN_KNOB_POINTS_PER_UNIT} pt: ход {full_travel_pt} pt на {units} единиц даёт \
         {points_per_unit} pt на единицу"
    )]
    KnobScaleTooFine {
        full_travel_pt: f64,
        units: u32,
        points_per_unit: f64,
    },
    #[error("период крупной риски должен быть не меньше 1")]
    ZeroMajorPeriod,
    #[error("период риски ручки должен быть не меньше 1")]
    ZeroTickPeriod,
    #[error("в ряду таблеток от 1 до {MAX_CHIP_POOL}: {value}")]
    ChipPoolOutOfRange { value: usize },
    #[error(
        "круг колеса: наименьший диаметр {min_pt} больше наибольшего {max_pt}, либо кольцо {thickness_pt} не уже радиуса"
    )]
    WheelCircleRange {
        min_pt: f64,
        max_pt: f64,
        thickness_pt: f64,
    },
    #[error("строка ({row_height_pt} pt) не вмещает верхнюю линию ({title_height_pt} pt) и линейку")]
    RowTooShort { row_height_pt: f64, title_height_pt: f64 },
    #[error("зона «↻» над строкой ({reach_pt} pt) съедает всю линейку ({ruler_pt} pt)")]
    ResetReachEatsRuler { reach_pt: f64, ruler_pt: f64 },
}

impl CalibrationWidgetsConfig {
    /// Путь пальца на одну единицу шкалы ручки, pt: полный ход / (2·N) (от −N до +N это 2·N единиц). Осмысленно
    /// только у проверенного конфига ([`CalibrationWidgetsConfig::validate`]): там ход конечный и > 0, единиц ≥ 1.
    pub fn knob_points_per_unit(&self) -> f64 {
        self.knob_full_travel_pt / (2.0 * f64::from(self.knob_units_per_full_scale))
    }

    /// Проверка значений.
    pub fn validate(&self) -> Result<(), CalibrationWidgetsConfigError> {
        let sizes = [
            ("panel_corner_radius_pt", f64::from(self.panel_corner_radius_pt)),
            ("panel_padding_pt", self.panel_padding_pt),
            ("finger_focus_backing_padding_pt", self.finger_focus_backing_padding_pt),
            (
                "finger_focus_backing_corner_radius_pt",
                f64::from(self.finger_focus_backing_corner_radius_pt),
            ),
            ("header_height_pt", self.header_height_pt),
            ("clean_button_height_pt", self.clean_button_height_pt),
            ("clean_button_width_pt", self.clean_button_width_pt),
            ("header_gap_pt", self.header_gap_pt),
            ("value_gap_pt", self.value_gap_pt),
            ("group_header_height_pt", self.group_header_height_pt),
            ("row_height_pt", self.row_height_pt),
            ("row_title_height_pt", self.row_title_height_pt),
            ("reset_button_diameter_pt", self.reset_button_diameter_pt),
            ("reset_touch_side_pt", self.reset_touch_side_pt),
            ("row_reset_reach_above_pt", self.row_reset_reach_above_pt),
            ("view_switcher_height_pt", self.view_switcher_height_pt),
            ("segment_height_pt", self.segment_height_pt),
            ("segment_gap_pt", self.segment_gap_pt),
            ("wheel_tabs_height_pt", self.wheel_tabs_height_pt),
            ("wheel_title_height_pt", self.wheel_title_height_pt),
            ("wheel_ring_ruler_height_pt", self.wheel_ring_ruler_height_pt),
            ("wheel_block_gap_pt", self.wheel_block_gap_pt),
            ("wheel_circle_max_diameter_pt", self.wheel_circle_max_diameter_pt),
            ("wheel_circle_min_diameter_pt", self.wheel_circle_min_diameter_pt),
            ("wheel_ring_thickness_pt", self.wheel_ring_thickness_pt),
            ("wheel_tab_marker_diameter_pt", self.wheel_tab_marker_diameter_pt),
            ("wheel_tab_marker_width_pt", self.wheel_tab_marker_width_pt),
            ("band_dots_height_pt", self.band_dots_height_pt),
            ("band_dot_diameter_pt", self.band_dot_diameter_pt),
            ("band_dot_gap_pt", self.band_dot_gap_pt),
            ("band_ring_width_pt", self.band_ring_width_pt),
            ("band_ring_gap_pt", self.band_ring_gap_pt),
            ("band_marker_diameter_pt", self.band_marker_diameter_pt),
            ("band_edited_ring_width_pt", self.band_edited_ring_width_pt),
            ("band_edited_ring_gap_pt", self.band_edited_ring_gap_pt),
            ("follow_tolerance_steps", self.follow_tolerance_steps),
            ("knob_full_travel_pt", self.knob_full_travel_pt),
            ("knob_pan_hysteresis_pt", self.knob_pan_hysteresis_pt),
            ("preset_marker_diameter_pt", self.preset_marker_diameter_pt),
            ("preset_marker_gap_pt", self.preset_marker_gap_pt),
            ("scope_page_dot_diameter_pt", self.scope_page_dot_diameter_pt),
            ("scope_page_dot_gap_pt", self.scope_page_dot_gap_pt),
            ("scope_label_inset_pt", self.scope_label_inset_pt),
            ("scope_swipe_min_pt", self.scope_swipe_min_pt),
            ("scope_page_dot_cell_width_pt", self.scope_page_dot_cell_width_pt),
            ("scope_axes_segment_width_pt", self.scope_axes_segment_width_pt),
            ("scope_strip_gap_pt", self.scope_strip_gap_pt),
            ("scope_strip_trailing_width_pt", self.scope_strip_trailing_width_pt),
            ("scope_level_label_inset_pt", self.scope_level_label_inset_pt),
            ("mode_pill_padding_pt", self.mode_pill_padding_pt),
            ("chip_height_pt", self.chip_height_pt),
            ("chip_padding_pt", self.chip_padding_pt),
            ("chip_gap_pt", self.chip_gap_pt),
            ("menu_card_actions_drop_pt", self.menu_card_actions_drop_pt),
            ("menu_card_names_drop_pt", self.menu_card_names_drop_pt),
            ("problem_banner_seconds", self.problem_banner_seconds),
            ("problem_banner_gap_pt", self.problem_banner_gap_pt),
        ];
        for (field, value) in sizes {
            if !(value.is_finite() && value > 0.0) {
                return Err(CalibrationWidgetsConfigError::NonPositive { field, value });
            }
        }
        // Размеры, у которых 0 значит «выключено».
        let switchable_sizes = [("knob_zero_magnet_pt", self.knob_zero_magnet_pt)];
        for (field, value) in switchable_sizes {
            if !(value.is_finite() && value >= 0.0) {
                return Err(CalibrationWidgetsConfigError::Negative { field, value });
            }
        }
        let fractions = [
            ("disabled_row_opacity", f64::from(self.disabled_row_opacity)),
            (
                "finger_focus_others_opacity",
                f64::from(self.finger_focus_others_opacity),
            ),
            ("clean_button_fill_alpha", f64::from(self.clean_button_fill_alpha)),
            ("chip_fill_alpha", f64::from(self.chip_fill_alpha)),
            ("wheel_pad_fill_alpha", f64::from(self.wheel_pad_fill_alpha)),
            ("preset_marker_alpha", f64::from(self.preset_marker_alpha)),
            ("scope_strip_hairline_alpha", f64::from(self.scope_strip_hairline_alpha)),
            (
                "clean_button_held_fill_alpha",
                f64::from(self.clean_button_held_fill_alpha),
            ),
        ];
        for (field, value) in fractions {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return Err(CalibrationWidgetsConfigError::FractionOutOfRange { field, value });
            }
        }
        if !(1..=MAX_KNOB_UNITS).contains(&self.knob_units_per_full_scale) {
            return Err(CalibrationWidgetsConfigError::KnobUnitsOutOfRange {
                value: self.knob_units_per_full_scale,
            });
        }
        let points_per_unit = self.knob_points_per_unit();
        if points_per_unit < MIN_KNOB_POINTS_PER_UNIT {
            return Err(CalibrationWidgetsConfigError::KnobScaleTooFine {
                full_travel_pt: self.knob_full_travel_pt,
                units: self.knob_units_per_full_scale,
                points_per_unit,
            });
        }
        if self.knob_tick_every_steps == 0 {
            return Err(CalibrationWidgetsConfigError::ZeroTickPeriod);
        }
        if self.knob_major_every_steps == 0 {
            return Err(CalibrationWidgetsConfigError::ZeroMajorPeriod);
        }
        if self.chip_pool_size == 0 || self.chip_pool_size > MAX_CHIP_POOL {
            return Err(CalibrationWidgetsConfigError::ChipPoolOutOfRange {
                value: self.chip_pool_size,
            });
        }
        if self.row_height_pt <= self.row_title_height_pt {
            return Err(CalibrationWidgetsConfigError::RowTooShort {
                row_height_pt: self.row_height_pt,
                title_height_pt: self.row_title_height_pt,
            });
        }
        let ruler_pt = self.row_height_pt - self.row_title_height_pt;
        if self.row_reset_reach_above_pt >= ruler_pt {
            return Err(CalibrationWidgetsConfigError::ResetReachEatsRuler {
                reach_pt: self.row_reset_reach_above_pt,
                ruler_pt,
            });
        }
        if self.wheel_circle_min_diameter_pt > self.wheel_circle_max_diameter_pt
            || self.wheel_ring_thickness_pt >= self.wheel_circle_min_diameter_pt / 2.0
        {
            return Err(CalibrationWidgetsConfigError::WheelCircleRange {
                min_pt: self.wheel_circle_min_diameter_pt,
                max_pt: self.wheel_circle_max_diameter_pt,
                thickness_pt: self.wheel_ring_thickness_pt,
            });
        }
        Ok(())
    }
}
