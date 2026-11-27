//! Проход цветовых приборов поверх видоискателя (сессия 14h, решения 183, 189, 190).
//!
//! Что здесь живёт:
//! * [`ScopePass`] — владелец `ColorScopes` (объекты GPU приборов), запроса экрана, закэшированного правила
//!   прибора, прореживания «каждый N-й кадр» и учёта событий;
//! * [`CameraPipelines::record_scopes_for_new_frame`] — запись прибора нового показанного кадра в ТОТ ЖЕ командный
//!   буфер, что и сам кадр (решение 190), сразу после ISP и ДО компоновки;
//! * передача картинки прибора рисованию UI ([`ScopePass::picture_for_ui`]).
//!
//! # Порядок команд кадра с приборами
//! ISP пишет изображение (читатель — фрагмент) → барьер «запись compute → чтение compute» на этом же изображении
//! (`BackdropSource::record_read_barrier(Compute)`) → приборы (накопление, показ; картинка прибора готова к чтению
//! фрагментом) → компоновка изображения на экран → UI (читает картинку прибора как живую картинку).
//!
//! # Инварианты
//! 1. Приборы только читают изображение видоискателя: сам кадр и статистика AWB/AE не меняются.
//! 2. Закрытые приборы ничего не стоят: без запроса команд нет, а объекты GPU выводятся из обращения СРАЗУ и
//!    уничтожаются, как только GPU закончил все кадры слотов, которые их использовали (без ожидания на потоке рендера).
//! 3. Отказ приборов (создание, правило, запись) считается и логируется, кадр видоискателя всё равно показан.
//!    Устойчивый отказ сообщается ОДИН раз на смену запроса (или формата цели), а не на каждом кадре: дальше
//!    приборы «заблокированы» ([`Blockage`]), кадры считаются в `frames_skipped_blocked`, а диагностика показывает
//!    `ScopeResources::Failed`.
//! 4. Меряется только показанная картинка (решение 189): новый кадр, скрытый удержанием или чёрным, прибор не пишет.
//! 5. Внутри кадра объекты GPU не создаются (как у рендера UI, ревью сессии 11: пайплайны внутри кадра рвут vsync).
//!    Набор и картинка под размер запроса готовятся МЕЖДУ кадрами ([`ScopePass::prepare_between_frames`]: после
//!    команды, подключения, смены формата, конца кадра); запись использует только готовый набор, иначе кадр
//!    пропускается и считается в `frames_skipped_not_ready`.
//!
//! # Какие слоты держат объекты
//! Слот «занят» набором, если его командный буфер ссылается на объекты набора: записан проход (`PassRecorded`) или
//! картинка прибора отдана UI без записи прохода в этом слоте (перекомпоновка, `PictureHandedToUi`). Набор
//! уничтожается только когда все слоты освобождены (`collect_slot` после fence слота). Так объекты не уходят из-под
//! кадров в полёте, включая кадры перекомпоновки, которые только читают картинку.

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::gpu_timing::TimestampCollect;
use gpu_vulkan::source_window::SourceWindow;
use gpu_vulkan::storage_image::SampledImageView;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::image::DisplayGamut;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, PictureChange, ScopeError, ScopeFrame, ScopeInput};
use scope_contract::{ScopeLabelsBoard, ScopeRequest};
use scope_model::{ResolveError, ResolvedScope};

use super::scope_labels_publisher::ScopeLabelsPublisher;
use super::{CameraPipelines, FrameTarget, LOG_TARGET, NewIspFrame, ShowFrameAs};
use crate::config::ScopesConfig;
use crate::viewfinder::{ScopeEvents, ScopeResidency, ScopeResources};

/// Как слот кадра связан с набором объектов приборов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotUse {
    /// Слот набор не использует (или его fence уже пройден и слот собран).
    Idle,
    /// В слоте картинка прибора отдана рисованию UI, а проход приборов не записан (перекомпоновка).
    PictureHandedToUi,
    /// В слоте записан проход приборов.
    PassRecorded,
}

/// Набор объектов GPU приборов и то, что про него знает владелец.
struct ScopeSet {
    scopes: ColorScopes,
    /// Использование каждого слота кольца кадров.
    slots: Vec<SlotUse>,
    /// Картинка прибора последней УСПЕШНОЙ записи: её можно отдать UI (раскладка «чтение фрагментом»). `None` —
    /// записей ещё не было или последняя отказала (картинка могла оказаться в неизвестной раскладке).
    picture: Option<SampledImageView>,
}

impl ScopeSet {
    /// Читают ли объекты набора кадры, которые GPU ещё мог не закончить.
    fn is_in_flight(&self) -> bool {
        self.slots.iter().any(|slot| *slot != SlotUse::Idle)
    }

    /// Слот использует набор не слабее, чем `usage`: запись прохода сильнее передачи картинки UI.
    fn mark_slot(&mut self, slot: usize, usage: SlotUse) {
        if let Some(current) = self.slots.get_mut(slot)
            && *current != SlotUse::PassRecorded
        {
            *current = usage;
        }
    }
}

/// Правило прибора, решённое для запроса и охвата экрана.
struct ResolvedEntry {
    request: ScopeRequest,
    gamut: DisplayGamut,
    rule: ResolvedScope,
}

/// Что нужно, чтобы записать приборы нового показанного кадра.
pub(super) struct ScopeRecordInput<'a> {
    pub command_buffer: vk::CommandBuffer,
    pub frame_slot: usize,
    /// Изображение видоискателя, которое меряем.
    pub view: SampledImageView,
    /// Окно содержимого с зеркалом — то же, что показывает компоновщик.
    pub window: SourceWindow,
    /// Охват экрана изображения (нужен правилу OKLab-вектороскопа).
    pub gamut: DisplayGamut,
    /// Записывает барьер чтения compute на изображении видоискателя; вызывается ровно перед записью прохода.
    pub record_compute_read_barrier: &'a dyn Fn(vk::CommandBuffer),
}

/// Что про новый кадр решает запись приборов (см. [`CameraPipelines::record_scopes_for_frame`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct ScopedFrameFacts {
    /// Что реально ляжет на экран: мерится только живое изображение (решение 189).
    pub show: ShowFrameAs,
    /// Номер сабмита кадра (для подложки изображения, из которой берётся барьер чтения).
    pub submission: gpu_vulkan::backdrop::SubmissionSerial,
    /// Зеркало изображения этого кадра при показе.
    pub mirror: gpu_vulkan::source_window::HorizontalMirror,
}

/// Почему приборы заблокированы до смены запроса или формата цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blockage {
    /// Правило прибора не решилось (запрос против настроек и охвата экрана).
    RuleUnresolved,
    /// Набор объектов GPU или картинка не созданы.
    CreationFailed,
    /// Запись прохода отказала ошибкой GPU: набор испорчен и выведен.
    RecordingFailed,
}

/// Владелец объектов приборов (см. описание модуля).
pub(super) struct ScopePass {
    context: GpuContext,
    config: ScopesConfig,
    frame_slots: usize,
    /// Что просит экран прямо сейчас (`None` — приборы закрыты).
    demand: Option<ScopeRequest>,
    /// Действующий набор объектов (`None` — не создан или выведен).
    live: Option<ScopeSet>,
    /// Выведенные наборы, которые ещё читают кадры в полёте.
    retiring: Vec<ScopeSet>,
    resolved: Option<ResolvedEntry>,
    /// Почему приборы не работают при этом запросе. Пока запрос и формат цели не изменились, не пытаемся снова на
    /// каждом кадре (сбой памяти, шейдеров и правила сам не лечится), как у рендера UI.
    blocked: Option<Blockage>,
    /// Причина последнего отказа, о котором уже сообщено (лог и счётчик): тот же отказ подряд не повторяется.
    reported_failure: Option<String>,
    /// Сколько новых показанных кадров прошло с открытия приборов или смены запроса: по нему считается «каждый N-й
    /// кадр».
    new_frames_since_open: u64,
    /// Писатель доски подписей прибора для приложения (решения 326, 327); `None` — доски нет (сборки без приложения,
    /// тесты), подписи никому не нужны.
    labels: Option<ScopeLabelsPublisher>,
    events: ScopeEvents,
}

impl ScopePass {
    /// Закрытые приборы: объекты GPU не создаются.
    pub(super) fn new(context: &GpuContext, frame_slots: usize, config: &ScopesConfig) -> Self {
        Self {
            context: context.clone(),
            config: config.clone(),
            frame_slots,
            demand: None,
            live: None,
            retiring: Vec::new(),
            resolved: None,
            blocked: None,
            reported_failure: None,
            new_frames_since_open: 0,
            labels: None,
            events: ScopeEvents::default(),
        }
    }

    /// Подключает доску подписей прибора: с этого момента подписи публикуются на смену решённого правила и
    /// снимаются при закрытии приборов.
    pub(super) fn attach_labels_board(&mut self, board: ScopeLabelsBoard) {
        self.labels = Some(ScopeLabelsPublisher::new(board));
    }

    /// Что просят приборы сейчас.
    pub(super) fn demand(&self) -> Option<ScopeRequest> {
        self.demand
    }

    /// Новое желание экрана; вызывается на КАЖДОМ кадре, какое бы содержимое он ни показывал. Пропажа запроса сразу
    /// выводит набор из обращения; смена типа, осей или размера набор НЕ пересоздаёт (пайплайны строятся один раз,
    /// размер картинки меняет внутри себя `ColorScopes`).
    pub(super) fn set_demand(&mut self, demand: Option<ScopeRequest>) {
        if demand == self.demand {
            return;
        }
        if demand.is_none() {
            self.retire_live();
            self.resolved = None;
            if let Some(labels) = self.labels.as_mut() {
                labels.withdraw();
            }
        }
        // Открытие приборов ИЛИ смена запроса (страница, оси, зум): счёт «каждый N-й кадр» начинается заново, и
        // ближайший кадр считается всегда. Иначе при N > 1 новый запрос мог попасть в пропускаемую фазу, и под новым
        // именем страницы кадр-другой стояла бы картинка прежнего прибора.
        self.new_frames_since_open = 0;
        // Новый запрос — новая попытка: прежний отказ к нему не относится, о новом сообщим заново.
        self.blocked = None;
        self.reported_failure = None;
        self.demand = demand;
    }

    /// Новый кадр с запросом не попал на экран (удержание, скрытие, чёрный): прибор его не меряет.
    pub(super) fn note_frame_not_shown(&mut self) {
        if self.demand.is_some() {
            self.events.counters.frames_skipped_not_shown += 1;
        }
    }

    /// Готовит набор объектов GPU и картинку под размер запроса `demand` МЕЖДУ кадрами (см. инвариант 5). Запрос
    /// передаётся ЯВНО и первым делом становится действующим ([`Self::set_demand`]): строить объекты по закэшированному
    /// запросу нельзя, он мог устареть (источник UI ушёл, запрос отозван), а закрытые приборы ничего не стоят. Нет
    /// запроса — набор выводится, и ничего не создаётся. Ничего не делает, если приборы заблокированы или всё уже
    /// готово. Отказ создания блокирует приборы до смены запроса и сообщается один раз.
    pub(super) fn prepare_between_frames(&mut self, demand: Option<ScopeRequest>) {
        self.set_demand(demand);
        let Some(request) = demand else {
            return;
        };
        if self.blocked.is_some() {
            return;
        }
        if let Err(error) = self.ensure_live() {
            self.block(
                Blockage::CreationFailed,
                FailureKind::Record,
                &format!("объекты приборов не созданы: {error}"),
            );
            return;
        }
        let Some(set) = self.live.as_mut() else {
            return;
        };
        match set.scopes.prepare_picture(request.image_size) {
            // Прежняя картинка (другого размера) остаётся показанной UI, пока первая запись не заменит её новой.
            Ok(PictureChange::Unchanged | PictureChange::Created | PictureChange::Replaced) => {}
            Err(error) => {
                self.retire_live();
                self.block(
                    Blockage::CreationFailed,
                    FailureKind::Record,
                    &format!("картинка прибора не создана: {error}"),
                );
            }
        }
    }

    /// Новый кадр с запросом не получил входа приборов (нет изображения видоискателя или подложки): прибор его не
    /// меряет, и это видно в диагностике.
    pub(super) fn note_frame_without_input(&mut self) {
        if self.demand.is_some() {
            self.events.counters.frames_skipped_no_input += 1;
        }
    }

    /// Записывает приборы нового показанного кадра (см. описание модуля). Никогда не возвращает ошибку наружу и
    /// никогда не создаёт объектов GPU: набор и картинка готовятся [`Self::prepare_between_frames`].
    pub(super) fn record(&mut self, input: &ScopeRecordInput<'_>) {
        let Some(request) = self.demand else {
            return;
        };
        if self.blocked.is_some() {
            self.events.counters.frames_skipped_blocked += 1;
            return;
        }
        let every_nth = u64::from(self.config.every_nth_frame.max(1));
        let due = self.new_frames_since_open.is_multiple_of(every_nth);
        self.new_frames_since_open += 1;
        if !due {
            self.events.counters.frames_skipped_by_cadence += 1;
            return;
        }
        if let Err(error) = self.resolve(&request, input.gamut) {
            self.block(
                Blockage::RuleUnresolved,
                FailureKind::Resolve,
                &format!("правило прибора: {error}"),
            );
            return;
        }
        let ready_set = self
            .live
            .as_mut()
            .filter(|set| set.scopes.is_picture_ready_for(request.image_size));
        let (Some(entry), Some(set)) = (self.resolved.as_ref(), ready_set) else {
            // Набор или картинка нужного размера ещё не подготовлены между кадрами: кадр без прибора, не создание в кадре.
            self.events.counters.frames_skipped_not_ready += 1;
            return;
        };
        (input.record_compute_read_barrier)(input.command_buffer);
        let recorded = set.scopes.record(
            input.command_buffer,
            &ScopeFrame {
                input: ScopeInput {
                    view: input.view,
                    window: input.window,
                },
                rule: &entry.rule,
                settings: &self.config.model,
                picture_size: request.image_size,
                frame_slot: input.frame_slot,
            },
        );
        match recorded {
            Ok(recorded) => {
                set.picture = Some(recorded.output);
                set.mark_slot(input.frame_slot, SlotUse::PassRecorded);
                self.events.counters.passes_recorded += 1;
            }
            Err(ScopeError::Gpu(error)) => {
                // Ошибка GPU возможна уже после начала записи команд (контракт `ColorScopes`): слот занят, картинка
                // могла оказаться в неизвестной раскладке, набор считается испорченным и выводится. Повтор на каждом
                // кадре бессмыслен: приборы блокируются до смены запроса.
                set.picture = None;
                set.mark_slot(input.frame_slot, SlotUse::PassRecorded);
                self.retire_live();
                self.block(
                    Blockage::RecordingFailed,
                    FailureKind::Record,
                    &format!("проход приборов не записан: GPU: {error}"),
                );
            }
            Err(error) => {
                // Отказ проверки ДО первой команды (контракт `ColorScopes`): набор, его слежение за раскладкой и
                // прежняя картинка целы, UI продолжает показывать её. Повтор той же причины подряд не сообщается.
                self.report_failure(&format!("проход приборов не записан: {error}"), FailureKind::Record);
            }
        }
    }

    /// Картинка прибора для рисования UI на этом кадре: есть только у действующего набора с успешной записью.
    pub(super) fn picture_for_ui(&self) -> Option<SampledImageView> {
        self.demand?;
        self.live.as_ref()?.picture
    }

    /// Картинка прибора записана в команды слота как живая картинка UI: до fence слота набор уничтожать нельзя.
    pub(super) fn note_picture_handed_to_ui(&mut self, frame_slot: usize) {
        if let Some(set) = self.live.as_mut() {
            set.mark_slot(frame_slot, SlotUse::PictureHandedToUi);
            self.events.counters.picture_handoffs += 1;
        }
    }

    /// Fence слота пройден: освободить слот у всех наборов, собрать время GPU и уничтожить выведенные наборы, которые
    /// больше никто не читает. Никогда не ждёт GPU.
    pub(super) fn collect_slot(&mut self, frame_slot: usize) {
        for set in self.live.iter_mut().chain(self.retiring.iter_mut()) {
            let usage = set.slots.get(frame_slot).copied().unwrap_or(SlotUse::Idle);
            if usage == SlotUse::Idle {
                continue;
            }
            match set.scopes.release_slot_after_fence(frame_slot) {
                Ok(report) => {
                    if let TimestampCollect::Ready(spans) = report.gpu_time {
                        self.events.gpu_times_ns.push(spans.total_ns);
                    }
                }
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, frame_slot, "слот приборов не освобождён");
                    self.events.counters.record_failures += 1;
                    self.events.counters.last_failure = Some(format!("слот приборов не освобождён: {error}"));
                }
            }
            set.slots[frame_slot] = SlotUse::Idle;
        }
        let before = self.retiring.len();
        self.retiring.retain(ScopeSet::is_in_flight);
        self.events.counters.resources_released += (before - self.retiring.len()) as u64;
    }

    /// Записанный кадр слота не отправлен: его проход приборов не исполнится, и слежение `ColorScopes` за раскладкой
    /// картинки неверно. Набор выводится (следующая запись создаст новый), а слот освободит обычный `collect_slot`.
    pub(super) fn frame_not_submitted(&mut self, frame_slot: usize) {
        let recorded_here = self
            .live
            .as_ref()
            .is_some_and(|set| set.slots.get(frame_slot) == Some(&SlotUse::PassRecorded));
        if recorded_here {
            self.retire_live();
        }
    }

    /// Счётчики последней записи действующего набора, прочитанные с GPU с ожиданием (тестовый шов). `Ok(None)` —
    /// набора нет или записей в нём ещё не было.
    #[cfg(feature = "test-support")]
    pub(super) fn read_counters_blocking(&self) -> Result<Option<scope_model::ScopeCounters>, ScopeError> {
        match &self.live {
            Some(set) if set.picture.is_some() => set.scopes.read_back_counters_blocking().map(Some),
            _ => Ok(None),
        }
    }

    /// Картинка прибора последней записи (тестовый шов, как [`Self::read_counters_blocking`]).
    #[cfg(feature = "test-support")]
    pub(super) fn read_picture_blocking(&self) -> Result<Option<isp_vulkan::scopes::PictureReadback>, ScopeError> {
        match &self.live {
            Some(set) if set.picture.is_some() => set.scopes.read_back_picture_blocking().map(Some),
            _ => Ok(None),
        }
    }

    /// Новый формат цели: вызывающий уже дождался GPU, поэтому всё освобождается сразу. Закэшированный запрос
    /// остаётся, но набор по нему здесь не поднимается: его поднимет [`Self::prepare_between_frames`] со СВЕЖИМ
    /// запросом владельца настроек, которого вызывающий зовёт сразу после пересоздания.
    pub(super) fn reset_for_format(&mut self) {
        let released = self.retiring.len() + usize::from(self.live.is_some());
        self.retiring.clear();
        self.live = None;
        // Новый формат цели: прежний отказ мог быть из-за формата (охват экрана), попытка и сообщение — заново.
        self.resolved = None;
        self.blocked = None;
        self.reported_failure = None;
        // Счёт «каждый N-й кадр» начинается заново: первый кадр после пересоздания цели считается всегда (иначе
        // кадры сразу после смены формата попадали бы в прежнюю фазу прореживания, и прибор молчал бы до N кадров).
        self.new_frames_since_open = 0;
        self.events.counters.resources_released += released as u64;
    }

    /// Приращения с прошлого вызова (их копит статистика видоискателя: цепочка пересоздаётся, счётчики живут дольше).
    pub(super) fn take_events(&mut self) -> ScopeEvents {
        std::mem::take(&mut self.events)
    }

    /// Живое состояние объектов для диагностики.
    pub(super) fn residency(&self) -> ScopeResidency {
        // Блокировка важнее остального: объекты могут ещё жить (правило не решилось), но прибор не работает.
        let resources = match (&self.blocked, &self.live, self.retiring.is_empty()) {
            (Some(_), _, _) => ScopeResources::Failed,
            (None, Some(_), _) => ScopeResources::Live,
            (None, None, false) => ScopeResources::Retiring,
            (None, None, true) => ScopeResources::Absent,
        };
        ScopeResidency {
            resources,
            accumulator_pipelines: self
                .live
                .as_ref()
                .map_or(0, |set| set.scopes.accumulator_pipeline_count()),
            retiring_sets: self.retiring.len(),
        }
    }

    /// Выводит действующий набор: уничтожается сразу, если его никто не читает, иначе — когда слоты освободятся.
    fn retire_live(&mut self) {
        let Some(set) = self.live.take() else {
            return;
        };
        if set.is_in_flight() {
            self.retiring.push(set);
        } else {
            self.events.counters.resources_released += 1;
        }
    }

    /// Правило прибора для запроса и охвата (повторное решение только при смене любого из них).
    fn resolve(&mut self, request: &ScopeRequest, gamut: DisplayGamut) -> Result<(), ResolveError> {
        let fresh = self
            .resolved
            .as_ref()
            .is_some_and(|entry| entry.request == *request && entry.gamut == gamut);
        if !fresh {
            let rule = ResolvedScope::resolve(request, &self.config.model, gamut)?;
            // Правило решено заново (запрос или охват другие): положения подписей могли измениться.
            if let Some(labels) = self.labels.as_mut() {
                labels.publish_for(request, &rule, &self.config.model.render);
            }
            self.resolved = Some(ResolvedEntry {
                request: *request,
                gamut,
                rule,
            });
        }
        Ok(())
    }

    /// Гарантирует, что набор объектов создан. Создаёт ВСЕ пайплайны сразу: смена типа и осей на ходу новых не строит.
    fn ensure_live(&mut self) -> Result<(), ScopeError> {
        if self.live.is_some() {
            return Ok(());
        }
        let scopes = ColorScopes::new(
            &self.context,
            &ColorScopesConfig {
                frame_slots: self.frame_slots,
                counter_sizing: self.config.model.clone(),
            },
        )?;
        self.live = Some(ScopeSet {
            scopes,
            slots: vec![SlotUse::Idle; self.frame_slots],
            picture: None,
        });
        self.events.counters.resources_created += 1;
        Ok(())
    }

    /// Блокирует приборы до смены запроса или формата цели и сообщает причину (один раз).
    fn block(&mut self, blockage: Blockage, kind: FailureKind, reason: &str) {
        self.blocked = Some(blockage);
        self.report_failure(reason, kind);
    }

    /// Сообщает об отказе: лог, счётчик и `last_failure`. Та же причина подряд (после последней смены запроса или
    /// формата) повторно не сообщается, а считается пропущенным кадром.
    fn report_failure(&mut self, reason: &str, kind: FailureKind) {
        if self.reported_failure.as_deref() == Some(reason) {
            self.events.counters.frames_skipped_blocked += 1;
            return;
        }
        tracing::warn!(target: LOG_TARGET, reason, "приборы кадра не записаны: видоискатель показан без обновления прибора");
        match kind {
            FailureKind::Resolve => self.events.counters.resolve_failures += 1,
            FailureKind::Record => self.events.counters.record_failures += 1,
        }
        self.events.counters.last_failure = Some(reason.to_owned());
        self.reported_failure = Some(reason.to_owned());
    }
}

/// Какой счётчик отказов растёт.
#[derive(Debug, Clone, Copy)]
enum FailureKind {
    Resolve,
    Record,
}

/// Охват экрана для правила прибора по пространству цвета изображения видоискателя.
fn gamut_of(color_space: DisplayColorSpace) -> DisplayGamut {
    match color_space {
        DisplayColorSpace::Srgb => DisplayGamut::Srgb,
        DisplayColorSpace::DisplayP3 => DisplayGamut::DisplayP3,
    }
}

impl CameraPipelines {
    /// Желание экрана о приборах на этом кадре (любое содержимое кадра). Приборы закрылись — набор выводится сразу.
    pub fn set_scope_demand(&mut self, demand: Option<ScopeRequest>) {
        self.scopes.set_demand(demand);
    }

    /// Делает `demand` действующим желанием о приборах и готовит объекты под него МЕЖДУ кадрами (создание набора и
    /// картинки не должно происходить внутри кадра). Желание вычисляет ВЛАДЕЛЕЦ настроек и источника UI
    /// (`ViewfinderSettings::effective_scope_demand`) и передаёт сюда каждый раз: закэшированному запросу доверять
    /// нельзя. Зовёт владелец цепочки после команды, подключения, смены формата, ухода источника UI и конца кадра.
    pub fn prepare_scopes_between_frames(&mut self, demand: Option<ScopeRequest>) {
        self.scopes.prepare_between_frames(demand);
    }

    /// Подключает доску подписей прибора (решения 326, 327): пишет рендер, читает приложение для экрана. Зовёт
    /// владелец цепочки сразу после создания конвейеров.
    pub fn attach_scope_labels_board(&mut self, board: ScopeLabelsBoard) {
        self.scopes.attach_labels_board(board);
    }

    /// Факт об устройстве для диагностики приборов: управление округлением и денормалями f32.
    pub fn scope_float32_controls(&self) -> gpu_vulkan::context::Float32Controls {
        self.context.capabilities().float32_controls
    }

    /// Приращения приборов с прошлого вызова (для статистики видоискателя, переживающей цепочку показа).
    pub fn take_scope_events(&mut self) -> ScopeEvents {
        self.scopes.take_events()
    }

    /// Живое состояние объектов приборов (диагностика).
    pub fn scope_residency(&self) -> ScopeResidency {
        self.scopes.residency()
    }

    /// Счётчики прибора последней записи (тестовый шов, фича `test-support`).
    #[cfg(feature = "test-support")]
    pub fn read_scope_counters(&self) -> Result<Option<scope_model::ScopeCounters>, ScopeError> {
        self.scopes.read_counters_blocking()
    }

    /// Картинка прибора последней записи (тестовый шов, фича `test-support`).
    #[cfg(feature = "test-support")]
    pub fn read_scope_picture(&self) -> Result<Option<isp_vulkan::scopes::PictureReadback>, ScopeError> {
        self.scopes.read_picture_blocking()
    }

    /// Записывает приборы НОВОГО кадра сразу после ISP (между записью ISP и компоновкой). Только для кадра, который
    /// реально лёг на экран живым и чей ISP записан: решение 189 — меряется показанная картинка.
    pub(super) fn record_scopes_for_new_frame(&mut self, frame: &FrameTarget, input: &NewIspFrame<'_>) {
        self.record_scopes_for_frame(
            frame,
            &ScopedFrameFacts {
                show: input.show,
                submission: input.submission,
                mirror: input.mirror,
            },
        );
    }

    /// То же, что [`Self::record_scopes_for_new_frame`], но по тому, что решает запись приборов (показ, номер сабмита,
    /// зеркало): решение «мерить или нет и что считать пропуском» не зависит от остальных полей кадра ISP.
    pub(super) fn record_scopes_for_frame(&mut self, frame: &FrameTarget, facts: &ScopedFrameFacts) {
        if self.scopes.demand().is_none() {
            return;
        }
        if !matches!(facts.show, ShowFrameAs::Live) {
            self.scopes.note_frame_not_shown();
            return;
        }
        let Some(image) = self.isp.image() else {
            self.scopes.note_frame_without_input();
            return;
        };
        // Серийный номер подложки нужен только для её проверки готовности; барьер им не пользуется.
        let Some(backdrop) = self.isp.backdrop_source(facts.submission) else {
            self.scopes.note_frame_without_input();
            return;
        };
        self.scopes.record(&ScopeRecordInput {
            command_buffer: frame.command_buffer,
            frame_slot: frame.ring_slot,
            view: image.sampled_view(),
            // То же окно, что у компоновщика: содержимое кадра и зеркало этого кадра.
            window: image.content_window().with_mirror(facts.mirror),
            gamut: gamut_of(image.color_space()),
            record_compute_read_barrier: &|command_buffer| {
                backdrop.record_read_barrier(
                    command_buffer,
                    gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage::Compute,
                );
            },
        });
    }
}

#[cfg(test)]
mod tests;
