//! Каталог работ стенда насыщения: имена для конфига и флага, и что за каждым именем стоит.
//!
//! Каталог — ДАННЫЕ (таблицы групп, см. [`entries`]), а не `if name == …` по коду: добавить работу значит добавить строку
//! и ветку в сборщике работ. Модуль не знает GPU: имена проверяются при разборе конфига, до любого запуска.

use serde::Serialize;

use super::draw_filter::DrawFilter;
use super::headroom_tuning::HeadroomExperiment;
use super::still_variant::StillTuning;
use super::viewfinder_tuning::{AwbSchedule, ViewfinderTuning};

mod baseline;
mod isp_ablation;
mod isp_tuning;
mod scopes_ablation;
mod still_tuning;
mod ui_ablation;
mod v4_tuning;

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
    /// Опыты сессии 16a «слитый проход»: размер группы, сведение AWB, опытная ветка шейдера.
    Tuned(ViewfinderTuning),
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
    /// Опыты над слитым проходом видоискателя (группы, чтение RAW, сведение AWB).
    IspTuning,
    /// Опыты над цепочкой V4 «запас под видео»: форма группы, пробы вместо Menon, отключённые стадии вывода.
    V4Tuning,
    /// Опыты над проявлением снимка (пакет P): высота полосы, форма рабочей группы.
    StillTuning,
}

impl WorkloadGroup {
    /// Все группы.
    pub const ALL: [WorkloadGroup; 7] = [
        Self::Baseline,
        Self::UiAblation,
        Self::IspAblation,
        Self::ScopesAblation,
        Self::IspTuning,
        Self::V4Tuning,
        Self::StillTuning,
    ];

    /// Имя группы в конфиге и флаге.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::UiAblation => "ui_ablation",
            Self::IspAblation => "isp_ablation",
            Self::ScopesAblation => "scopes_ablation",
            Self::IspTuning => "isp_tuning",
            Self::V4Tuning => "v4_tuning",
            Self::StillTuning => "still_tuning",
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
    /// Стенд V4 с опытными настройками (форма группы, пробы, отключённые стадии): цена, не картинка.
    HeadroomV4Tuned(HeadroomExperiment),
    /// Проявление снимка целиком.
    Still,
    /// Проявление снимка с опытными настройками (группа `still_tuning`).
    StillTuned { tuning: StillTuning },
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

const fn tuned(tuning: ViewfinderTuning) -> WorkloadKind {
    viewfinder(ViewfinderVariant::Tuned(tuning))
}

const fn without(stage: AblatedStage) -> WorkloadKind {
    viewfinder(ViewfinderVariant::Without { stage })
}

/// Все работы стенда в порядке прогона по умолчанию.
/// Таблицы групп в порядке каталога. Строки каждой группы лежат в своём модуле (`catalog/<группа>.rs`): каталог
/// растёт с каждым опытом, а один файл на все группы перерос предел размера модуля.
const GROUP_TABLES: [&[CatalogEntry]; 7] = [
    baseline::ENTRIES,
    ui_ablation::ENTRIES,
    isp_ablation::ENTRIES,
    scopes_ablation::ENTRIES,
    isp_tuning::ENTRIES,
    v4_tuning::ENTRIES,
    still_tuning::ENTRIES,
];

/// Все работы каталога в порядке каталога (группа за группой).
pub fn entries() -> impl Iterator<Item = &'static CatalogEntry> {
    GROUP_TABLES.iter().flat_map(|table| table.iter())
}

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
    entries()
        .find(|entry| entry.name == name)
        .ok_or_else(|| UnknownWorkload {
            name: name.to_owned(),
            available: entries()
                .map(|entry| entry.name)
                .chain(WorkloadGroup::ALL.map(WorkloadGroup::name))
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// Имена ВСЕХ работ каталога в порядке каталога (не умолчание: умолчание — [`baseline_names`]).
pub fn all_names() -> Vec<String> {
    entries().map(|entry| entry.name.to_owned()).collect()
}

/// Работы группы в порядке каталога.
pub fn group_entries(group: WorkloadGroup) -> Vec<&'static CatalogEntry> {
    entries().filter(|entry| entry.group == group).collect()
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
