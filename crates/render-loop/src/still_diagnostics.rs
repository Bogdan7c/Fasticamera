//! Счётчики снимков для `BackendDiagnostics` / `RenderStatus` (рядом с диагностикой видоискателя).
//!
//! Правило сессии 14: «снимок без задержки» доказывается числами. Поэтому выбор разведён на точный и заменённый
//! (с причинами), а каждый итог — по видам. Поля накопительные за жизнь бэкенда, кроме `queue_len`.

use serde::Serialize;

/// Среднее и наибольшее по наблюдениям в миллисекундах (дробные, без смешивания часов).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct DurationStat {
    /// Сколько наблюдений.
    pub count: u64,
    /// Сумма, мс.
    pub total_ms: f64,
    /// Наибольшее, мс.
    pub max_ms: f64,
}

impl DurationStat {
    /// Добавляет наблюдение.
    pub fn observe(&mut self, millis: f64) {
        self.count += 1;
        self.total_ms += millis;
        self.max_ms = self.max_ms.max(millis);
    }

    /// Среднее, мс (`None` — наблюдений нет).
    pub fn mean_ms(&self) -> Option<f64> {
        (self.count > 0).then(|| self.total_ms / self.count as f64)
    }
}

/// Счётчики выбора кадра.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PickCounters {
    /// Точное попадание: ровно тот кадр, что был на экране (показ доказан записями драйвера).
    pub exact: u64,
    /// Замена по оценке: записей показа не дождались или доказать нельзя.
    pub substituted_timing_unknown: u64,
    /// Замена: показанный кадр уже вытеснен из кольца.
    pub substituted_evicted: u64,
    /// Замена: цели нет в кольце, хотя она не старше старейшего кадра (потерян до кольца).
    pub substituted_lost_before_ring: u64,
    /// Замена: у показанного кадра нет метаданных.
    pub substituted_without_metadata: u64,
    /// Замена: показанный кадр — не обычный кадр потока.
    pub substituted_not_regular: u64,
    /// Кадра нет: кольцо выключено.
    pub no_frame_ring_disabled: u64,
    /// Кадра нет: кольцо пусто.
    pub no_frame_ring_empty: u64,
    /// Кадра нет: в кольце есть кадры, но годных нет.
    pub no_frame_no_eligible: u64,
    /// Кадра нет: на экране в момент нажатия не живой кадр камеры (удержание смены объектива, чёрный).
    pub no_frame_not_live_on_screen: u64,
    /// Кадра нет: рендер не показывает камеру (нет surface или показов).
    pub no_frame_nothing_shown: u64,
}

/// Счётчики доказательства «что было на экране».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PresentationCounters {
    /// Не дождались записей показа за `presentation_wait_ms`.
    pub wait_expired: u64,
    /// Дыра между записями драйвера у момента нажатия.
    pub unprovable_gap: u64,
    /// Все известные записи позже момента нажатия.
    pub unprovable_press_before_timings: u64,
    /// Содержимое показа забыто журналом.
    pub unprovable_content_forgotten: u64,
}

/// Счётчики параметров кадра.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ParamsCounters {
    /// Взята запись самого кадра.
    pub own: u64,
    /// Своей записи не было: взята запись ближайшего кадра того же потока.
    pub nearest_of_same_stream: u64,
    /// Записей потока нет вовсе: снимок отпущен без захвата.
    pub missing: u64,
    /// Параметры ISP не посчитались.
    pub unresolved: u64,
}

/// Счётчики отказов до захвата (`NotSecured`), которым нет точного двойника в `NoFrameReason`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RefusalCounters {
    /// Очередь снимков полна.
    pub queue_full: u64,
    /// У кадра нет метаданных результата.
    pub facts_no_capture_details: u64,
    /// Камеры кадра нет в каталоге зума плана.
    pub facts_camera_not_in_catalog: u64,
    /// Кадрирование не построилось.
    pub facts_crop: u64,
    /// Цвет кадра (сенсор, нейтраль, карта виньетирования) не построился.
    pub facts_color: u64,
    /// GPU снимков не создался.
    pub gpu_unavailable: u64,
    /// Копия RAW не запустилась.
    pub capture_start_failed: u64,
    /// Копия RAW сорвалась на GPU.
    pub capture_failed: u64,
    /// Копия готова, но сырые отсчёты прочитать нельзя (`NotSecured(CaptureFailed)`: `Secured` не отправлялся).
    pub raw_samples_unreadable: u64,
    /// `Begin` пришёл, когда surface не присоединена: снимку не на чем жить (`NotSecured(RendererStopped)`).
    pub begin_without_surface: u64,
}

/// Счётчики итогов (ровно один на принятый снимок).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OutcomeCounters {
    pub not_secured: u64,
    pub rendered: u64,
    pub readback: u64,
    pub failed_params_unavailable: u64,
    pub failed_gpu: u64,
    pub failed_target_unavailable: u64,
    pub failed_target_timed_out: u64,
    pub failed_abandoned: u64,
}

impl OutcomeCounters {
    /// Сколько итогов выдано всего.
    pub fn total(&self) -> u64 {
        self.not_secured
            + self.rendered
            + self.readback
            + self.failed_params_unavailable
            + self.failed_gpu
            + self.failed_target_unavailable
            + self.failed_target_timed_out
            + self.failed_abandoned
    }
}

/// CPU потока рендера на шаги движка снимков, мс (часы потока рендера; наблюдение — один вызов шага).
///
/// Шаги движка идут на потоке рендера сразу после показа кадра видоискателя: всё, что шаг держит поток,
/// следующий кадр видоискателя ждёт. Наибольшее значение здесь — прямая цена снимка для видоискателя
/// (14c: в серии видоискатель терял ≈ 40 кадров, а «кадр захвачен» опаздывал на ≈ 110 мс на снимок).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct StillStepCpu {
    /// Весь шаг движка за один кадр (сумма шагов ниже и мелочь между ними).
    pub advance: DurationStat,
    /// Выбор кадра: записи показа, кольцо.
    pub choosing: DurationStat,
    /// Опрос готовности копий RAW: возврат буфера камере, подготовка буфера следующей копии.
    pub capture_poll: DurationStat,
    /// Запуск копий RAW: импорт буфера камеры, отправка.
    pub capture_start: DurationStat,
    /// Проявление: запуск (картинка снимка), полосы, передача готовой картинки.
    pub develop: DurationStat,
    /// Показы в кодер: запуск ожидающих и опрос идущих.
    pub present: DurationStat,
    /// Из `develop`: только запуск проявления (`develop_start` — картинка, параметры, дескрипторы).
    pub develop_begin: DurationStat,
    /// Из `develop`/`present`: только запуск показа в кодер (`present_start` — поверхность, swapchain,
    /// получение картинки, blit, показ).
    pub encoder_present_start: DurationStat,
}

/// Счётчики снимков со вспышкой (14d): каждый исход и каждый обход проверки — отдельно, молчаливых путей нет.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FlashStillCounters {
    /// Принятые `Begin` со вспышкой.
    pub begins: u64,
    /// Кадр запроса со вспышкой найден в кольце и пошёл на копию.
    pub frames_found: u64,
    /// Кадр не дошёл до кольца в срок `flash_frame_wait_ms` (`FlashFrameNeverArrived`).
    pub never_arrived: u64,
    /// Движок камеры сообщил, что вспышка не снята, пока снимок ждал кадр (`FlashNotCaptured`).
    pub not_captured_by_camera: u64,
    /// Сообщение «вспышка не снята» пришло для снимка, который кадр уже не ждёт (опоздало или без вспышки): проигнорировано.
    pub late_failure_reports: u64,
    /// У рендера нет плана кадра со вспышкой для снимка (`FrameUnusable`).
    pub no_plan: u64,
    /// У кадра нет спаренных выдержки и ISO: подтяжку считать не из чего (`FrameUnusable`).
    pub no_exposure_product: u64,
    /// Белая для проявления взята из оценки кадра с фонариком.
    pub white_from_torch_frame: u64,
    /// Оценки по кадру с фонариком нет: взята текущая белая видоискателя (причина — `FlashWhiteFallback`).
    pub white_fallback_no_torch_estimate: u64,
    /// Проверка освещённости пройдена: кадр освещён.
    pub lit_check_passed: u64,
    /// Проверка: кадр не освещён (`FlashFrameNotLit`).
    pub not_lit: u64,
    /// Проверка не решаема: предсказанная доля вспышки слишком мала, кадр принят.
    pub lit_check_skipped_low_predicted_share: u64,
    /// Проверка не решаема: нет уровня света сцены того же потока, кадр принят.
    pub lit_check_skipped_no_ambient_level: u64,
    /// Проверка не решаема: отсчёты RAW или уровни кадра прочитать нельзя, кадр принят.
    pub lit_check_skipped_samples_unavailable: u64,
}

/// Диагностика снимков.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StillDiagnostics {
    /// Принятые команды `Begin`.
    pub begins: u64,
    /// Выбор кадра.
    pub picks: PickCounters,
    /// Доказательство «что было на экране».
    pub presentation: PresentationCounters,
    /// Параметры кадра.
    pub params: ParamsCounters,
    /// Отказы до захвата.
    pub refusals: RefusalCounters,
    /// Итоги.
    pub outcomes: OutcomeCounters,
    /// Снимки со вспышкой.
    pub flash: FlashStillCounters,
    /// Копии RAW: запущено / готово.
    pub captures_started: u64,
    pub captures_completed: u64,
    /// Проявления: начато / закончено.
    pub developments_started: u64,
    pub developments_completed: u64,
    /// Полос проявления, отправленных за всё время.
    pub slices_submitted: u64,
    /// Показы в кодер: начато / закончено.
    pub encoder_presents_started: u64,
    pub encoder_presents_completed: u64,
    /// Цели кодера, пришедшие не вовремя или для неизвестного снимка.
    pub targets_unexpected: u64,
    /// Снимки, брошенные при отсоединении, остановке или сбое: до захвата / после.
    pub abandoned_before_capture: u64,
    pub abandoned_after_capture: u64,
    /// Снимки, которые пережили отсоединение surface и доводились без экрана (копия RAW в полёте или уже
    /// захвачены; решение 146). Считается в момент отсоединения, итог каждого — в `outcomes`.
    pub continued_without_surface: u64,
    /// GPU снимков освобождён без surface, когда очередь опустела (память фонового процесса не держится).
    pub gpu_released_without_surface: u64,
    /// GPU снимков создан / не создался / оставлен в утечку при остановке (GPU не освободился).
    pub gpu_created: u64,
    pub gpu_leaked_on_abandon: u64,
    /// Сколько раз GPU снимков объявлен потерянным (fence не опросился или не просигналил в срок) и оставлен в
    /// учтённой утечке; следующий снимок создаёт GPU заново.
    pub gpu_lost: u64,
    /// Из них по сроку (`capture_timeout_ms` / `develop_timeout_ms` / `encoder_present_timeout_ms`).
    pub gpu_lost_by_deadline: u64,
    /// Миниатюры (14g): отправлены приёмнику (строго до итога снимка) / не вышли (причина — в логе
    /// `fasticamera::render::still`). Снимок без миниатюры остаётся удачным: итог от неё не зависит.
    pub thumbnails_delivered: u64,
    pub thumbnails_unavailable: u64,
    /// Сколько раз GPU заранее (до нажатия) создать не удалось.
    pub gpu_prewarm_failures: u64,
    /// Снимков в очереди сейчас.
    pub queue_len: usize,
    /// Наибольшая длина очереди за всё время.
    pub queue_high_water: usize,
    /// Сколько снимок стоял на экране до нажатия: `нажатие − первый показ` кадра (обе метки `MONOTONIC`).
    /// Только точный путь: на оценке момент показа неизвестен.
    pub exact_press_after_shown: DurationStat,
    /// От запуска проявления до готовой картинки (стенные часы потока рендера).
    pub develop_wall: DurationStat,
    /// CPU, потраченный потоком рендера на один шаг проявления (`develop_advance`): цена снимка для кадра.
    pub develop_step_cpu: DurationStat,
    /// CPU потока рендера по шагам движка снимков.
    pub step_cpu: StillStepCpu,
}
