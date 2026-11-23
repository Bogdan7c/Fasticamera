use super::*;

fn request(max_chars: u32) -> TextRequest {
    TextRequest {
        id: TextRequestId(1),
        purpose: TextPurpose::NewPresetName,
        initial_text: String::new(),
        max_chars,
    }
}

#[test]
fn normalize_trims_edges_and_keeps_inner_spaces() {
    assert_eq!(
        request(30).normalize_entry("  Warm  film \t"),
        Some("Warm  film".to_owned())
    );
}

#[test]
fn normalize_removes_control_characters_including_newlines() {
    assert_eq!(
        request(30).normalize_entry("Warm\nfilm\u{7}\r"),
        Some("Warmfilm".to_owned())
    );
}

#[test]
fn normalize_truncates_by_chars_not_bytes() {
    // Кириллица — 2 байта на символ: предел считается символами, иначе имя обрезалось бы вдвое раньше.
    assert_eq!(
        request(4).normalize_entry("Плёнка тёплая"),
        Some("Плёнк".chars().take(4).collect())
    );
}

#[test]
fn normalize_does_not_leave_trailing_space_after_truncation() {
    assert_eq!(request(5).normalize_entry("Warm film"), Some("Warm".to_owned()));
}

#[test]
fn normalize_rejects_empty_after_cleanup() {
    assert_eq!(request(30).normalize_entry(" \n\t "), None);
    assert_eq!(request(30).normalize_entry(""), None);
    assert_eq!(request(0).normalize_entry("anything"), None);
}

#[test]
fn purposes_are_named_like_serde() {
    for purpose in TextPurpose::ALL {
        assert_eq!(
            serde_json::to_value(purpose).expect("json"),
            serde_json::json!(purpose.name())
        );
    }
}

#[test]
fn outcome_serializes_with_tag_and_text() {
    assert_eq!(
        serde_json::to_value(TextInputOutcome::Entered("A".to_owned())).expect("json"),
        serde_json::json!({"outcome": "entered", "text": "A"})
    );
    assert_eq!(
        serde_json::to_value(TextInputOutcome::Superseded).expect("json"),
        serde_json::json!({"outcome": "superseded"})
    );
}
