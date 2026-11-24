//! Каталог работ стенда насыщения: имена для конфига и флага, и что за каждым именем стоит.
//!
//! Каталог — ДАННЫЕ (таблица [`CATALOG`]), а не `if name == …` по коду: добавить работу значит добавить строку
//! и ветку в сборщике работ. Модуль не знает GPU: имена проверяются при разборе конфига, до любого запуска.

use serde::Serialize;

use super::draw_filter::DrawFilter;

/// Какой прибор считает работа `scopes_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeShape {
    Waveform,
    Parade,
    /// Вектороскоп на осях Cb/Cr (BT.709).
    VectorscopeYcbcr,
    /// Вектороскоп на осях a/b OKLab.
    VectorscopeOklab,
}

/// Что на входе приборов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeInputKind {
    /// Настоящая сцена (синтетическая «натура»).
    Natural,
    /// Однородный серый: все отсчёты в одну ячейку, худший случай конкуренции `atomicAdd`.
    UniformGray,
}

/// Какой кусок накопления прибора оставлен в диагностической работе `scopes_*_<вариант>` (зеркало
/// `isp_vulkan::saturation::AccumulateDiagnostic`: каталог GPU не знает).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeAblation {
    /// Пустой шейдер: очистка счётчиков и запуск групп.
    Empty,
    /// Только чтение картинки и квантование.
    ReadOnly,
    /// Чтение + проекция без записи.
    SkipWrite,
    /// Все `atomicAdd` в одну ячейку.
    WriteCellZero,
    /// У каждой нити своя ячейка.
    WriteSpread,
    /// Слияние одинаковых ячеек в subgroup перед atomicAdd.
    Aggregated,
    /// Нить обходит 4 отсчёта по y, серии одинаковых ячеек сливаются в регистре.
    Runs4,
    /// То же, 8 отсчётов.
    Runs8,
    /// То же, 16 отсчётов.
    Runs16,
}

/// Экран камеры, список рисования которого гонит работа `display_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayScreen {
    /// Режим PHOTO.
    Photo,
    /// Раздел калибровки CALIBRATE (колонка ручек, панель прибора с живой картинкой).
    Calibrate,
}

/// Как меняется фон («кадр камеры») между кадрами работы показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackdropRhythm {
    /// Новый кадр камеры на каждом кадре: пирамида размытия строится каждый раз.
    NewFrameEveryFrame,
    /// Тот же кадр (пересборка экрана на тиках 120 Гц): пирамида переиспользуется.
    SameFrame,
}

/// Отключаемая стадия слитого прохода видоискателя (имена — как у `IspStage`, кроме статистики AWB: её режимами
/// управляет [`ViewfinderVariant`]). Каталог GPU не знает, поэтому своё перечисление, а не `IspStage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AblatedStage {
    Levels,
    LensShading,
    WhiteBalance,
    Demosaic,
    Color,
    GamutCompression,
    Tone,
    LookColor,
    LookLut,
    OutputGamutCompression,
    Oetf,
}

/// Какой вариант видоискателя гонит работа `viewfinder*`. Умолчание продукта — [`Self::Product`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "variant", rename_all = "snake_case")]
pub enum ViewfinderVariant {
    /// Продукт: все стадии, статистика AWB каждый 4-й кадр.
    Product,
    /// Статистика AWB не записывается никогда (остальные стадии продуктовые).
    AwbNever,
    /// Статистика AWB на каждом кадре.
    AwbEveryFrame,
    /// Одна стадия отключена, каденция AWB продуктовая.
    Without { stage: AblatedStage },
    /// Пол цены: остаются только `Levels` и `Demosaic` — проход читает всю ячейку RAW 2×2 (4 отсчёта на
    /// выходной пиксель), нормализует, сворачивает в RGB и пишет картинку; всё остальное выключено.
    Minimal,
}

/// Группа работ для флага `--saturation-workloads` и конфига: имя группы раскрывается в её работы по порядку
/// каталога.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadGroup {
    /// Первые 14 работ сессии 16a: умолчание списка, состав и порядок НЕ меняются.
    Baseline,
    /// Абляции прохода UI: что стоит каждый вид элемента экрана.
    UiAblation,
    /// Абляции видоискателя: что стоит каждая стадия и статистика AWB.
    IspAblation,
    /// Абляции накопления приборов: цена чтения, проекции и записи `atomicAdd`.
    ScopesAblation,
}

impl WorkloadGroup {
    /// Все группы.
    pub const ALL: [WorkloadGroup; 4] = [
        Self::Baseline,
        Self::UiAblation,
        Self::IspAblation,
        Self::ScopesAblation,
    ];

    /// Имя группы в конфиге и флаге.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::UiAblation => "ui_ablation",
            Self::IspAblation => "isp_ablation",
            Self::ScopesAblation => "scopes_ablation",
        }
    }

    /// Группа по имени (`None` — это не имя группы, возможно, имя отдельной работы).
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.name() == name)
    }
}

/// Что за работа стоит за именем каталога.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "work", rename_all = "snake_case")]
pub enum WorkloadKind {
    /// Слитый проход видоискателя на RAW10 продуктового размера.
    Viewfinder { variant: ViewfinderVariant },
    /// Цветовой прибор.
    Scopes { shape: ScopeShape, input: ScopeInputKind },
    /// Цветовой прибор с урезанным накоплением (диагностика цены).
    ScopesAblated { shape: ScopeShape, ablation: ScopeAblation },
    /// Только компоновщик (картинка видоискателя в цель экрана).
    Compositor,
    /// Компоновщик + рендер UI настоящего экрана камеры.
    Display {
        screen: DisplayScreen,
        rhythm: BackdropRhythm,
        /// Какие элементы списка рисования остаются (абляции; продукт — [`DrawFilter::All`]).
        filter: DrawFilter,
    },
    /// Стенд V4: цепочка запаса под видео на кадре 4096×2304 без шумодава.
    HeadroomV4,
    /// Проявление снимка целиком.
    Still,
}

/// Строка каталога.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    /// Имя для конфига и флага `--saturation-workloads`.
    pub name: &'static str,
    pub kind: WorkloadKind,
    /// В какую группу входит работа.
    pub group: WorkloadGroup,
    /// Что мерит работа (одна строка для справки и ошибок).
    pub summary: &'static str,
}

const fn scopes(shape: ScopeShape, input: ScopeInputKind) -> WorkloadKind {
    WorkloadKind::Scopes { shape, input }
}

const fn scopes_ablated(shape: ScopeShape, ablation: ScopeAblation) -> WorkloadKind {
    WorkloadKind::ScopesAblated { shape, ablation }
}

const fn display(screen: DisplayScreen, rhythm: BackdropRhythm) -> WorkloadKind {
    WorkloadKind::Display {
        screen,
        rhythm,
        filter: DrawFilter::All,
    }
}

/// Кадр показа с фоном «новый кадр каждый кадр» и отфильтрованным списком рисования (абляция).
const fn ablated_display(screen: DisplayScreen, filter: DrawFilter) -> WorkloadKind {
    WorkloadKind::Display {
        screen,
        rhythm: BackdropRhythm::NewFrameEveryFrame,
        filter,
    }
}

const fn viewfinder(variant: ViewfinderVariant) -> WorkloadKind {
    WorkloadKind::Viewfinder { variant }
}

const fn without(stage: AblatedStage) -> WorkloadKind {
    viewfinder(ViewfinderVariant::Without { stage })
}

/// Все работы стенда в порядке прогона по умолчанию.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "viewfinder",
        kind: viewfinder(ViewfinderVariant::Product),
        group: WorkloadGroup::Baseline,
        summary: "ISP видоискателя: RAW10 4096x3072, все стадии, AWB каждый 4-й кадр",
    },
    CatalogEntry {
        name: "scopes_waveform",
        kind: scopes(ScopeShape::Waveform, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "прибор waveform на сцене",
    },
    CatalogEntry {
        name: "scopes_parade",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "прибор RGB parade на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп Cb/Cr на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_oklab",
        kind: scopes(ScopeShape::VectorscopeOklab, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп a/b OKLab на сцене",
    },
    CatalogEntry {
        name: "scopes_parade_gray",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::UniformGray),
        group: WorkloadGroup::Baseline,
        summary: "parade на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "scopes_vectorscope_gray",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::UniformGray),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "compositor",
        kind: WorkloadKind::Compositor,
        group: WorkloadGroup::Baseline,
        summary: "компоновщик: картинка видоискателя в цель экрана",
    },
    CatalogEntry {
        name: "display_photo",
        kind: display(DisplayScreen::Photo, BackdropRhythm::NewFrameEveryFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа экрана PHOTO, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_photo_recompose",
        kind: display(DisplayScreen::Photo, BackdropRhythm::SameFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа экрана PHOTO, фон тот же кадр",
    },
    CatalogEntry {
        name: "display_calibrate",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::NewFrameEveryFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа раздела CALIBRATE с живой картинкой прибора, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_calibrate_recompose",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::SameFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа раздела CALIBRATE, фон тот же кадр",
    },
    CatalogEntry {
        name: "headroom_v4",
        kind: WorkloadKind::HeadroomV4,
        group: WorkloadGroup::Baseline,
        summary: "стенд V4 4096x2304 без шумодава",
    },
    CatalogEntry {
        name: "still",
        kind: WorkloadKind::Still,
        group: WorkloadGroup::Baseline,
        summary: "проявление снимка целиком",
    },
    // ---- Абляции прохода UI (группа ui_ablation): тот же настоящий список рисования, один вид элементов. ----
    CatalogEntry {
        name: "display_calibrate_glass_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::GlassOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только стеклянные группы",
    },
    CatalogEntry {
        name: "display_calibrate_shapes_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::ShapesOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только фигуры",
    },
    CatalogEntry {
        name: "display_calibrate_text_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::TextOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только текст и значки (глифы)",
    },
    CatalogEntry {
        name: "display_calibrate_images_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::ImagesOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только картинки (живая картинка прибора)",
    },
    CatalogEntry {
        name: "display_calibrate_no_glass",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::NoGlass),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: всё, кроме стекла",
    },
    CatalogEntry {
        name: "display_photo_glass_only",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::GlassOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: только стеклянные группы",
    },
    CatalogEntry {
        name: "display_photo_no_glass",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::NoGlass),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: всё, кроме стекла",
    },
    // ---- Абляции видоискателя (группа isp_ablation). ----
    CatalogEntry {
        name: "viewfinder_awb_never",
        kind: viewfinder(ViewfinderVariant::AwbNever),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя: статистика AWB не записывается никогда",
    },
    CatalogEntry {
        name: "viewfinder_awb_every_frame",
        kind: viewfinder(ViewfinderVariant::AwbEveryFrame),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя: статистика AWB на каждом кадре",
    },
    CatalogEntry {
        name: "viewfinder_without_levels",
        kind: without(AblatedStage::Levels),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии levels",
    },
    CatalogEntry {
        name: "viewfinder_without_lens_shading",
        kind: without(AblatedStage::LensShading),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии lens_shading",
    },
    CatalogEntry {
        name: "viewfinder_without_white_balance",
        kind: without(AblatedStage::WhiteBalance),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии white_balance",
    },
    CatalogEntry {
        name: "viewfinder_without_demosaic",
        kind: without(AblatedStage::Demosaic),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии demosaic (читается один отсчёт ячейки вместо четырёх, пока включена статистика AWB — всё равно четыре)",
    },
    CatalogEntry {
        name: "viewfinder_without_color",
        kind: without(AblatedStage::Color),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии color",
    },
    CatalogEntry {
        name: "viewfinder_without_gamut_compression",
        kind: without(AblatedStage::GamutCompression),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии gamut_compression",
    },
    CatalogEntry {
        name: "viewfinder_without_tone",
        kind: without(AblatedStage::Tone),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии tone",
    },
    CatalogEntry {
        name: "viewfinder_without_look_color",
        kind: without(AblatedStage::LookColor),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии look_color",
    },
    CatalogEntry {
        name: "viewfinder_without_look_lut",
        kind: without(AblatedStage::LookLut),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии look_lut (в продукте она и так вырезается кадром без таблицы вида)",
    },
    CatalogEntry {
        name: "viewfinder_without_output_gamut_compression",
        kind: without(AblatedStage::OutputGamutCompression),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии output_gamut_compression (у прежней основы тона её в проходе нет)",
    },
    CatalogEntry {
        name: "viewfinder_without_oetf",
        kind: without(AblatedStage::Oetf),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии oetf",
    },
    CatalogEntry {
        name: "viewfinder_minimal",
        kind: viewfinder(ViewfinderVariant::Minimal),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя, пол цены: только levels + demosaic (4 отсчёта RAW на пиксель, запись картинки)",
    },
    // ---- Абляции накопления приборов (группа scopes_ablation). ----
    CatalogEntry {
        name: "scopes_waveform_empty",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_waveform_read_only",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_waveform_skip_write",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_waveform_cell_zero",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_waveform_spread",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_parade_empty",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_parade_read_only",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_parade_skip_write",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_parade_cell_zero",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_parade_spread",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_empty",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_read_only",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_skip_write",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_cell_zero",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_spread",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_waveform_aggregated",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_parade_aggregated",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_aggregated",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_waveform_runs4",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs8",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs4",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs8",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs16",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 16 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs16",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 16 отсчётов на нить, серии в регистре",
    },
];

/// Имя в каталоге нет.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("неизвестная работа стенда насыщения {name:?}; доступны: {available}")]
pub struct UnknownWorkload {
    pub name: String,
    /// Имена каталога через запятую (для сообщения).
    pub available: String,
}

/// Строка каталога по имени.
pub fn find_entry(name: &str) -> Result<&'static CatalogEntry, UnknownWorkload> {
    CATALOG
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| UnknownWorkload {
            name: name.to_owned(),
            available: CATALOG
                .iter()
                .map(|entry| entry.name)
                .chain(WorkloadGroup::ALL.map(WorkloadGroup::name))
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// Имена ВСЕХ работ каталога в порядке каталога (не умолчание: умолчание — [`baseline_names`]).
pub fn all_names() -> Vec<String> {
    CATALOG.iter().map(|entry| entry.name.to_owned()).collect()
}

/// Работы группы в порядке каталога.
pub fn group_entries(group: WorkloadGroup) -> Vec<&'static CatalogEntry> {
    CATALOG.iter().filter(|entry| entry.group == group).collect()
}

/// Имена работ группы `baseline` в порядке прогона: умолчание списка в конфиге. Новые работы каталога (абляции)
/// умолчание НЕ расширяют.
pub fn baseline_names() -> Vec<String> {
    group_entries(WorkloadGroup::Baseline)
        .into_iter()
        .map(|entry| entry.name.to_owned())
        .collect()
}

/// Раскрывает имя из конфига или флага: имя группы — в её работы, имя работы — в неё одну.
pub fn expand_name(name: &str) -> Result<Vec<&'static CatalogEntry>, UnknownWorkload> {
    match WorkloadGroup::by_name(name) {
        Some(group) => Ok(group_entries(group)),
        None => find_entry(name).map(|entry| vec![entry]),
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
