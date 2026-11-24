//! Шов «прибор на GPU»: что нужно проходам накопления и показа от конкретного прибора.
//!
//! Прибор = ИДЕНТИФИКАТОР шейдера накопления ([`AccumulatorId`]) + форма сетки + параметры шейдера (слова и,
//! если нужно, таблица) + цвета следа + список примитивов сетки. Это ДАННЫЕ: [`GpuScopeRule`] только отдаёт их,
//! а [`crate::scopes::ColorScopes`] не знает, какой это прибор. Новый прибор — новая реализация [`GpuScopeRule`]
//! и новый шейдер накопления, зарегистрированный при создании; соседние приборы, общая часть шейдера
//! (`scope_accumulate_common.glsl`) и проход показа не правятся.
//!
//! Для приборов `scope-model` реализация лежит в [`super::resolved_rule`].

use scope_contract::ScopeScan;
use scope_model::projection::IntegerProjection;

use super::error::ScopeError;
use scope_model::{GraticulePrimitive, GridShape, RenderSettings};

/// Сколько слов параметров занимают общий заголовок (форма сетки и чтение картинки глазом): с `HEADER_WORDS` начинаются слова прибора.
pub(super) const HEADER_WORDS: usize = 4;

/// Всего слов в блоке параметров шейдера накопления (заголовок + слова прибора).
pub(super) const PARAMETER_WORDS: usize = 64;

/// Сколько слов остаётся прибору.
pub const RULE_WORD_CAPACITY: usize = PARAMETER_WORDS - HEADER_WORDS;

/// Размер таблицы «код 0…1023 → свет экрана».
pub const DECODE_TABLE_LEN: usize = 1024;

/// Идентификатор шейдера накопления. Прибор указывает, КАКОЙ зарегистрированный шейдер его считает;
/// пайплайны всех шейдеров создаются один раз при создании [`crate::scopes::ColorScopes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccumulatorId(&'static str);

impl AccumulatorId {
    /// Целочисленная проекция: waveform, RGB parade, вектороскоп Y'CbCr.
    pub const INTEGER_PROJECTION: Self = Self("integer-projection");
    /// Вектороскоп по осям a/b OKLab.
    pub const OKLAB_VECTORSCOPE: Self = Self("oklab-vectorscope");

    /// То же, что [`Self::INTEGER_PROJECTION`], но нить обходит несколько отсчётов по y и сливает серии одинаковых
    /// ячеек в регистре (waveform и parade: много записей `atomicAdd`, мало конкуренции).
    pub const INTEGER_PROJECTION_RUNS: Self = Self("integer-projection-runs");
    /// То же, что [`Self::INTEGER_PROJECTION`], но одинаковые ячейки сливаются внутри subgroup перед `atomicAdd`
    /// (прибор с сильной конкуренцией за ячейки: вектороскоп Y'CbCr). Нужен баллот subgroup; без него
    /// [`ColorScopes`](crate::scopes::ColorScopes) берёт [`Self::fallback`].
    pub const INTEGER_PROJECTION_MERGED: Self = Self("integer-projection-merged");
    /// То же, что [`Self::OKLAB_VECTORSCOPE`], со слиянием ячеек в subgroup.
    pub const OKLAB_VECTORSCOPE_MERGED: Self = Self("oklab-vectorscope-merged");

    /// Запасной шейдер на устройстве, где идентификатор не зарегистрирован (нет баллота subgroup): тот же прибор
    /// без слияния. У остальных идентификаторов запасного нет.
    pub const fn fallback(self) -> Option<Self> {
        if self.is(Self::INTEGER_PROJECTION_MERGED) {
            Some(Self::INTEGER_PROJECTION)
        } else if self.is(Self::OKLAB_VECTORSCOPE_MERGED) {
            Some(Self::OKLAB_VECTORSCOPE)
        } else {
            None
        }
    }

    /// Сравнение в `const`-контексте (`PartialEq` в `const fn` недоступен).
    const fn is(self, other: Self) -> bool {
        let (left, right) = (self.0.as_bytes(), other.0.as_bytes());
        if left.len() != right.len() {
            return false;
        }
        let mut index = 0;
        while index < left.len() {
            if left[index] != right[index] {
                return false;
            }
            index += 1;
        }
        true
    }

    /// Новый идентификатор (например, для прибора из теста или будущего прибора).
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// Имя для логов и ошибок.
    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// Слова параметров прибора: что шейдер читает из `words[HEADER_WORDS..]` (смысл слов знают только прибор и
/// его шейдер). Вещественные числа кладутся БИТАМИ (`f32::to_bits`): GPU получает ровно те числа, что на CPU.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleWords {
    words: Vec<u32>,
}

impl RuleWords {
    /// Пустой набор.
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавляет беззнаковое слово.
    pub fn push_u32(&mut self, value: u32) -> &mut Self {
        self.words.push(value);
        self
    }

    /// Добавляет знаковое слово (битами).
    pub fn push_i32(&mut self, value: i32) -> &mut Self {
        self.words.push(value.cast_unsigned());
        self
    }

    /// Добавляет вещественное слово (битами).
    pub fn push_f32(&mut self, value: f32) -> &mut Self {
        self.words.push(value.to_bits());
        self
    }

    /// Добавляет целочисленную проекцию: `w0 w1 w2 offset shift` (5 слов).
    pub fn push_projection(&mut self, projection: &IntegerProjection) -> &mut Self {
        for weight in projection.weights() {
            self.push_i32(weight);
        }
        self.push_i32(projection.offset());
        self.push_u32(projection.shift());
        self
    }

    /// Сколько слов набрано.
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Пуст ли набор.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub(super) fn as_slice(&self) -> &[u32] {
        &self.words
    }
}

/// Всё, что шейдер накопления читает о приборе.
#[derive(Debug, Clone)]
pub struct AccumulateParameters<'table> {
    /// Слова прибора.
    pub words: RuleWords,
    /// Таблица «код → свет экрана», если шейдер её читает (иначе `None`: в буфере остаётся прежняя).
    pub decode_table: Option<&'table [f32; DECODE_TABLE_LEN]>,
}

/// Прибор в форме, нужной GPU-проходам. См. описание модуля.
pub trait GpuScopeRule {
    /// Какой зарегистрированный шейдер накопления считает этот прибор.
    fn accumulator(&self) -> AccumulatorId;

    /// Форма сетки счётчиков (столбцы × строки × плоскости).
    fn grid(&self) -> GridShape;

    /// Как глаз читает показанную картинку слева направо (положение телефона). Уходит в слово 3 заголовка
    /// блока параметров: смена чтения меняет только параметры, ресурсы не пересоздаются. Приборам, которым
    /// положение не важно, достаточно чтения по умолчанию.
    fn scan(&self) -> ScopeScan {
        ScopeScan::Upright
    }

    /// Параметры шейдера накопления. Прибор, чьи данные не помещаются в слова шейдера, отказывает типизированной
    /// ошибкой (а не подставляет число молча): `ColorScopes::record` вернёт её до первой команды.
    fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError>;

    /// Цвет следа каждой плоскости (цвета экрана); по одному на плоскость сетки.
    fn trace_colors(&self, settings: &RenderSettings) -> Vec<[f32; 3]>;

    /// Примитивы сетки прибора (не больше [`scope_model::MAX_GRATICULE_PRIMITIVES`]).
    fn graticule(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_and_signed_values_are_stored_as_exact_bits() {
        let mut words = RuleWords::new();
        words.push_f32(1.5).push_i32(-2).push_u32(7);
        assert_eq!(words.as_slice(), &[1.5_f32.to_bits(), 0xFFFF_FFFE, 7]);
        assert_eq!(words.len(), 3);
    }

    #[test]
    fn projection_takes_five_words_in_shader_order() {
        let projection = IntegerProjection::from_integers([3, -4, 5], 6, 16).unwrap();
        let mut words = RuleWords::new();
        words.push_projection(&projection);
        assert_eq!(words.as_slice(), &[3, 0xFFFF_FFFC, 5, 6, 16]);
    }

    #[test]
    fn merged_accumulators_fall_back_to_their_plain_twins_and_others_have_no_fallback() {
        assert_eq!(
            AccumulatorId::INTEGER_PROJECTION_MERGED.fallback(),
            Some(AccumulatorId::INTEGER_PROJECTION)
        );
        assert_eq!(
            AccumulatorId::OKLAB_VECTORSCOPE_MERGED.fallback(),
            Some(AccumulatorId::OKLAB_VECTORSCOPE)
        );
        assert_eq!(AccumulatorId::INTEGER_PROJECTION.fallback(), None);
        assert_eq!(AccumulatorId::new("custom").fallback(), None);
        // Подставной «двойник» с похожим именем запасного не получает: сравнение по полному имени.
        assert_eq!(AccumulatorId::new("integer-projection-merged-x").fallback(), None);
    }

    #[test]
    fn accumulator_ids_compare_by_name() {
        assert_eq!(
            AccumulatorId::new("integer-projection"),
            AccumulatorId::INTEGER_PROJECTION
        );
        assert_ne!(AccumulatorId::INTEGER_PROJECTION, AccumulatorId::OKLAB_VECTORSCOPE);
    }
}
