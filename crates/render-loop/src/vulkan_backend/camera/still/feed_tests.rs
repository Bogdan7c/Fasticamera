use super::*;

fn stamp(nanos: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(nanos)
}

#[test]
fn recompose_repeats_the_last_live_frame_and_not_live_forgets_it() {
    let mut recorder = StillFeedRecorder::default();
    recorder.note_present(1, ShownNow::NewLiveFrame(stamp(100)));
    recorder.note_present(2, ShownNow::RecomposedLive);
    recorder.note_present(3, ShownNow::NotLive);
    // После удержанной картинки перекомпоновка «живой» быть не может: живого кадра нет.
    recorder.note_present(4, ShownNow::RecomposedLive);
    let feed = recorder.take();
    let contents: Vec<ShownContent> = feed.presents.iter().map(|record| record.content).collect();
    assert_eq!(
        contents,
        vec![
            ShownContent::Live(stamp(100)),
            ShownContent::Live(stamp(100)),
            ShownContent::NotLive,
            ShownContent::NotLive,
        ]
    );
    assert_eq!(
        feed.presents.iter().map(|record| record.present_id).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
}

#[test]
fn take_empties_the_feed_and_restart_drops_older_presents() {
    let mut recorder = StillFeedRecorder::default();
    recorder.note_present(7, ShownNow::NewLiveFrame(stamp(5)));
    recorder.note_presents_restarted();
    recorder.note_present(0, ShownNow::RecomposedLive);
    let feed = recorder.take();
    assert!(feed.presents_restarted);
    // Старый показ 7 забыт, а перекомпоновка после рестарта без живого кадра — не живая.
    assert_eq!(
        feed.presents,
        vec![ShownRecord {
            present_id: 0,
            content: ShownContent::NotLive
        }]
    );
    assert!(recorder.take().is_empty());
}
