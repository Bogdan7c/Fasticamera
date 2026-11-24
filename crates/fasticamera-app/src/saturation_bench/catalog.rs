//! Каталог работ стенда насыщения: имена для конфига и флага, и что за каждым именем стоит.
//!
//! Каталог — ДАННЫЕ (таблица [`CATALOG`]), а не `if name == …` по коду: добавить работу значит добавить строку
//! и ветку в сборщике работ. Модуль не знает GPU: имена проверяются при разборе конфига, до любого запуска.

use serde::Serialize;

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

/// Что за работа стоит за именем каталога.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "work", rename_all = "snake_case")]
pub enum WorkloadKind {
    /// Слитый проход видоискателя на RAW10 продуктового размера.
    Viewfinder,
    /// Цветовой прибор.
    Scopes { shape: ScopeShape, input: ScopeInputKind },
    /// Только компоновщик (картинка видоискателя в цель экрана).
    Compositor,
    /// Компоновщик + рендер UI настоящего экрана камеры.
    Display {
        screen: DisplayScreen,
        rhythm: BackdropRhythm,
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
    /// Что мерит работа (одна строка для справки и ошибок).
    pub summary: &'static str,
}

const fn scopes(shape: ScopeShape, input: ScopeInputKind) -> WorkloadKind {
    WorkloadKind::Scopes { shape, input }
}

const fn display(screen: DisplayScreen, rhythm: BackdropRhythm) -> WorkloadKind {
    WorkloadKind::Display { screen, rhythm }
}

/// Все работы стенда в порядке прогона по умолчанию.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "viewfinder",
        kind: WorkloadKind::Viewfinder,
        summary: "ISP видоискателя: RAW10 4096x3072, все стадии, AWB каждый 4-й кадр",
    },
    CatalogEntry {
        name: "scopes_waveform",
        kind: scopes(ScopeShape::Waveform, ScopeInputKind::Natural),
        summary: "прибор waveform на сцене",
    },
    CatalogEntry {
        name: "scopes_parade",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::Natural),
        summary: "прибор RGB parade на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::Natural),
        summary: "вектороскоп Cb/Cr на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_oklab",
        kind: scopes(ScopeShape::VectorscopeOklab, ScopeInputKind::Natural),
        summary: "вектороскоп a/b OKLab на сцене",
    },
    CatalogEntry {
        name: "scopes_parade_gray",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::UniformGray),
        summary: "parade на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "scopes_vectorscope_gray",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::UniformGray),
        summary: "вектороскоп на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "compositor",
        kind: WorkloadKind::Compositor,
        summary: "компоновщик: картинка видоискателя в цель экрана",
    },
    CatalogEntry {
        name: "display_photo",
        kind: display(DisplayScreen::Photo, BackdropRhythm::NewFrameEveryFrame),
        summary: "кадр показа экрана PHOTO, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_photo_recompose",
        kind: display(DisplayScreen::Photo, BackdropRhythm::SameFrame),
        summary: "кадр показа экрана PHOTO, фон тот же кадр",
    },
    CatalogEntry {
        name: "display_calibrate",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::NewFrameEveryFrame),
        summary: "кадр показа раздела CALIBRATE с живой картинкой прибора, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_calibrate_recompose",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::SameFrame),
        summary: "кадр показа раздела CALIBRATE, фон тот же кадр",
    },
    CatalogEntry {
        name: "headroom_v4",
        kind: WorkloadKind::HeadroomV4,
        summary: "стенд V4 4096x2304 без шумодава",
    },
    CatalogEntry {
        name: "still",
        kind: WorkloadKind::Still,
        summary: "проявление снимка целиком",
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
            available: CATALOG.iter().map(|entry| entry.name).collect::<Vec<_>>().join(", "),
        })
}

/// Имена всех работ каталога (умолчание списка в конфиге).
pub fn all_names() -> Vec<String> {
    CATALOG.iter().map(|entry| entry.name.to_owned()).collect()
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
