//! Основа тона на GPU: ОДИН общий uniform-блок `ToneBaseParameters` под любую модель основы для ВСЕХ путей
//! (видоискатель, качественный конвейер, проявление снимка) и выбор модели (решение 182).
//!
//! Блок — только «хранилище»: массив из [`TONE_BASE_ROWS_MAX`] строк `vec4`. Что лежит в строках модели, знают
//! её упаковщик (`tone_base/<модель>.rs`) и её GLSL-файл (`shaders/tone_base_<модель>.glsl`); общий код
//! ([`pack_tone_base_uniforms`], `shaders/tone_base.glsl`) знает только строку 0 и номер модели. Числа — ровно те
//! f32, что отдаёт модель `isp-model` (эталон подставляет их же): путь берёт блок как есть и ничего не пересчитывает.
//!
//! # Раскладка (std140, строки по 16 байт; зеркало `shaders/tone_base.glsl`)
//! | Строка | Смещение | Содержимое |
//! |---|---|---|
//! | 0 | 0 | общая: xyz — усиления баланса белого R, G, B (для белого точки `L`), w — номер модели ([`ToneBaseKind::model_id`], целое как float) |
//! | 1… | 16 | числа выбранной модели (раскладки — [`agx`], [`aces`], [`filmic`], [`legacy`]); неиспользуемые строки — нули |
//!
//! Все 64 строки пишутся каждый кадр (1 КиБ: меняются с подтяжкой AE). Флаги внутри моделей — числа float
//! (0.0 / 1.0), а не uint-биты: `floatBitsToUint` на денормализованных значениях небезопасен.
//!
//! # Почему 64 строки
//! Четыре модели: старая — только строка 0 (1 строка); AgX с возвратом оттенка — 1 + 33 = 34 строки (матрицы 12,
//! кривая 7, флаги 1, OKLab 13); filmic — 1 + 28 = 29 строк (матрицы 6, кривая 4, сжатия охвата 4, флаги и яркость 2,
//! OKLab 12); ACES — 1 + 44 = 45 строк (самая большая: сплайны RRT и ODT по 9 строк). 64 — степень двойки с запасом
//! над самой большой моделью (ACES) и 1/16 от минимума, который гарантирует Vulkan для uniform-блока
//! (16 КиБ). Строки за краем блока не пишутся (`write_row` их отбрасывает, а не портит память), поэтому защита от
//! переполнения — тест «модель пишет ровно `ROW_COUNT` строк и помещается в блок»: добавляя модель, добавьте её в него.
//!
//! # Как добавить модель основы на GPU
//! Предусловие — CPU-часть готова (`isp-model::tone::base`: вариант в `ToneBase`/`ResolvedToneBase` и её
//! `...Parameters` с готовыми f32).
//! 1. **Упаковщик**: `src/tone_base/<модель>.rs` с функцией `pack(rows, &Parameters)`: пишет строки МОДЕЛИ
//!    (строка 0 пишется общим кодом) через [`ToneBaseRows::push`] / [`ToneBaseRows::push_matrix3`]; в доке файла —
//!    таблица строк (смещение от первой строки модели → содержимое). Константа `ROW_COUNT` — сколько строк
//!    занято (тест проверяет, что упаковщик пишет ровно столько).
//! 2. **Номер**: вариант в [`ToneBaseKind`] (+ `model_id`) и ветка в [`ToneBaseKind::of`] и в
//!    [`pack_tone_base_uniforms`] — единственная точка выбора модели на стороне Rust.
//! 3. **GLSL**: `shaders/tone_base_<модель>.glsl` — включает `tone_base.glsl` (блок и помощники), объявляет
//!    константы строк от `TONE_BASE_FIRST_MODEL_ROW` (зеркало таблицы упаковщика) и функции модели; номер
//!    модели добавляется в `tone_base.glsl` (`TONE_MODEL_*`, зеркало `model_id`). Соседние модели не правятся.
//! 4. **Выбор в шейдере**: у видоискателя — ветка по константе специализации `TONE_BASE_MODEL` (вариант
//!    пайплайна на модель, `viewfinder_isp.glsl`), у выходного прохода качественного пути и снимка — ветка по
//!    `tone_base_model()` (`quality_output_main.glsl`). Слой вида (`look_stage.glsl`) и выход (OETF) от модели
//!    не зависят и не правятся.
//! 5. **Тесты**: раскладка (смещения по таблице), «модель помещается в блок», паритет GPU с эталоном `isp-model`
//!    (видоискатель, качественный путь, снимок: допуски те же, что у AgX), переключение моделей на одном ISP
//!    даёт по варианту на модель.
//!
//! Слой вида работает над светом экрана ПОСЛЕ основы и о модели не знает: ни ручки цвета, ни LUT при добавлении
//! модели не трогаются.

pub(crate) mod aces;
pub(crate) mod agx;
pub(crate) mod filmic;
pub(crate) mod legacy;

use isp_model::gamut::GamutCompressionParameters;
use isp_model::params::IspParams;
use isp_model::tone::base::ResolvedToneBase;

/// Наибольшее число строк блока (зеркало `TONE_BASE_ROWS_MAX` в `tone_base.glsl`).
pub(crate) const TONE_BASE_ROWS_MAX: usize = 64;

/// Размер блока в байтах: все [`TONE_BASE_ROWS_MAX`] строк по 16 байт.
pub(crate) const TONE_BASE_UNIFORM_BYTES: usize = TONE_BASE_ROWS_MAX * 16;

/// Номер строки, с которой начинаются числа модели (строка 0 — общая).
pub(crate) const FIRST_MODEL_ROW: usize = 1;

/// Какая модель основы тона у кадра (ключ варианта пайплайна видоискателя и тема проверок).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToneBaseKind {
    /// Прежняя кривая: обрезка по 1,0, выход как до 14a.
    Legacy,
    /// AgX со своим белым точки.
    Agx,
    /// ACES (RRT + выходное преобразование) со своим белым точки.
    Aces,
    /// filmic (кривая по норме пикселя, обесцвечивание в светах, «вписывание» в охват выхода) со своим белым точки.
    Filmic,
}

impl ToneBaseKind {
    /// Модель, которую выбрали параметры кадра. Ветки явные, без `_ =>`: новая модель в `isp-model` не пройдёт
    /// молча (не соберётся), пока для неё нет варианта здесь и упаковщика в [`pack_tone_base_uniforms`].
    pub(crate) fn of(params: &IspParams) -> Self {
        match &params.tone_base {
            ResolvedToneBase::Legacy => Self::Legacy,
            ResolvedToneBase::Agx(_) => Self::Agx,
            ResolvedToneBase::Aces(_) => Self::Aces,
            ResolvedToneBase::Filmic(_) => Self::Filmic,
        }
    }

    /// Номер модели в шейдере (зеркало `TONE_MODEL_*` в `tone_base.glsl`): значение константы специализации
    /// `TONE_BASE_MODEL` видоискателя и число в строке 0 блока для качественного пути.
    pub(crate) fn model_id(self) -> u32 {
        match self {
            Self::Legacy => 0,
            Self::Agx => 1,
            Self::Aces => 2,
            Self::Filmic => 3,
        }
    }
}

/// Значение флага «вход» в строке флагов модели: 0 — обрезка нулём, 1 — мягкое сжатие охвата (зеркало шейдера).
pub(crate) fn input_compression_flag(compression: Option<&GamutCompressionParameters>) -> f32 {
    match compression {
        Some(_) => 1.0,
        None => 0.0,
    }
}

/// Строки блока модели: пишущая обёртка, которая считает занятое и не даёт выйти за [`TONE_BASE_ROWS_MAX`].
#[derive(Debug, Default)]
pub(crate) struct ToneBaseRows {
    rows: Vec<[f32; 4]>,
}

impl ToneBaseRows {
    /// Строка `vec4`.
    pub(crate) fn push(&mut self, row: [f32; 4]) {
        self.rows.push(row);
    }

    /// Три строки `vec4` — матрица 3×3 по строкам (четвёртое число строки — 0).
    pub(crate) fn push_matrix3(&mut self, matrix: [[f32; 3]; 3]) {
        for row in matrix {
            self.push([row[0], row[1], row[2], 0.0]);
        }
    }

    /// Две строки сжатия охвата (порог по каналам + степень `p`, масштаб по каналам + `1/p`): общая раскладка
    /// «сжатие охвата» для входа и выхода всех моделей. Эталон считает `1.0 / power` в f32 на пиксель — здесь
    /// та же операция один раз.
    pub(crate) fn push_gamut_compression(&mut self, compression: &GamutCompressionParameters) {
        self.push([
            compression.threshold[0],
            compression.threshold[1],
            compression.threshold[2],
            compression.power,
        ]);
        self.push([
            compression.scale[0],
            compression.scale[1],
            compression.scale[2],
            1.0 / compression.power,
        ]);
    }

    /// Две строки сжатия охвата НА ВХОДЕ модели. Вход «обрезка нулём» (`None`) шейдер порогов не читает: нули с
    /// безопасной степенью 1. Флаг режима — [`input_compression_flag`] (его модель кладёт в свою строку флагов).
    pub(crate) fn push_input_gamut_compression(&mut self, compression: Option<&GamutCompressionParameters>) {
        match compression {
            Some(compression) => self.push_gamut_compression(compression),
            None => {
                self.push([0.0, 0.0, 0.0, 1.0]);
                self.push([1.0, 1.0, 1.0, 1.0]);
            }
        }
    }

    /// Сколько строк записано (тесты сверяют с `ROW_COUNT` модели).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }
}

/// Собирает байты блока `ToneBaseParameters`: общая строка 0 и строки выбранной модели; остальное — нули.
///
/// Строк больше [`TONE_BASE_ROWS_MAX`] не бывает по построению (тест «модель помещается в блок»); если бы
/// упаковщик всё же переполнил блок, лишнее не пишется, а не портит память.
pub(crate) fn pack_tone_base_uniforms(params: &IspParams) -> [u8; TONE_BASE_UNIFORM_BYTES] {
    let kind = ToneBaseKind::of(params);
    let gains = params.white_balance.as_array();
    let mut model_rows = ToneBaseRows::default();
    match &params.tone_base {
        ResolvedToneBase::Legacy => legacy::pack(&mut model_rows),
        ResolvedToneBase::Agx(base) => agx::pack(&mut model_rows, &base.parameters()),
        ResolvedToneBase::Aces(base) => aces::pack(&mut model_rows, &base.parameters()),
        ResolvedToneBase::Filmic(base) => filmic::pack(&mut model_rows, &base.parameters()),
    }
    let mut bytes = [0_u8; TONE_BASE_UNIFORM_BYTES];
    let common = [gains[0], gains[1], gains[2], kind.model_id() as f32];
    write_row(&mut bytes, 0, common);
    for (offset, row) in model_rows.rows.into_iter().enumerate() {
        write_row(&mut bytes, FIRST_MODEL_ROW + offset, row);
    }
    bytes
}

/// Пишет строку `row` блока; строка за краем блока не пишется (см. доку [`pack_tone_base_uniforms`]).
fn write_row(bytes: &mut [u8; TONE_BASE_UNIFORM_BYTES], row_index: usize, row: [f32; 4]) {
    let Some(target) = bytes.get_mut(16 * row_index..16 * row_index + 16) else {
        return;
    };
    for (word, value) in target.chunks_exact_mut(4).zip(row) {
        word.copy_from_slice(&value.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests;
