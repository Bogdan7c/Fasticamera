//! Счастливые пути службы записи снимков через настоящий рабочий поток: от допуска и `Secured`
//! до опубликованных файлов и фазы `Saved`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod service_support;

use service_support::{DraftState, EncoderEvent, FakeFormats, FakeJpeg, FixedCameras, FixedClock, Rig, readback_for};
use std::time::Duration;
use still_contract::develop::{DevelopEvent, PrimaryTarget, PrimaryTargetOffer};
use still_contract::phase::{SavedStill, StillPhase};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

#[test]
fn heif_with_dng_is_saved_after_the_encoder_finishes() {
    let rig = Rig::start();
    rig.admit(1);

    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);
    // Рендер получил поверхность кодера, а не память.
    let PrimaryTargetOffer::Ready { id, target } = offer else {
        panic!("ждали готовую цель");
    };
    assert_eq!(id, StillId(1));
    assert!(matches!(target, PrimaryTarget::EncoderSurface(_)), "{target:?}");

    // Кодер открыт с размером, поворотом, качеством и Exif этого кадра.
    let opens = rig.heif.opens();
    assert_eq!(opens.len(), 1);
    let (settings, target_uri) = &opens[0];
    assert_eq!((settings.width, settings.height), (8, 6));
    assert_eq!(settings.rotation_degrees, 90);
    assert_eq!(settings.quality, 95);
    assert_eq!(
        settings.exif_tiff.as_deref(),
        Some(FakeFormats::exif_for(8, 6, FixedCameras::MODEL).as_slice())
    );
    assert_eq!(target_uri, &format!("content://fake/{}.HEIC", FixedClock::STEM));

    // DNG записан в черновик, пока рендер ещё проявляет; ничего не опубликовано.
    service_support::wait_until("DNG записан в черновик", || {
        rig.store
            .log
            .by_extension("DNG")
            .is_some_and(|dng| dng.bytes == FakeFormats::DNG_BYTES)
    });
    assert_eq!(rig.store.log.published_count(), 0);
    assert_eq!(rig.handle.stats().pending_now, 1);

    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    let last = rig.wait_final(1);

    assert_eq!(
        last,
        StillPhase::Saved {
            id: StillId(1),
            files: SavedStill {
                primary: format!("content://fake/{}.HEIC", FixedClock::STEM),
                raw: Some(format!("content://fake/{}.DNG", FixedClock::STEM)),
            },
        }
    );
    assert_eq!(
        rig.phases.of(StillId(1)).first(),
        Some(&StillPhase::Secured { id: StillId(1) }),
        "Secured раньше Saved"
    );
    assert_eq!(rig.store.log.published_count(), 2);
    assert_eq!(
        rig.heif.events(),
        vec![EncoderEvent::Finished(Duration::from_millis(1_000))]
    );

    // Метаданные хранилища: каталог, MIME, время съёмки.
    let heic = rig.store.log.by_extension("HEIC").unwrap();
    assert_eq!(heic.request.mime_type, "image/heic");
    assert_eq!(heic.request.relative_path, "DCIM/Camera");
    assert_eq!(heic.request.date_taken_millis, FixedClock::UTC_MILLIS);
    assert_eq!(
        rig.store.log.by_extension("DNG").unwrap().request.mime_type,
        "image/x-adobe-dng"
    );
    assert_eq!(rig.formats.dng_models(), vec![FixedCameras::MODEL.to_owned()]);

    // Слот допуска освобождён к моменту фазы, счётчики сходятся.
    let stats = rig.handle.stats();
    assert_eq!((stats.accepted, stats.saved, stats.pending_now), (1, 1, 0));
    assert_eq!(stats.discarded_drafts, 0);
}

#[test]
fn heif_without_raw_creates_one_draft_and_no_dng() {
    let rig = Rig::start();
    rig.admit(2);
    let offer = rig.secure(2, PrimaryFormat::Heif, RawOutput::Skip);
    assert!(matches!(offer, PrimaryTargetOffer::Ready { .. }));
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(2) });

    let StillPhase::Saved { files, .. } = rig.wait_final(2) else {
        panic!("ждали Saved");
    };
    assert_eq!(files.raw, None);
    assert_eq!(rig.store.log.drafts().len(), 1);
    assert!(rig.formats.dng_models().is_empty());
}

#[test]
fn jpeg_readback_is_compressed_given_exif_and_saved() {
    let rig = Rig::start();
    rig.admit(3);

    let offer = rig.secure(3, PrimaryFormat::Jpeg, RawOutput::Skip);
    let PrimaryTargetOffer::Ready { target, .. } = offer else {
        panic!("ждали готовую цель");
    };
    assert!(matches!(target, PrimaryTarget::Readback), "{target:?}");
    assert!(rig.heif.opens().is_empty(), "кодер HEIF для JPEG не открывается");

    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(3),
        image: readback_for(0x5A),
    });
    let StillPhase::Saved { files, .. } = rig.wait_final(3) else {
        panic!("ждали Saved");
    };
    assert_eq!(files.primary, format!("content://fake/{}.JPG", FixedClock::STEM));

    // В сжатие ушли именно байты картинки рендера, с размером и качеством из конфига.
    let calls = rig.jpeg.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].width, calls[0].height, calls[0].quality), (8, 6, 95));
    assert_eq!(calls[0].rgba, vec![0x5A; 8 * 6 * 4]);

    // В файле: JPEG платформы с вставленным Exif сразу после SOI.
    let mut expected = vec![0xFF, 0xD8];
    expected.extend_from_slice(b"APP1[");
    expected.extend_from_slice(&FakeFormats::exif_for(8, 6, FixedCameras::MODEL));
    expected.push(b']');
    expected.extend_from_slice(&FakeJpeg::BODY[2..]);
    let file = rig.store.log.by_extension("JPG").unwrap();
    assert_eq!(file.bytes, expected);
    assert_eq!(file.state, DraftState::Published);
    assert_eq!(file.request.mime_type, "image/jpeg");
}

#[test]
fn jpeg_with_dng_publishes_both_files() {
    let rig = Rig::start();
    rig.admit(4);
    rig.secure(4, PrimaryFormat::Jpeg, RawOutput::Dng);
    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(4),
        image: readback_for(1),
    });

    let StillPhase::Saved { files, .. } = rig.wait_final(4) else {
        panic!("ждали Saved");
    };
    assert_eq!(files.raw, Some(format!("content://fake/{}.DNG", FixedClock::STEM)));
    assert_eq!(rig.store.log.by_extension("DNG").unwrap().bytes, FakeFormats::DNG_BYTES);
    assert_eq!(rig.store.log.published_count(), 2);
}

#[test]
fn secured_goes_out_on_the_callers_thread_and_the_result_on_the_service_thread() {
    let rig = Rig::start();
    rig.admit(5);
    rig.secure(5, PrimaryFormat::Heif, RawOutput::Skip);
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(5) });
    rig.wait_final(5);

    let names = rig.phases.thread_names();
    assert_eq!(names.len(), 2, "Secured и итог: {names:?}");
    // `Secured` не ждёт рабочего потока: его шлёт тот, кто передал событие (здесь — поток теста).
    assert_ne!(names[0].as_deref(), Some("fasticamera-still"), "{names:?}");
    // Итог (и всё платформенное) — только на потоке службы.
    assert_eq!(names[1].as_deref(), Some("fasticamera-still"), "{names:?}");
}

/// Настоящие DNG/Exif/JPEG-Exif: тело `write_dng` приходит с пакетом форматов.
#[test]
fn real_formats_write_a_valid_dng_and_the_still_is_saved() {
    use photo_output::service::{PhotoFileFormats, StillServiceDeps};
    // Служба с НАСТОЯЩИМИ форматами (`crate::dng`, `crate::exif`): DNG в хранилище — настоящий TIFF с отсчётами
    // кадра, снимок доходит до `Saved`. Ломается, если служба пишет не то или форматы отвергают факты кадра.
    let mut rig = Rig::start();
    let deps = StillServiceDeps {
        store: rig.store.clone(),
        heif: rig.heif.clone(),
        jpeg: rig.jpeg.clone(),
        formats: std::sync::Arc::new(PhotoFileFormats),
        clock: std::sync::Arc::new(FixedClock),
        cameras: rig.cameras.clone(),
        phases: rig.phases.clone(),
        targets: rig.targets.clone(),
    };
    rig.service = photo_output::service::StillService::start(service_support::fast_config(), deps).unwrap();
    rig.handle = rig.service.handle();
    rig.admit(6);
    rig.secure(6, PrimaryFormat::Heif, RawOutput::Dng);
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(6) });
    let last = rig.wait_final(6);
    assert!(
        matches!(last, StillPhase::Saved { ref files, .. } if files.raw.is_some()),
        "{last:?}"
    );

    let dng = rig.store.log.by_extension("DNG").unwrap();
    assert!(
        dng.bytes.starts_with(b"II*\0"),
        "DNG начинается с заголовка TIFF little-endian"
    );
    // Отсчёты кадра лежат в файле как есть (2 байта на отсчёт, без копий и преобразований).
    let raw = service_support::raw_for();
    let samples = raw.le_bytes();
    assert!(
        dng.bytes.windows(samples.len()).any(|window| window == samples),
        "сырые отсчёты кадра есть в DNG байт-в-байт"
    );
    assert_eq!(dng.state, DraftState::Published);
}

#[test]
fn heif_keeps_rotation_in_the_container_and_jpeg_in_exif() {
    use photo_output::exif::RotationCarrier;
    let rig = Rig::start();
    rig.admit(10);
    rig.secure(10, PrimaryFormat::Heif, RawOutput::Skip);
    rig.admit(11);
    rig.secure(11, PrimaryFormat::Jpeg, RawOutput::Skip);
    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(11),
        image: readback_for(1),
    });
    rig.wait_final(11);

    // Первый запрос блока Exif — HEIF (поворот несёт irot), второй — JPEG (поворот несёт тег).
    assert_eq!(
        rig.formats.exif_carriers(),
        vec![RotationCarrier::ContainerTransform, RotationCarrier::ExifOrientationTag]
    );
    assert_eq!(rig.heif.opens()[0].0.rotation_degrees, 90);
}

#[test]
fn heif_is_still_saved_without_exif_when_the_exif_block_cannot_be_built() {
    let rig = Rig::start();
    rig.formats.fail_exif();
    rig.admit(12);
    rig.secure(12, PrimaryFormat::Heif, RawOutput::Skip);
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(12) });

    assert!(matches!(rig.wait_final(12), StillPhase::Saved { .. }));
    assert_eq!(rig.heif.opens()[0].0.exif_tiff, None, "кодер открыт без Exif");
    let stats = rig.handle.stats();
    assert_eq!((stats.saved, stats.failed_encode, stats.exif_build_failures), (1, 0, 1));
}

#[test]
fn jpeg_is_still_saved_without_app1_when_the_exif_block_cannot_be_built() {
    let rig = Rig::start();
    rig.formats.fail_exif();
    rig.admit(13);
    rig.secure(13, PrimaryFormat::Jpeg, RawOutput::Skip);
    rig.send(DevelopEvent::PrimaryReadback {
        id: StillId(13),
        image: readback_for(2),
    });

    assert!(matches!(rig.wait_final(13), StillPhase::Saved { .. }));
    assert_eq!(
        rig.store.log.by_extension("JPG").unwrap().bytes,
        FakeJpeg::BODY.to_vec(),
        "JPEG записан как отдала платформа, без APP1"
    );
    assert_eq!(rig.handle.stats().exif_build_failures, 1);
}
