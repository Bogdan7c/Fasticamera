use super::*;

/// Колонка `width` × `height` pt.
fn frames_for(width: f64, height: f64) -> Result<BarsFrames, BarsFramesError> {
    let config = CalibrationWidgetsConfig::default();
    let panel = Rect::from_xywh(0.0, 0.0, width, height);
    BarsFrames::of(super::super::ColumnFrames::of(panel, &config).content, &config)
}

#[test]
fn tabs_and_three_rows_stack_without_overlap_and_inside_the_window() {
    let config = CalibrationWidgetsConfig::default();
    let frames = frames_for(196.0, 666.0).expect("портретная колонка вмещает вид");
    assert!(frames.tabs.max_y() <= frames.rows[0].min_y());
    for pair in frames.rows.windows(2) {
        assert_eq!(pair[0].max_y(), pair[1].min_y(), "строки встык, как в списке шкал");
    }
    assert!(
        frames.rows[2].max_y() <= 666.0 - config.panel_padding_pt,
        "без прокрутки"
    );
    assert!(
        frames.rows[0].min_y() - frames.tabs.max_y() >= config.row_reset_reach_above_pt,
        "«↻» первой строки не залезает на вкладки"
    );
}

#[test]
fn landscape_column_of_the_reference_device_fits_with_room_to_spare() {
    // Альбом эталона: окно под шапкой и переключателем 272 pt (решение 343); вкладки и три шкалы — 248 pt и отступ 12 pt.
    let config = CalibrationWidgetsConfig::default();
    let window_height = 272.0;
    let content = Rect::from_xywh(0.0, 0.0, 300.0, window_height);
    let frames = BarsFrames::of(content, &config).expect("альбомное окно вмещает вид");
    assert_eq!(frames.rows[2].max_y(), 36.0 + 8.0 + 3.0 * 68.0);
    assert!(frames.rows[2].max_y() + config.panel_padding_pt <= window_height);
}

#[test]
fn window_too_short_for_the_tabs_and_three_rows_is_an_error_not_a_cut_view() {
    let content = Rect::from_xywh(0.0, 0.0, 300.0, 255.0);
    let result = BarsFrames::of(content, &CalibrationWidgetsConfig::default());
    assert_eq!(
        result,
        Err(BarsFramesError::NoRoom {
            required_pt: 260.0,
            available_pt: 255.0
        })
    );
}
