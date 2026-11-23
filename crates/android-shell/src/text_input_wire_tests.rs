use super::*;

fn request(id: u64, purpose: TextPurpose) -> TextRequest {
    TextRequest {
        id: TextRequestId(id),
        purpose,
        initial_text: "Warm".to_owned(),
        max_chars: 30,
    }
}

#[test]
fn purpose_codes_are_unique_and_match_kotlin_constants() {
    let codes: Vec<i32> = TextPurpose::ALL.into_iter().map(text_purpose_code).collect();
    // TextInputDialog.PURPOSE_NEW_PRESET_NAME / PURPOSE_RENAME_PRESET = 1 / 2.
    assert_eq!(codes, vec![1, 2]);
}

#[test]
fn encoded_request_carries_all_fields() {
    let wire = encode_text_request(&request(7, TextPurpose::RenamePreset));
    assert_eq!(
        wire,
        TextRequestWire {
            id: 7,
            purpose_code: 2,
            initial_text: "Warm".to_owned(),
            max_chars: 30,
        }
    );
}

#[test]
fn request_id_round_trips_through_long_including_high_bit() {
    for id in [0, 1, u64::from(u32::MAX), i64::MAX as u64, u64::MAX] {
        let wire = encode_text_request(&request(id, TextPurpose::NewPresetName));
        let decoded = decode_text_input_result(wire.id, OUTCOME_CANCELLED, None).expect("итог");
        assert_eq!(decoded.id, TextRequestId(id));
    }
}

#[test]
fn huge_limit_saturates_instead_of_wrapping_negative() {
    let mut huge = request(1, TextPurpose::NewPresetName);
    huge.max_chars = u32::MAX;
    assert_eq!(encode_text_request(&huge).max_chars, i32::MAX);
}

#[test]
fn decodes_the_three_platform_outcomes() {
    assert_eq!(
        decode_text_input_result(5, OUTCOME_ENTERED, Some("Film".to_owned())),
        Ok(TextInputResult {
            id: TextRequestId(5),
            outcome: TextInputOutcome::Entered("Film".to_owned())
        })
    );
    assert_eq!(
        decode_text_input_result(5, OUTCOME_CANCELLED, None).map(|result| result.outcome),
        Ok(TextInputOutcome::Cancelled)
    );
    assert_eq!(
        decode_text_input_result(5, OUTCOME_UNAVAILABLE, None).map(|result| result.outcome),
        Ok(TextInputOutcome::Unavailable)
    );
}

#[test]
fn text_is_ignored_unless_entered() {
    assert_eq!(
        decode_text_input_result(5, OUTCOME_CANCELLED, Some("leftover".to_owned())).map(|result| result.outcome),
        Ok(TextInputOutcome::Cancelled)
    );
}

#[test]
fn entered_without_text_is_a_contract_error() {
    assert_eq!(
        decode_text_input_result(5, OUTCOME_ENTERED, None),
        Err(TextInputDecodeError::EnteredWithoutText)
    );
}

#[test]
fn unknown_outcome_code_is_a_contract_error() {
    assert_eq!(
        decode_text_input_result(5, 0, None),
        Err(TextInputDecodeError::UnknownOutcome(0))
    );
    assert_eq!(
        decode_text_input_result(5, 4, None),
        Err(TextInputDecodeError::UnknownOutcome(4))
    );
}
