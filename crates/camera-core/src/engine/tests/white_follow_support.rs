//! Стенд слежения за белым: движок, ящики рендера и кадры с метками сенсора 60 к/с (по ним слежение считает темп).

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::three_a::RequestOrigin;
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId, WhiteSourceOutcome, WhiteSourceRequest};

use super::three_a_support::{center, result_meta};
use super::white_support::*;
use super::*;
use crate::white::{
    RememberedOrigin, WhiteFollowRecord, WhiteFollowState, WhiteKeeping, WhiteMemoryLoad, WhiteMemoryRecord,
    WhiteResumeRule,
};

/// Период опроса по умолчанию (100 мс) в кадрах 60 к/с.
pub(super) const ASK_EVERY_FRAMES: usize = 6;
pub(super) const PHONE: WhiteSourceId = WhiteSourceId::PHONE;

/// Стенд слежения: движок, ящики рендера и счётчик кадров.
pub(super) struct FollowRig {
    pub(super) rig: Rig,
    pub(super) outside: WhiteOutside,
    pub(super) next_frame: i64,
}

impl FollowRig {
    /// Идёт поток, открытие получило ответ 4000 К; слежение выключено, событий в очереди нет.
    pub(super) fn streaming() -> Self {
        let mut follow = Self::over(white_rig());
        follow.rig.send(CameraCommand::EnterForeground);
        follow.rig.start_streaming_on_wide();
        follow.frames(1);
        follow.answer(setting(4_000.0, 0.0));
        assert_eq!(
            follow.rig.held(),
            phone(setting(4_000.0, 0.0)),
            "открытие взяло белое телефона"
        );
        follow.rig.fresh_events();
        follow
    }

    /// То же, но «Auto» уже включён.
    pub(super) fn following() -> Self {
        let mut follow = Self::streaming();
        assert_eq!(follow.follow(), CommandOutcome::Accepted);
        follow.rig.fresh_events();
        follow
    }

    /// Стенд в фоне, где память помнит белую и выбор слежения.
    pub(super) fn remembering(rule: WhiteResumeRule, remembered: WhiteMemoryLoad, follow: WhiteFollowRecord) -> Self {
        let mut config = CameraEngineConfig::default();
        config.three_a.white.resume_rule = rule;
        let (mut white, outside) = white_wiring(remembered);
        white.remembered_follow = follow;
        let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
        let mut rig = Rig::over_fake_with_wiring(config, AppPresence::Background, FakeBackendControl::new(), wiring);
        rig.engine.load_catalog(rig.now);
        rig.pump();
        Self::over((rig, outside))
    }

    pub(super) fn over((rig, outside): (Rig, WhiteOutside)) -> Self {
        Self {
            rig,
            outside,
            next_frame: 1,
        }
    }

    pub(super) fn follow(&mut self) -> CommandOutcome {
        self.rig.send(CameraCommand::FollowWhite { source: PHONE })
    }

    /// `count` результатов повторяющегося запроса; часы движка идут вместе с кадрами.
    pub(super) fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.rig.now += Duration::from_nanos(16_666_667);
            self.rig
                .feed_result(result_meta(self.next_frame, RequestOrigin::Repeating, None));
            self.next_frame += 1;
        }
    }

    pub(super) fn request(&self) -> Option<WhiteSourceRequest> {
        self.outside.directive().request
    }

    /// Источник ответил «вот такой свет» на заявку, которую видит рендер.
    pub(super) fn answer(&mut self, white: WhiteBalanceSetting) {
        let id = self.request().expect("рендер видит заявку").id.0;
        self.rig.deliver_answer(&self.outside, ready(id, white));
    }

    pub(super) fn refuse(&mut self, outcome: WhiteSourceOutcome) {
        let id = self.request().expect("рендер видит заявку").id.0;
        self.rig.deliver_answer(&self.outside, answer(id, outcome));
    }

    /// Один круг слежения: дождаться вопроса и ответить.
    pub(super) fn round(&mut self, white: WhiteBalanceSetting) {
        self.frames(ASK_EVERY_FRAMES);
        self.answer(white);
    }

    /// Вопрос по темпу и отказ источника.
    pub(super) fn refuse_next(&mut self) {
        self.frames(ASK_EVERY_FRAMES);
        self.refuse(WhiteSourceOutcome::Busy);
    }

    pub(super) fn lock(&mut self) {
        assert_eq!(
            self.rig.send(CameraCommand::LockFocusAndExposureAt { point: center() }),
            CommandOutcome::Accepted
        );
    }

    pub(super) fn unlock(&mut self) {
        assert_eq!(
            self.rig.send(CameraCommand::ResetPointOfInterest),
            CommandOutcome::Accepted
        );
    }

    pub(super) fn state(&self) -> WhiteFollowState {
        self.rig.white_status().follow.state
    }

    pub(super) fn asked(&self) -> u64 {
        self.rig.white_status().follow.asked
    }
}

pub(super) fn phone(setting: WhiteBalanceSetting) -> HeldWhite {
    HeldWhite {
        setting,
        origin: WhiteOrigin::Source(PHONE),
    }
}

pub(super) const FOLLOWING: WhiteFollowState = WhiteFollowState::Following { source: PHONE };
pub(super) const HELD_BY_LOCK: WhiteFollowState = WhiteFollowState::HeldByLock { source: PHONE };

pub(super) fn remembered_source(kelvin: f64) -> WhiteMemoryLoad {
    WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
        setting: setting(kelvin, 0.0),
        origin: RememberedOrigin::Source,
    })
}

pub(super) fn followed() -> WhiteFollowRecord {
    WhiteFollowRecord {
        keeping: WhiteKeeping::FollowSource,
        speed: None,
    }
}

pub(super) fn follow_changes(events: &[CameraEvent]) -> Vec<WhiteKeeping> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::WhiteFollowChanged { follow } => Some(follow.keeping),
            _ => None,
        })
        .collect()
}
