//! Подставной «рендер белой» для тестов приложения: отвечает на заявки источнику, как отвечал бы поток рендера
//! с источником «телефон», но без GPU. Настоящий путь (рендер + источник) проверяет сквозной тест
//! `tests/white_end_to_end.rs`; этот стенд нужен сценариям, которым важно лишь, чтобы заявка закрылась.

use camera_contract::auto_exposure::CameraTag;
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{SceneIlluminant, WhiteAnswer, WhiteConfidence, WhiteRequestId, WhiteSourceOutcome};
use camera_core::CameraHandle;

use crate::camera_test_support::FakeCameraRig;

/// Свет, который подставной «телефон» называет в ответе: лампа накаливания 3200 К.
pub fn lamp_light() -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3_200.0).expect("3200 К — допустимая температура"),
        tint: Tint::NEUTRAL,
    }
}

/// Отвечатель на заявки белого: на каждую новую заявку из ящика указаний кладёт в ящик ответов «свет такой».
pub struct FakeRenderWhite {
    /// Ручка движка; `None` — движка нет (камера недоступна): отвечать некому и не на что.
    engine: Option<CameraHandle>,
    directives: camera_contract::three_a::DirectiveSlot<camera_contract::white::WhiteDirective>,
    answers: camera_contract::white::WhiteAnswerChannel,
    light: WhiteBalanceSetting,
    answered: Option<WhiteRequestId>,
}

impl FakeRenderWhite {
    /// Стенд на ящиках риг-движка; `light` — свет, который «источник» называет в каждом ответе.
    pub fn new(rig: &FakeCameraRig, light: WhiteBalanceSetting) -> Self {
        Self {
            engine: rig.services.running().ok().map(|running| running.handle().clone()),
            directives: rig.white_directives.clone(),
            answers: rig.white_answers.clone(),
            light,
            answered: None,
        }
    }

    /// Отвечает на действующую заявку, если на неё ещё не отвечали. Вызывается из потока подставного рендера.
    pub fn answer_pending(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        let Some((_, directive)) = self.directives.latest() else {
            return;
        };
        let Some(request) = directive.request else {
            return;
        };
        if self.answered == Some(request.id) {
            return;
        }
        // Камера кадра нужна ответу как факт диагностики; без настроенной сессии отвечать нечем.
        let Some(camera) = engine
            .status()
            .configured
            .as_ref()
            .map(|configured| configured.stream.camera.clone())
        else {
            return;
        };
        self.answered = Some(request.id);
        self.answers.publish(WhiteAnswer {
            request: request.id,
            camera: CameraTag::of(&camera),
            measured_frame: None,
            outcome: WhiteSourceOutcome::Ready {
                illuminant: SceneIlluminant::Uniform(self.light),
                confidence: WhiteConfidence::FULL,
            },
        });
    }
}
