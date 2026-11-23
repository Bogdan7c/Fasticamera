//! Числа, которые GLSL не может взять из Rust и повторяет литералами (размеры массивов буферов), и номера
//! specialization-констант размера группы: тест читает ИСХОДНИКИ шейдеров и сверяет каждое число с константой
//! Rust, чтобы правка одной стороны без другой ломала сборку тестов, а не читала память за границей буфера.

use scope_model::MAX_GRATICULE_PRIMITIVES;

use super::accumulators::{LOCAL_SIZE_X_ID, LOCAL_SIZE_Y_ID};
use super::rule::{DECODE_TABLE_LEN, PARAMETER_WORDS};

const ACCUMULATE_COMMON: &str = include_str!("../../shaders/scope_accumulate_common.glsl");
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
