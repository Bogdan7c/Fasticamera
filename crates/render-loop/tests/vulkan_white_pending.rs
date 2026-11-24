//! Незакрытая заявка источнику белого переживает потерю surface и смену линзы (RADV, полная validation).
//!
//! Роль владельца белого (движка) играет `WhiteTestEngine`. Сквозная проверка идёт по картинке: после ответа
//! источника кадр побитово равен кадру эталонного прогона, у которого эта белая названа с самого начала.
//!
//! * Потеря surface при заявке `HideFrames`: новая сессия показа (новый раннер) спрашивает источник заново под
//!   ТЕМ ЖЕ номером, ответ один, скрытых кадров не больше предела на эту сессию.
//! * Смена линзы при заявке `HideFrames`: первый поток не ответил, движок переиздаёт заявку новым номером,
//!   запоздалый ответ на СТАРЫЙ номер наружу не выходит, а ответ по кадрам новой линзы даёт её эталон.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod awb_support;
mod support;
mod vulkan_support;
mod white_engine;
mod white_rig;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{WhileAwaitingWhite, WhiteRequestId, WhiteSourceId};
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{WhiteLinks, WhiteSourceRegistry};
use scene_estimation::white_source::{
    WhiteFrame, WhiteInputKind, WhiteReply, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts,
};
use white_engine::{SCRIPTED_ID, Script, WhiteTestEngine, held_setting, lamp_outcome, scripted_registry, setting_of};
use white_rig::{FrameImage, Rig, rig_or_skip, scene_set, second_lens_passport};

/// Дневной свет из конфига владельца: запасная белая до ответа источника.
const FALLBACK_KELVIN: f64 = 5600.0;
/// Всего кадров прогона: хватает на ответ и на плавный приём ответа (0,3 с ≈ 18 кадров).
const TOTAL_FRAMES: i64 = 50;
/// Кадры хвоста, где переход давно закончен и картинка обязана быть эталоном.
const SETTLED_TAIL: std::ops::Range<usize> = 45..50;

fn quiet(_: i64) -> CaptureColorHints {
    CaptureColorHints::default()
}

fn start(links: WhiteLinks, config: WhiteBalanceConfig) -> Option<(Rig, Arc<support::Journal>)> {
    rig_or_skip(scene_set(false), links, CaptureColorHints::default(), config)
}

/// Эталонный прогон: белая лампы названа с первого кадра, заявок нет; `second_lens` — с начала на второй линзе.
fn reference_images(kelvin: f64, second_lens: bool) -> Vec<FrameImage> {
    let (mut rig, journal) = start(held_setting(setting_of(kelvin)), WhiteBalanceConfig::default()).expect("GPU есть");
    if second_lens {
        rig.begin_stream(second_lens_passport());
    }
    let images = rig.drive(0..TOTAL_FRAMES, &quiet, &mut |_| {});
    rig.finish(&journal);
    images
}

fn assert_tail_equals(actual: &[FrameImage], expected: &[FrameImage], what: &str) {
    for index in SETTLED_TAIL {
        assert!(actual[index].is_some(), "{what}: кадр {index} показан");
        assert_eq!(actual[index], expected[index], "{what}: кадр {index} побитово эталон");
    }
}

#[test]
fn a_lost_surface_with_an_open_request_asks_the_source_again_under_the_same_number_and_hides_within_the_limit() {
    let limit = 6;
    let config = WhiteBalanceConfig {
        max_hidden_frames: limit,
        ..WhiteBalanceConfig::default()
    };
    // Источник отвечает лампой после 9 кадров потока: первая сессия показа (4 кадра) до ответа не доживает.
    let (registry, log) = scripted_registry(
        Script::AnswerAfter {
            observed_frames: 9,
            outcome: lamp_outcome(2800.0),
        },
        WhiteInputKind::ColorHints,
    );
    let links = WhiteLinks {
        sources: registry,
        ..WhiteLinks::default()
    };
    let mut engine = WhiteTestEngine::open(&links, FALLBACK_KELVIN, SCRIPTED_ID);
    let asked = engine.request_id().expect("заявка открытия");
    let Some((mut rig, journal)) = start(links, config) else {
        return;
    };
    let before = rig.drive(0..4, &quiet, &mut |_| engine.react());
    assert!(before.iter().all(Option::is_none), "до потери surface кадры скрыты");
    assert!(engine.answers_read.is_empty(), "ответа ещё нет");

    // Surface потеряна и вернулась: новая сессия показа, раннер новый, заявка движка всё ещё висит.
    rig.reattach_surface(&journal, 2);
    let after = rig.drive(4..TOTAL_FRAMES, &quiet, &mut |_| engine.react());
    let diagnostics = rig.viewfinder().white_balance;
    rig.finish(&journal);

    let log = log.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        log.requests,
        vec![asked, asked],
        "источник спрошен заново, и под тем же номером"
    );
    drop(log);
    assert_eq!(engine.answers_read.len(), 1, "ответ один");
    assert_eq!(engine.answers_read[0].request, asked);
    assert_eq!(engine.answers_accepted, 1, "движок принял его");
    assert_eq!(engine.current(), setting_of(2800.0));
    // Предел действует на каждую сессию показа: новая сессия спрятала ровно `limit`, потом показала принудительно.
    assert_eq!(diagnostics.hidden_frames_cold_start, u64::from(limit));
    assert_eq!(diagnostics.cold_start_forced, 1);
    assert_eq!(diagnostics.answers_published, 1);
    let first_shown = after.iter().position(Option::is_some).expect("новая сессия показывает");
    assert_eq!(first_shown, limit as usize, "в новой сессии скрыто не больше предела");
    let reference = reference_images(2800.0, false);
    let mut all = before;
    all.extend(after);
    assert_tail_equals(&all, &reference, "после ответа на новой сессии");
}

/// Источник с медленным исполнителем (роль нейросети): ответ по заявке даёт после `ANSWER_AFTER` кадров потока,
/// а заявку прежнего потока помнит и отвечает на неё ПОЗЖЕ, уже в новом потоке (отмену и сброс потока игнорирует).
struct SlowSource {
    state: Arc<Mutex<SlowState>>,
    /// Что ответить на действующую заявку.
    current_white: f64,
    /// Что ответить на заявку, оставшуюся от прежнего потока.
    stale_white: f64,
    current: Option<WhiteRequestId>,
    stale: Vec<WhiteRequestId>,
    observed_in_stream: u32,
    ready: VecDeque<WhiteReply>,
}

#[derive(Default)]
struct SlowState {
    requests: Vec<WhiteRequestId>,
    stale_replies_sent: u32,
}

/// Кадров потока до ответа на действующую заявку.
const ANSWER_AFTER: u32 = 4;
/// Кадров потока до запоздалого ответа на старую заявку.
const STALE_AFTER: u32 = 2;

impl SlowSource {
    fn reply(request: WhiteRequestId, kelvin: f64) -> WhiteReply {
        WhiteReply {
            request,
            measured_frame: Some(SensorTimestamp::from_nanos(0)),
            outcome: lamp_outcome(kelvin),
        }
    }
}

impl WhiteSource for SlowSource {
    fn id(&self) -> WhiteSourceId {
        SCRIPTED_ID
    }

    fn required_input(&self) -> WhiteInputKind {
        WhiteInputKind::ColorHints
    }

    fn begin_stream(&mut self, _stream: &WhiteStreamFacts<'_>) {
        // Новый поток: действующая заявка прежнего потока становится «старой», но источник её не забывает.
        self.stale.extend(self.current.take());
        self.observed_in_stream = 0;
    }

    fn observe(&mut self, _frame: &WhiteFrame<'_>) {
        self.observed_in_stream += 1;
        if self.observed_in_stream >= STALE_AFTER {
            for request in std::mem::take(&mut self.stale) {
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stale_replies_sent += 1;
                self.ready.push_back(Self::reply(request, self.stale_white));
            }
        }
        if self.observed_in_stream >= ANSWER_AFTER
            && let Some(request) = self.current.take()
        {
            self.ready.push_back(Self::reply(request, self.current_white));
        }
    }

    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .push(request);
        self.current = Some(request);
        Ok(())
    }

    fn cancel(&mut self, _request: WhiteRequestId) {
        // Медленный исполнитель отмену не слышит: старый ответ всё равно придёт.
    }

    fn poll(&mut self) -> Option<WhiteReply> {
        self.ready.pop_front()
    }
}

fn slow_registry(current_white: f64, stale_white: f64) -> (WhiteSourceRegistry, Arc<Mutex<SlowState>>) {
    let state = Arc::new(Mutex::new(SlowState::default()));
    let shared = Arc::clone(&state);
    let mut registry = WhiteSourceRegistry::new();
    registry.register(move || {
        Ok(Box::new(SlowSource {
            state: Arc::clone(&shared),
            current_white,
            stale_white,
            current: None,
            stale: Vec::new(),
            observed_in_stream: 0,
            ready: VecDeque::new(),
        }) as Box<dyn WhiteSource>)
    });
    (registry, state)
}

#[test]
fn a_lens_change_with_an_open_request_reissues_it_and_the_old_number_never_gets_published() {
    let limit = 10;
    let config = WhiteBalanceConfig {
        max_hidden_frames: limit,
        ..WhiteBalanceConfig::default()
    };
    // Действующая заявка получит лампу 2800 К, запоздалый ответ на старую — 6500 К (был бы виден на картинке).
    let (registry, state) = slow_registry(2800.0, 6500.0);
    let links = WhiteLinks {
        sources: registry,
        ..WhiteLinks::default()
    };
    let mut engine = WhiteTestEngine::open(&links, FALLBACK_KELVIN, SCRIPTED_ID);
    let first_request = engine.request_id().expect("заявка открытия");
    let Some((mut rig, journal)) = start(links, config) else {
        return;
    };
    // Первый поток: 3 кадра скрыты, источник не успел ответить (нужно ANSWER_AFTER = 4).
    let before = rig.drive(0..3, &quiet, &mut |_| engine.react());
    assert!(before.iter().all(Option::is_none), "первый поток: кадры скрыты");
    assert!(engine.answers_read.is_empty(), "первый поток не ответил");

    // Смена линзы: новый поток, другой профиль; движок переиздаёт заявку новым номером.
    rig.begin_stream(second_lens_passport());
    engine.ask(SCRIPTED_ID, WhileAwaitingWhite::HideFrames);
    let second_request = engine.request_id().expect("новая заявка");
    assert_ne!(first_request, second_request, "номер новый");
    let after = rig.drive(3..TOTAL_FRAMES, &quiet, &mut |_| engine.react());
    let diagnostics = rig.viewfinder().white_balance;
    rig.finish(&journal);

    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        state.requests,
        vec![first_request, second_request],
        "источнику ушла заявка нового номера, старая не подавалась заново"
    );
    assert_eq!(
        state.stale_replies_sent, 1,
        "запоздалый ответ на старый номер источник дал"
    );
    drop(state);
    assert_eq!(
        engine.answers_read.len(),
        1,
        "наружу вышел один ответ: {:?}",
        engine.answers_read
    );
    assert_eq!(engine.answers_read[0].request, second_request);
    assert_eq!(diagnostics.answers_published, 1, "старый номер не опубликован");
    assert_eq!(engine.answers_accepted, 1);
    assert_eq!(
        engine.current(),
        setting_of(2800.0),
        "белая — по действующей заявке, не по старой"
    );
    // Открытие одно, предел общий на оба потока: 3 кадра первого + ANSWER_AFTER второго, не больше предела.
    assert_eq!(diagnostics.hidden_frames_cold_start, 3 + u64::from(ANSWER_AFTER));
    assert!(diagnostics.hidden_frames_cold_start <= u64::from(limit));
    assert_eq!(diagnostics.cold_start_forced, 0);

    // Нейтральный предмет новой линзы с этой белой — эталон второй линзы.
    let reference = reference_images(2800.0, true);
    let mut all = before;
    all.extend(after);
    assert_tail_equals(&all, &reference, "вторая линза, ответ по её кадрам");
}
