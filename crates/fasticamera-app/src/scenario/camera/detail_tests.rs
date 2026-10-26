//! Раскладка подробностей по строкам: ничего не теряется, ни одна строка не выходит за бюджет.

use serde_json::json;

use super::*;

fn item(index: usize, filler: usize) -> Value {
    json!({ "index": index, "filler": "x".repeat(filler) })
}

fn line_bytes(line: &[Value]) -> usize {
    Value::Array(line.to_vec()).to_string().len()
}

#[test]
fn small_items_share_one_line() {
    let lines = pack_items((0..3).map(|index| item(index, 10)).collect(), 1_000);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 3);
}

#[test]
fn items_are_split_in_order_and_every_line_fits_the_budget() {
    let items: Vec<Value> = (0..10).map(|index| item(index, 300)).collect();
    let lines = pack_items(items.clone(), 1_000);
    assert!(lines.len() > 1, "десять элементов по 300 байт не влезают в 1000 байт");
    for line in &lines {
        assert!(line_bytes(line) <= 1_000, "строка {} байт", line_bytes(line));
    }
    let flattened: Vec<Value> = lines.into_iter().flatten().collect();
    assert_eq!(flattened, items, "порядок и состав сохранены");
}

#[test]
fn item_larger_than_the_budget_is_replaced_by_a_size_record_not_dropped_or_cut() {
    let lines = pack_items(vec![item(0, 10), item(1, 5_000), item(2, 10)], 1_000);
    let flattened: Vec<Value> = lines.into_iter().flatten().collect();
    assert_eq!(flattened.len(), 3, "элемент не потерян");
    assert_eq!(flattened[1]["budget_bytes"], 1_000);
    assert!(flattened[1]["oversized_item_bytes"].as_u64().expect("размер") > 5_000);
    assert_eq!(flattened[0], item(0, 10));
    assert_eq!(flattened[2], item(2, 10));
}

#[test]
fn no_items_give_no_lines() {
    assert!(pack_items(Vec::new(), 1_000).is_empty());
}

#[test]
fn serialization_failure_becomes_an_explanation_instead_of_a_panic() {
    struct Broken;
    impl serde::Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("сломан"))
        }
    }
    assert_eq!(to_value(&Broken)["serialization_error"], "сломан");
    assert_eq!(to_value(&vec![1, 2]), json!([1, 2]));
}
