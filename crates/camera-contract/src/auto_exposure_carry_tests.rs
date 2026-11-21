use super::*;
use crate::fixtures::camera_id;

#[test]
fn tag_is_stable_per_camera_and_differs_between_cameras() {
    let main = camera_id("0");
    assert_eq!(CameraTag::of(&main), CameraTag::of(&camera_id("0")));
    // Реальные id телефона: все пары различимы.
    let ids = [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "20", "21", "22",
    ];
    for (index, first) in ids.iter().enumerate() {
        for second in &ids[index + 1..] {
            assert_ne!(
                CameraTag::of(&camera_id(first)),
                CameraTag::of(&camera_id(second)),
                "камеры {first} и {second} должны различаться"
            );
        }
    }
}

#[test]
fn carried_exposure_applies_only_to_its_own_camera() {
    let target = ExposureProduct::from_seconds_iso(2.0).unwrap();
    let carried = CarriedSceneExposure::for_camera(&camera_id("2"), target);
    assert!(carried.is_for(&camera_id("2")));
    assert!(!carried.is_for(&camera_id("0")));
    assert_eq!(carried.target, target);
}
