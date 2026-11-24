//! Числа, которые GLSL не может взять из Rust и повторяет литералами (размеры массивов буферов), и номера
//! specialization-констант размера группы: тест читает ИСХОДНИКИ шейдеров и сверяет каждое число с константой
//! Rust, чтобы правка одной стороны без другой ломала сборку тестов, а не читала память за границей буфера.

use scope_model::MAX_GRATICULE_PRIMITIVES;

use super::accumulators::{INTEGER_RUNS_ROWS_PER_THREAD, LOCAL_SIZE_X_ID, LOCAL_SIZE_Y_ID};
use super::parameter_blocks::{
    SCAN_CODE_ROTATED_CLOCKWISE, SCAN_CODE_ROTATED_COUNTER_CLOCKWISE, SCAN_CODE_UPRIGHT, SCAN_CODE_UPSIDE_DOWN,
};
use super::rule::{DECODE_TABLE_LEN, HEADER_WORDS, PARAMETER_WORDS};

const ACCUMULATE_COMMON: &str = include_str!("../../shaders/scope_accumulate_common.glsl");
const INTEGER_RUNS: &str = include_str!("../../shaders/scope_accumulate_integer_runs.comp");
const RENDER: &str = include_str!("../../shaders/scope_render.comp");

/// Размер массива `name[N]` из исходника шейдера; ищется ровно одно объявление.
fn array_length(source: &str, name: &str) -> usize {
    let needle = format!("{name}[");
    let mut found = source.match_indices(&needle).filter_map(|(at, _)| {
        let tail = &source[at + needle.len()..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        // Обращения `name[index]` цифр не дают (индекс — выражение), объявление — число и `]`.
        (!digits.is_empty() && tail[digits.len()..].starts_with(']')).then(|| digits.parse::<usize>().ok())
    });
    let length = found
        .next()
        .flatten()
        .unwrap_or_else(|| panic!("в шейдере нет объявления {name}[N]"));
    assert!(found.next().is_none(), "объявление {name}[N] должно быть одно");
    length
}

#[test]
fn parameter_buffer_array_lengths_in_shaders_equal_the_rust_constants() {
    assert_eq!(array_length(ACCUMULATE_COMMON, "words"), PARAMETER_WORDS);
    assert_eq!(array_length(ACCUMULATE_COMMON, "decode_table"), DECODE_TABLE_LEN);
    assert_eq!(array_length(RENDER, "primitives"), MAX_GRATICULE_PRIMITIVES);
    // Список отобранных примитивов в shared-памяти вмещает все примитивы сетки.
    assert_eq!(array_length(RENDER, "kept_primitive_index"), MAX_GRATICULE_PRIMITIVES);
}

#[test]
fn workgroup_size_comes_from_specialization_constants_with_the_rust_ids_not_from_literals() {
    let layout = format!("layout(local_size_x_id = {LOCAL_SIZE_X_ID}, local_size_y_id = {LOCAL_SIZE_Y_ID}) in;");
    for (name, source) in [
        ("scope_accumulate_common.glsl", ACCUMULATE_COMMON),
        ("scope_render.comp", RENDER),
    ] {
        assert!(source.contains(&layout), "{name}: ожидалось `{layout}`");
        assert!(
            !source.contains("local_size_x ="),
            "{name}: размер группы задан литералом, а не константой Rust"
        );
    }
}

#[test]
fn the_array_length_reader_ignores_accesses_and_rejects_a_missing_declaration() {
    let source = "uint words[64]; x = words[WORD_GRID] + words[i];";
    assert_eq!(array_length(source, "words"), 64);
    let result = std::panic::catch_unwind(|| array_length("x = words[i];", "words"));
    assert!(result.is_err(), "без объявления читатель не угадывает число");
}

#[test]
fn scan_codes_and_header_word_in_the_shader_equal_the_rust_constants() {
    for (name, code) in [
        ("SCAN_UPRIGHT", SCAN_CODE_UPRIGHT),
        ("SCAN_ROTATED_CLOCKWISE", SCAN_CODE_ROTATED_CLOCKWISE),
        ("SCAN_UPSIDE_DOWN", SCAN_CODE_UPSIDE_DOWN),
        ("SCAN_ROTATED_COUNTER_CLOCKWISE", SCAN_CODE_ROTATED_COUNTER_CLOCKWISE),
    ] {
        let declaration = format!("const uint {name} = {code}u;");
        assert!(
            ACCUMULATE_COMMON.contains(&declaration),
            "в шейдере нет `{declaration}`"
        );
    }
    // Слово чтения — последнее слово заголовка, прибор начинается сразу за ним.
    assert!(ACCUMULATE_COMMON.contains(&format!("const uint WORD_SCAN = {}u;", HEADER_WORDS - 1)));
    assert!(ACCUMULATE_COMMON.contains(&format!("const uint WORD_RULE_FIRST = {HEADER_WORDS}u;")));
}

#[test]
fn rows_per_thread_of_the_runs_shader_equal_the_rust_constant() {
    let declaration = format!("#define SCOPE_ROWS_PER_THREAD {INTEGER_RUNS_ROWS_PER_THREAD}u");
    assert!(
        INTEGER_RUNS.contains(&declaration),
        "в scope_accumulate_integer_runs.comp нет `{declaration}`: dispatch и шейдер разойдутся"
    );
}
