#!/usr/bin/env bash
# Smoke-сценарий на телефоне: запуск по intent-параметру и проверка строки результата.
#
#   scripts/device-smoke.sh [--variant debug|release] [--no-build] [--timeout SEC] [--artifacts-dir DIR]
#                           [--stages LIST] [--surface PLACE] [--ab-window-ms MS] [--ab-pairs N] [--endurance-ms MS]
#                           [--unchanged-frame skip|always] [--exposure-controller own|vendor] [--neutral-card] [--keep-captures] [--stop-app-after]
#                           [--saturation-workloads LIST] [--saturation-measure-ms MS] [--saturation-warmup-ms MS]
#                           <сценарий>
#
# Сценарии (--timeout в скобках): hello, panic (debug), lifecycle, permission (60), motion, platform,
# render (120), render-lifecycle (400), ahb-import и gpu-fence (60); камера: camera-catalog (30, сверка с
# dumpsys), camera-stream (120), camera-evict (debug, 90; сам запускает системную камеру); поток 05:
# raw-stream (120), raw-stream-endurance (420), raw-stream-cpu-probe (debug, 60).
# Видоискатель 07: viewfinder (120; ISP за минуту: к/с, GPU, AWB, задержка, скриншоты; --neutral-card —
# ещё и «серая карта в центре вышла серой», телефон наведён на карту); viewfinder-latency (240; ABBA
# двух профилей baseline / tuned: 6 окон × 20 с = 120 с + запуск); viewfinder-stage-budget (~6 мин на строку, по умолчанию 9 строк = 3600;
# --stages levels,tone,whole_viewfinder — подмножество; --ab-window-ms 15000 — короткие окна для мс GPU;
# только на батарее, без кабеля); viewfinder-endurance
# (720; 10 минут, троттлинг); video-headroom-bench (720; стенд V4 16:9@60, 120 Гц — скрипт касается экрана).
# Цена работ GPU током 16a — viewfinder-stage-budget на НАСТОЯЩЕМ экране камеры. --surface bare_viewfinder (по умолчанию: экран
# снят, как раньше) | photo_screen (экран остаётся, режим PHOTO) | calibrate_screen (экран остаётся, драйвер свайпом открывает
# CALIBRATE: живой видоискатель + цветовой прибор на каждом кадре + стекло UI; в конце свайп обратно в PHOTO). Новые строки
# --stages (имена через запятую вместе со старыми): color_scopes (только calibrate_screen: приборы вкл/выкл), ui_overlay
# (photo_screen, calibrate_screen: рисование UI вкл/выкл), whole_product (photo_screen, calibrate_screen: продукт против «камеры
# без нашей обработки и UI», даёт базу camera_without_product_gpu), display_120hz (все места: 120 Гц с удерживаемым касанием
# против 60 Гц; драйвер держит палец ФОНОВОЙ командой input на всё окно A, окно проверяет реальную частоту показа). Недопустимое
# сочетание строки и места — отказ Rust при initialize ДО замера. --ab-pairs N — число пар A/B на строку (по умолчанию 3,
# ≈ 6 окон по 60 с). Примеры: --surface photo_screen --stages ui_overlay,whole_product --timeout 1500 viewfinder-stage-budget;
# --surface calibrate_screen --stages color_scopes,ui_overlay,whole_product,display_120hz --timeout 2400 viewfinder-stage-budget.
# --unchanged-frame skip|always — политика пропуска тиков без изменений (решение 214, сессия 16a): skip — тик без нового кадра
# камеры и без изменений UI не рисуется и не показывается; always — рисуется каждый тик (умолчание продукта до замера тока A/B).
# Нет флага — extra не передаётся, Rust берёт умолчание; другое значение — отказ скрипта.
# --stop-app-after — после получения результата сделать am force-stop нашего пакета (иначе приложение остаётся на экране и греет
# телефон между замерами; по умолчанию выключено). Внешнее питание отвергает замер, как и раньше.
# Стенд насыщения 16a: gpu-saturation-bench (по умолчанию 14 работ × ~16 с + паузы ≈ 5 мин, --timeout 600; камера на время замера
# закрыта, экран снят, GPU делит только стенд; таблица «работа → к/с → мс/кадр → вердикт» в результате, проходы, шкала по шагам,
# тепло и состав сцены — строки FASTICAMERA_DETAIL, начало каждой работы — стадия workload_started для совмещения с журналом тепла).
# --saturation-workloads LIST — имена работ через запятую (viewfinder, scopes_waveform, scopes_parade, scopes_vectorscope_ycbcr,
# scopes_vectorscope_oklab, scopes_parade_gray, scopes_vectorscope_gray, compositor, display_photo, display_photo_recompose,
# display_calibrate, display_calibrate_recompose, headroom_v4, still; ИЛИ имя группы: baseline — эти 14 работ (умолчание),
# ui_ablation — 7 абляций прохода UI (display_{calibrate,photo}_{glass_only,no_glass,...}), isp_ablation — 14 абляций видоискателя
# (viewfinder_awb_never, viewfinder_awb_every_frame, viewfinder_without_<стадия>, viewfinder_minimal); неизвестное имя — отказ Rust при initialize;
# дайджест выхода каждой работы — в FASTICAMERA_DETAIL, «база → новое»: python3 scripts/smoke_checks_saturation.py compare A.txt B.txt),
# --saturation-measure-ms MS и --saturation-warmup-ms MS — окно замера и прогрев каждой работы (умолчание 10000 и 3000).
# Пример: --variant release --timeout 300 --saturation-workloads viewfinder,compositor --saturation-measure-ms 5000 gpu-saturation-bench.
# 3A 08: 3a (150; тап в центр и 4 угла, ручная выдержка 1/1000, EV, блокировка AE/AF, брекетинг 4 кадра, AWB;
# скриншоты на стадии awb_ready, --neutral-card — серая карта; для вопроса о регионах фокуса навести на сцену
# с ближним и дальним планом). 3A 08c: ae-trace (150; наш AE по ступенькам света: фото 36 с, видео 36 с, статика 20 с;
# телефон показывает на экране отсчёт «приготовься… 3, 2, 1» и «ЩЁЛКНИ ЛАМПОЙ» — владелец щёлкает лампой; сводка + ряд FASTICAMERA_DETAIL; нужен own AE).
# Стекло 11: glass (90; стекло поверх живого видоискателя: запуск + окно 30 с анимации — GPU-время кадра UI в бюджете,
# 0 пропусков кадров, скриншоты на стадии glass_live; гравитация — заметка, не провал; гоняется и в debug, и в release).
# Экран камеры 12: screen (180; экран iOS поверх видоискателя, скрипт делает касания: тапы фокуса, долгий тап — блокировка AE/AF,
# свайпы меню и шкалы EV, горизонтальные свайпы, клавиша громкости; на каждой стадии screen_gesture — один жест по adb; шаги Rust
# проверяет по следствиям на экране и в камере, GPU UI в бюджете; нужен РАЗБЛОКИРОВАННЫЙ телефон в портрете, камеру не закрывать).
# Зум 13: zoom (240; экран камеры остаётся на месте, сценарий сам кладёт в него тапы по кнопкам .6/1/2/1/.6, переворот на фронталку и
# назад и протяжку по ленте (колесо, вибрации); по каждому шагу — время «действие → смена объектива» и «действие → первый живой кадр
# новой камеры», удержанные кадры, частота, зеркало фронтальной; чёрных кадров там, где ждали картинку, должно быть 0; скрипт
# ничего не нажимает, телефон разблокирован, экран не трогать; цифры по шагам — строки FASTICAMERA_DETAIL).
# Тёплая камера 13b: standby (120; сценарий сам кладёт намерения объектива в ящик движка: греет соседа, обменивает роли туда и
# обратно повышением тёплой без переоткрытия, затем «не нужно»; по каждому шагу — время до готовности тёплой и до смены объектива,
# частота обеих задних камер (порог 55 к/с), счётчики потока кадров (кадры тёплой, leaked_leases = 0), отказов тёплой 0, после
# отпуска ни одной тёплой; скрипт ничего не нажимает, телефон разблокирован, экран не трогать).
# Снимок 14: capture (150; снимок на настоящем экране без скрипта-нажимателя: один HEIF, HEIF + DNG, вспышка On (14d: кадр по своему
# запросу, освещён по пикселям, вспышка сработает), вспышка Auto (наблюдение: сработает ли — зависит от света сцены) и серия из 10 нажатий по 100 мс; по каждому
# снимку — время «нажатие → кадр захвачен → сохранено», адреса файлов; сервис снимка обязан быть НАСТОЯЩИМ (запасной фейк — провал).
# После результата скрипт ПК проверяет каждый файл (MediaStore: is_pending 0, DCIM/Camera/, MIME, размеры; heif-info, DNG, exiftool/rawpy,
# системный ImageDecoder в debug-сборке) и УДАЛЯЕТ с телефона только файлы этого прогона; --keep-captures оставляет их владельцу.
# Телефон разблокирован, экран не трогать).
# Шаги на телефоне — scripts/smoke_driver.py, ожидания — smoke_wait.py, smoke_checks*.py.
# raw-stream*, viewfinder, glass и 3a снимают два скриншота на стадии viewfinder_live (3a — awb_ready, glass — glass_live) в каталог артефактов ВНЕ
# репозитория: --artifacts-dir DIR либо свежий `mktemp -d` (путь печатается).
# --endurance-ms MS — длина viewfinder-endurance на этот запуск (camera.scenarios.power.endurance.duration_ms;
# 180000 — короткий замер тока для A/B «наш AE / вендорский»; не короче шага снимков 60 с — проверит Rust).
# --exposure-controller own|vendor — какая автоэкспозиция на этот запуск (camera.exposure_controller; без флага —
# умолчание конфига, own); годится для любого сценария, допустимость значения проверяет Rust при initialize.
# Validation-слой есть только в debug-сборке: в release отчёт пишет validation.mode=disabled.
# Подробности отчёта — строки FASTICAMERA_DETAIL в выводе.
# Коды выхода: 0 — сценарий прошёл, 1 — сценарий упал, 2 — устройство не найдено,
# 3 — нет строки результата за таймаут, 4 — не собралось/не встало/не запустилось, 64 — неверный вызов.
#
# Разрешение камеры у пакета должно быть выдано (release: adb shell pm grant dev.fasticamera
# android.permission.CAMERA); экран разблокирован.
#
set -euo pipefail

# shellcheck source=lib/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

usage() {
    # Справка — вся шапка-комментарий файла (от второй строки до первой строки без «#»). Номера строк не
    # зашиваем: шапка растёт с каждым новым флагом, и зашитый диапазон молча обрезал её хвост (16a).
    awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}" >&2
}

variant="debug"
should_build=true
timeout_seconds=30
artifacts_dir=""
stages=""
surface=""
ab_window_ms=""
ab_pairs=""
unchanged_frame=""
stop_app_after=false
endurance_ms=""
exposure_controller=""
saturation_workloads=""
saturation_measure_ms=""
saturation_warmup_ms=""
neutral_card=false
keep_captures=false
scenario=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) variant="${2:-}"; shift 2 ;;
        --no-build) should_build=false; shift ;;
        --timeout) timeout_seconds="${2:-}"; shift 2 ;;
        --artifacts-dir) artifacts_dir="${2:-}"; shift 2 ;;
        --stages) stages="${2:-}"; shift 2 ;;
        --surface) surface="${2:-}"; shift 2 ;;
        --ab-window-ms) ab_window_ms="${2:-}"; shift 2 ;;
        --ab-pairs) ab_pairs="${2:-}"; shift 2 ;;
        --unchanged-frame) unchanged_frame="${2:-}"; shift 2 ;;
        --stop-app-after) stop_app_after=true; shift ;;
        --endurance-ms) endurance_ms="${2:-}"; shift 2 ;;
        --exposure-controller) exposure_controller="${2:-}"; shift 2 ;;
        --saturation-workloads) saturation_workloads="${2:-}"; shift 2 ;;
        --saturation-measure-ms) saturation_measure_ms="${2:-}"; shift 2 ;;
        --saturation-warmup-ms) saturation_warmup_ms="${2:-}"; shift 2 ;;
        --neutral-card) neutral_card=true; shift ;;
        --keep-captures) keep_captures=true; shift ;;
        -h|--help) usage; exit "$EXIT_OK" ;;
        -*) log_error "неизвестный аргумент: $1"; usage; exit "$EXIT_USAGE" ;;
        *)
            if [[ -n "$scenario" ]]; then
                log_error "сценарий уже задан ($scenario), лишний аргумент: $1"
                exit "$EXIT_USAGE"
            fi
            scenario="$1"; shift ;;
    esac
done

if [[ -z "$scenario" ]]; then
    log_error "не задан сценарий"
    usage
    exit "$EXIT_USAGE"
fi
if ! package="$(package_for_variant "$variant")"; then
    log_error "вариант должен быть debug или release, а не '$variant'"
    exit "$EXIT_USAGE"
fi
# Строки таблицы — только имена через запятую (их проверит Rust при initialize); ничего, что сломает adb shell.
if [[ -n "$stages" && ! "$stages" =~ ^[a-z0-9_]+(,[a-z0-9_]+)*$ ]]; then
    log_error "--stages ждёт имена строк через запятую (levels,tone,whole_viewfinder), а не '$stages'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$stages" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--stages имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
# Место замера — одно из трёх имён (их точный набор проверит и Rust при initialize).
if [[ -n "$surface" && ! "$surface" =~ ^(bare_viewfinder|photo_screen|calibrate_screen)$ ]]; then
    log_error "--surface ждёт bare_viewfinder, photo_screen или calibrate_screen, а не '$surface'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$surface" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--surface имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
# Политика пропуска тиков — только skip или always; имя поля Rust проверит ещё раз при initialize.
if [[ -n "$unchanged_frame" && "$unchanged_frame" != "skip" && "$unchanged_frame" != "always" ]]; then
    log_error "--unchanged-frame ждёт skip или always, а не '$unchanged_frame'"
    exit "$EXIT_USAGE"
fi
# Число пар A/B — только целое положительное число; границы проверит Rust при initialize.
if [[ -n "$ab_pairs" && ! "$ab_pairs" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--ab-pairs ждёт целое число пар, а не '$ab_pairs'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$ab_pairs" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--ab-pairs имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
# Длина окна A/B — только целое число мс; границы (длиннее прогрева) проверит Rust при initialize.
if [[ -n "$ab_window_ms" && ! "$ab_window_ms" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--ab-window-ms ждёт целое число миллисекунд, а не '$ab_window_ms'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$ab_window_ms" && "$scenario" != "viewfinder-stage-budget" ]]; then
    log_error "--ab-window-ms имеет смысл только для viewfinder-stage-budget"
    exit "$EXIT_USAGE"
fi
# Длина прогона на выносливость — только целое число мс; нижнюю границу (шаг снимков) проверит Rust при initialize.
if [[ -n "$endurance_ms" && ! "$endurance_ms" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--endurance-ms ждёт целое число миллисекунд, а не '$endurance_ms'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$endurance_ms" && "$scenario" != "viewfinder-endurance" ]]; then
    log_error "--endurance-ms имеет смысл только для viewfinder-endurance"
    exit "$EXIT_USAGE"
fi
# Имя автоэкспозиции — own или vendor; точный набор значений и тут, и в Rust (ExposureControllerKind).
if [[ -n "$exposure_controller" && ! "$exposure_controller" =~ ^(own|vendor)$ ]]; then
    log_error "--exposure-controller ждёт own или vendor, а не '$exposure_controller'"
    exit "$EXIT_USAGE"
fi
# Стенд насыщения: имена работ — только строчные буквы, цифры и «_» через запятую (набор имён проверит Rust при initialize);
# длины окон — целые миллисекунды (границы проверит Rust).
if [[ -n "$saturation_workloads" && ! "$saturation_workloads" =~ ^[a-z0-9_]+(,[a-z0-9_]+)*$ ]]; then
    log_error "--saturation-workloads ждёт имена работ через запятую (viewfinder,compositor), а не '$saturation_workloads'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$saturation_measure_ms" && ! "$saturation_measure_ms" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--saturation-measure-ms ждёт целое число миллисекунд, а не '$saturation_measure_ms'"
    exit "$EXIT_USAGE"
fi
if [[ -n "$saturation_warmup_ms" && ! "$saturation_warmup_ms" =~ ^[1-9][0-9]*$ ]]; then
    log_error "--saturation-warmup-ms ждёт целое число миллисекунд, а не '$saturation_warmup_ms'"
    exit "$EXIT_USAGE"
fi
if [[ ( -n "$saturation_workloads" || -n "$saturation_measure_ms" || -n "$saturation_warmup_ms" ) && "$scenario" != "gpu-saturation-bench" ]]; then
    log_error "--saturation-* имеют смысл только для gpu-saturation-bench"
    exit "$EXIT_USAGE"
fi
if [[ "$neutral_card" == true && "$scenario" != "viewfinder" && "$scenario" != "3a" ]]; then
    log_error "--neutral-card имеет смысл только для viewfinder и 3a"
    exit "$EXIT_USAGE"
fi
if [[ "$keep_captures" == true && "$scenario" != "capture" ]]; then
    log_error "--keep-captures имеет смысл только для capture"
    exit "$EXIT_USAGE"
fi
if [[ ! "$timeout_seconds" =~ ^[0-9]+$ ]]; then
    log_error "--timeout ждёт целое число секунд, а не '$timeout_seconds'"
    exit "$EXIT_USAGE"
fi

# Сценариям со скриншотами нужен каталог артефактов: без явного — свежий временный (вне репозитория).
if [[ -z "$artifacts_dir" ]] && scenario_takes_screenshots "$scenario"; then
    artifacts_dir="$(mktemp -d -t fasticamera-smoke-XXXXXX)" || {
        log_error "не удалось создать временный каталог артефактов"
        exit "$EXIT_USAGE"
    }
fi
if [[ -n "$artifacts_dir" ]]; then
    mkdir -p "$artifacts_dir" || {
        log_error "не удалось создать каталог артефактов '$artifacts_dir'"
        exit "$EXIT_USAGE"
    }
    log_step "каталог артефактов (скриншоты): $artifacts_dir"
fi

serial="$(find_device_serial)" || exit "$EXIT_NO_DEVICE"

if [[ "$should_build" == true ]]; then
    build_apk "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"
fi
install_apk "$serial" "$variant" || exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Разрешение камеры возвращаем в любом исходе (в том числе по Ctrl+C). Ловушка ставится ДО сброса:
# частично выполненный сброс тоже надо откатить. Код выхода скрипта ловушка не меняет.
restore_permission_on_exit() {
    restore_camera_permission "$serial" "$package" ||
        log_error "не удалось вернуть разрешение камеры $package: выдайте вручную (adb -s $serial shell pm grant $package $CAMERA_PERMISSION)"
}
if scenario_resets_camera_permission "$scenario"; then
    trap restore_permission_on_exit EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    reset_camera_permission "$serial" "$package" || {
        log_error "не удалось сбросить разрешение камеры $package"
        exit "$EXIT_BUILD_OR_INSTALL_FAILED"
    }
fi

stop_other_variant "$serial" "$variant" || {
    log_error "не удалось остановить второй вариант приложения перед запуском"
    exit "$EXIT_BUILD_OR_INSTALL_FAILED"
}
since_epoch="$(device_epoch_timestamp "$serial")"
start_activity "$serial" "$package" -e scenario "$scenario" ${stages:+-e stages "$stages"} \
    ${surface:+-e surface "$surface"} \
    ${ab_window_ms:+-e ab_window_ms "$ab_window_ms"} \
    ${ab_pairs:+-e ab_pairs "$ab_pairs"} \
    ${unchanged_frame:+-e unchanged_frame "$unchanged_frame"} \
    ${endurance_ms:+-e endurance_ms "$endurance_ms"} \
    ${saturation_workloads:+-e saturation_workloads "$saturation_workloads"} \
    ${saturation_measure_ms:+-e saturation_measure_ms "$saturation_measure_ms"} \
    ${saturation_warmup_ms:+-e saturation_warmup_ms "$saturation_warmup_ms"} \
    ${exposure_controller:+-e exposure_controller "$exposure_controller"} ||
    exit "$EXIT_BUILD_OR_INSTALL_FAILED"

# Дополнительные флаги проверки результата (пустой массив — без флагов).
wait_options=()
if [[ "$neutral_card" == true ]]; then
    wait_options+=(--neutral-card)
fi
if [[ "$keep_captures" == true ]]; then
    wait_options+=(--keep-captures)
fi
log_step "ожидание FASTICAMERA_RESULT сценария $scenario (до $timeout_seconds с)"
# Без exec: после проверки надо отработать ловушку восстановления разрешения.
if python3 "$FASTICAMERA_ROOT/scripts/smoke_wait.py" \
    --serial "$serial" \
    --since-epoch "$since_epoch" \
    --package "$package" \
    --scenario "$scenario" \
    --timeout "$timeout_seconds" \
    ${artifacts_dir:+--artifacts-dir "$artifacts_dir"} \
    "${wait_options[@]}"; then
    smoke_exit_code=0
else
    smoke_exit_code=$?
fi
# После результата (любого исхода) остановить НАШЕ приложение: оно остаётся на экране и греет телефон между замерами.
# Ошибка остановки печатается, но код выхода сценария не меняет.
if [[ "$stop_app_after" == true ]]; then
    log_step "остановка $package после результата"
    adb -s "$serial" shell am force-stop "$package" ||
        log_error "не удалось остановить $package: остановите вручную (adb -s $serial shell am force-stop $package)"
fi
exit "$smoke_exit_code"
