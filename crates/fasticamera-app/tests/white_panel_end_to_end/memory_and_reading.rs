//! Сценарии i и j: перезапуск процесса (память белого в настоящем файле) и показание источника на кнопке «Phone».

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use camera_contract::white::WhiteOrigin;
use camera_contract::white_reading::{WhiteReading, WhiteSourceReading};
use camera_screen::ScreenConfig;
use camera_screen::white_display::phone_estimate_text;
use fasticamera::app_directory::AppDirectory;
use fasticamera::white_memory::{WhiteMemoryConfig, WhiteMemoryHandoff, open_white_memory};

use super::fake_source::Reaction;
use super::panel_view::{caption, colors, strip_number};
use super::session::{PHONE, WhiteSession, lamp_after, opening_setting, white_of};

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

/// Новая пустая папка приложения во временном каталоге системы.
fn app_directory() -> (AppDirectory, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "fasticamera-white-panel-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("папка создана");
    (
        AppDirectory::new(path.to_string_lossy().into_owned()).expect("папка годится"),
        path,
    )
}

fn session_with_memory(directory: &AppDirectory, reaction: Reaction) -> Option<WhiteSession> {
    let memory = WhiteMemoryHandoff::holding(open_white_memory(directory, &WhiteMemoryConfig::default()));
    WhiteSession::open_with(reaction, memory)
}

/// i. Протянули шкалу, «процесс» умер, новый стенд с тем же файлом показывает то же число (жёлтое), кадр = эталон.
#[test]
fn a_scale_value_survives_a_process_restart_and_the_new_screen_shows_the_same_yellow_number() {
    let (directory, path) = app_directory();
    let (_, yellow) = colors();

    // Первый «процесс»: открытие берёт лампу у источника, человек тянет шкалу Кельвинов.
    let held = {
        let Some(mut session) = session_with_memory(&directory, lamp_after(2, 2_800.0)) else {
            return;
        };
        session.open_card();
        session.drag_ruler(-2.0, 40);
        let held = session.wait_origin("шкала поставила ручное белое", WhiteOrigin::Manual);
        WhiteSession::wait_until("ручное белое записано на диск", || {
            std::fs::read_to_string(path.join("white.json")).is_ok_and(|text| text.contains("manual"))
        });
        session.stand.finish();
        held
    };
    assert!(held.setting.temperature.kelvin() > 2_800.0);

    // Второй «процесс»: источник теперь назвал бы другое (7000 К), но ручное держится и источника не спрашивают.
    let Some(mut session) = session_with_memory(&directory, lamp_after(0, 7_000.0)) else {
        return;
    };
    assert_eq!(
        session.held(),
        held,
        "новый движок держит то же белое с тем же происхождением"
    );
    session.frames(4);
    let number = strip_number(session.stand.ui_list(), held.setting.temperature.kelvin())
        .expect("новый экран показывает то же число");
    assert_eq!(number.color, yellow, "число жёлтое: белое ручное");
    assert!(
        session.source.requests().is_empty(),
        "ручное держится: телефон не спрашивали"
    );
    session.assert_picture_is(held.setting, "кадр после перезапуска");
    session.stand.finish();
    std::fs::remove_dir_all(path).expect("уборка");
}

/// j. Показание на кнопке «Phone»: «≈ N» от доски; поток закрыт — число исчезло; само показание белое и кадр не меняет.
#[test]
fn the_phone_reading_shows_as_an_estimate_vanishes_with_the_stream_and_never_changes_the_white_or_the_frame() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.open_card();
    let held = session.held();
    let estimate = phone_estimate_text(
        Some(WhiteReading::Measured {
            white: white_of(3_100.0, 0.0),
        }),
        &ScreenConfig::default().white,
    )
    .expect("оценка для измеренного показания");
    assert!(estimate.starts_with('\u{2248}'), "знак «≈»: {estimate}");
    assert!(
        caption(session.stand.ui_list(), &estimate).is_empty(),
        "до показания оценки нет"
    );

    session.readings.publish(WhiteSourceReading {
        source: PHONE,
        reading: WhiteReading::Measured {
            white: white_of(3_100.0, 0.0),
        },
    });
    session.frames(6);
    assert_eq!(
        caption(session.stand.ui_list(), &estimate).len(),
        1,
        "на кнопке Phone: {estimate}"
    );
    assert_eq!(session.held(), held, "показание белое не меняет");
    assert_eq!(
        session.source.requests().len(),
        1,
        "и источника не спрашивает (только заявка открытия)"
    );
    session.assert_picture_is(opening_setting(), "показание кадр не меняет");

    // Показание не измерено — оценки нет.
    session.readings.publish(WhiteSourceReading {
        source: PHONE,
        reading: WhiteReading::NotMeasuredYet,
    });
    session.frames(6);
    assert!(
        caption(session.stand.ui_list(), &estimate).is_empty(),
        "не измерено — строки нет"
    );

    // Снова измерено, потом поток закрыт: прежнее число показывать нельзя.
    session.readings.publish(WhiteSourceReading {
        source: PHONE,
        reading: WhiteReading::Measured {
            white: white_of(3_100.0, 0.0),
        },
    });
    session.frames(6);
    assert_eq!(caption(session.stand.ui_list(), &estimate).len(), 1);
    session.stand.pause();
    session.stand.ui_frames(10);
    assert!(
        caption(session.stand.ui_list(), &estimate).is_empty(),
        "поток закрыт — оценка исчезла"
    );
    session.stand.finish();
}
