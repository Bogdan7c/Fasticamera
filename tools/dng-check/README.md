# dng-check — сверка нашего DNG с эталоном (вне CI)

Наш DNG пишет `crates/photo-output` (`dng::write_dng`) по спецификации Adobe DNG 1.7.1.0. Здесь — три проверки,
которые не входят в CI, потому что нужны внешние программы и снятые телефоном эталоны.

Эталоны лежат вне репозитория: `~/fasticamera-captures/c7-reference/captures/<прогон>/`
(`*.dng` — платформенный `DngCreator`, `*.stream.raw16` — кадр потока, `*.passport.json` — паспорт зонда).

> Важно: `*.dng` и `*.stream.raw16` в прогоне — **два разных снимка одной сцены** (разные метки времени в паспорте,
> раздел `still_frame` против `stream_frame`). Поэтому сверка идёт двумя способами, см. ниже.

## 1. Рендер через rawpy: `compare_with_dngcreator.py`

```sh
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/dng-check/compare_with_dngcreator.py \
    ~/fasticamera-captures/c7-reference/captures/204802-f42af9 \
    ~/fasticamera-captures/c7-reference/captures/204805-34dcf3 \
    ~/fasticamera-captures/c7-reference/captures/204813-691863
```

Для каждого прогона собирает наш DNG примером `cargo run -p photo-output --example dng_from_capture`:

* «те же пиксели» — метаданные кадра `still_frame` + отсчёты, вынутые из эталонного DNG. Расхождение здесь — только
  расхождение тегов;
* «кадр потока» — отсчёты `*.stream.raw16` (другой снимок: к разнице тегов добавляются шум и мерцание лампы).

Оба рендерятся rawpy одинаково: баланс белого — множители эталона (`camera_whitebalance` из `AsShotNeutral`),
без автояркости, линейный выход 16 бит, sRGB-примары, демозаика AHD. Таблица: PSNR (пик 65535), средний и максимальный
ΔE76 по плиткам 128×128 (ΔE между средними цветами плиток). Ниже таблицы — различия тегов IFD0.

Результат прогона 22.11.2026:

| прогон | камера | вариант | PSNR, дБ | ΔE ср. | ΔE макс. |
|---|---|---|---|---|---|
| 204802-f42af9 | основная, лампа | те же пиксели | ∞ (побитово) | 0,000 | 0,000 |
| 204802-f42af9 | основная, лампа | кадр потока | 40,39 | 0,261 | 0,530 |
| 204805-34dcf3 | ширик | те же пиксели | ∞ (побитово) | 0,000 | 0,000 |
| 204805-34dcf3 | ширик | кадр потока | 35,66 | 0,690 | 1,130 |
| 204813-691863 | фронт (RAW с биннингом, 2304×1728) | те же пиксели | ∞ (побитово) | 0,000 | 0,000 |
| 204813-691863 | фронт | кадр потока | 37,80 | 0,480 | 0,786 |

Разбор:

* «Те же пиксели»: рендер идентичен платформенному побитово у всех трёх камер (при этом rawpy читает у обоих файлов
  одинаковые уровни чёрного, баланс и матрицы). Первый прогон у фронта давал 89,9 дБ: пример квантовал матрицы
  паспорта (двоичные дроби n/128) в знаменатель 1 000 000 и вносил ошибку ≈ 3·10⁻⁷; теперь пример подбирает
  знаменатель, при котором число из JSON восстанавливается точно (128 … 10000 … 10⁶).
* «Кадр потока» 35–40 дБ, ΔE < 1,2 — это разница двух разных снимков (шум, лампа), а не ошибка записи: пиксели файла
  равны входу бит-в-бит (тест `samples_are_written_bit_for_bit_…`).
* Разница тегов (ожидаемая и осознанная):
  * `Orientation`: у DngCreator 9 (значение вне TIFF 6.0 — exiftool ругается), у нас 1 (в примере поворот 0);
  * съёмочные теги (`ExposureTime`, `FNumber`, ISO, `FocalLength`, `DateTimeOriginal`) у DngCreator лежат прямо в
    IFD0 — это нарушение TIFF/Exif (exiftool: «Wrong IFD»); у нас они в Exif IFD;
  * `DefaultCropOrigin/Size`: DngCreator срезает по 8 пикселей с каждого края, пример берёт весь кадр
    (в продукте кадрирование задаёт `StillFrameFacts.crop`);
  * `ActiveArea`, `DefaultScale`, тег 37398 (`TIFF/EPStandardID`) — необязательные, не пишем;
  * `OpcodeList2` (3908 байт) и `OpcodeList3` (88 байт) — у DngCreator они есть (по виду, карта виньетирования
    HAL и ещё один список), у нас нет. rawpy/libraw opcode-списки игнорирует, поэтому этот рендер их разницу НЕ
    показывает. Что с ними делать, решено в сессии 14f: пока в файле нашего DNG список 2 пишет только
    применённая НАМИ коррекция виньетирования (раздел 4 ниже); вендорские списки не копируем.

## 2. exiftool: `-validate -warning -a`

Скачать Image-ExifTool (Perl, версия на 22.11.2026 — 13.64) без установки в систему и проверить контрольную сумму
с `https://exiftool.org/checksums.txt`:

```sh
mkdir -p ~/.cache/fasticamera/tools/exiftool_dl && cd ~/.cache/fasticamera/tools/exiftool_dl
curl -sL -o Image-ExifTool-13.64.tar.gz https://sourceforge.net/projects/exiftool/files/Image-ExifTool-13.64.tar.gz/download
sha256sum Image-ExifTool-13.64.tar.gz     # dcb9f6be1f6705ee826a92d1522e3fec1d0b3749cdc061b03c46873d5fe204fc
mkdir -p ../exiftool && tar xzf Image-ExifTool-13.64.tar.gz -C ../exiftool
perl ~/.cache/fasticamera/tools/exiftool/Image-ExifTool-13.64/exiftool -validate -warning -a  файл.dng
```

(Прямая ссылка `exiftool.org/Image-ExifTool-….tar.gz` отдаёт 404: сайт ссылается на SourceForge.)

Результат: все три наших DNG — `Validate : OK`. У эталона DngCreator: `6 Warnings` (неизвестное значение
`Orientation`, пять «Wrong IFD» для съёмочных тегов). JPEG с нашим Exif-блоком, вставленным `insert_exif_app1`, — тоже `OK`.

## 3. dng_validate из Adobe DNG SDK 1.7.1

```sh
tools/dng-check/build_dng_validate.sh      # ~1 минута, результат: ~/.cache/fasticamera/tools/dng_build/dng_validate
~/.cache/fasticamera/tools/dng_build/dng_validate -v файл.dng
```

SDK (`dng_sdk_1_7_1.zip`, ≈ 80 МБ, `download.adobe.com/pub/adobe/dng/`) собирается g++ без проектов Xcode/VS: XMP, libjpeg
и libjxl отключены (`-DqDNGUseXMP=0 -DqDNGUseLibJPEG=0`), вместо `dng_jxl.cpp` — заглушка `jxl_stub.cpp`. Для
несжатого DNG этого достаточно. Сама спецификация лежит в том же архиве: `DNG_Spec_1_7_1_0.pdf`
(с `helpx.adobe.com` PDF отдаётся только браузеру, curl получает 403).

Результат: `Validation complete`, код выхода 0, ошибок нет. Предупреждения «Too little padding on … edge of CFA image» — только у DNG из
примера, потому что он берёт кадрирование «весь кадр» (`DefaultCropOrigin = 0, 0`); у эталона отступ 8 пикселей и
предупреждений нет. В продукте кадрирование задаёт снимок; при кадре «весь буфер» предупреждение появится снова
(редактор доинтерполирует край отражением, но формально DNG просит отступ).

## «Продуктовый» DNG для валидаторов

Три DNG выше — простые (поворот 0, кадрирование весь кадр, подтяжка 0, чётное начало). Поэтому отдельно проверен файл,
собранный `dng::write_dng` на кадре 256×192 с нечётным началом полезной области (1, 3), внутренним кадрированием 160×120,
`Clockwise90`, подтяжкой +0,25 EV, динамическими уровнями чёрного/белого и профилем шума: `exiftool -validate` — `OK`,
`dng_validate` — код выхода 0, `Validation complete`, без предупреждений. Читает же: Orientation 6, CFAPattern R G / G B,
DefaultCropOrigin 32, 24, BaselineExposure +0.25, NoiseProfile на 3 плоскости.

Урок для фикстур: `dng_validate` проверяет, что ForwardMatrix переводит равные значения камеры в белый D50
(0,9642; 1; 0,8249) и называет файл неверным, если это не так; поэтому тестовые матрицы должны давать этот белый.

## 4. Виньетирование: `OpcodeList2` с `GainMap` (сессия 14f)

Если снимок проявлен с нашей коррекцией виньетирования, DNG получает `OpcodeList2` — четыре `GainMap` (DNG 1.7,
«Opcode Lists» → «GainMap», опкод 9; по одному на позицию квадрата CFA 2×2, как у `DngCreator`), а
`DNGBackwardVersion` поднимается до 1.3.0.0 (только в этом случае). Отсчёты не трогаются. Список всегда big-endian.

> `rawpy`/LibRaw опкод-списки **игнорирует**: сравнение через rawpy (раздел 1) коррекцию не покажет и не должно
> показывать. Работу списка проверяет только Adobe SDK (`dng_validate`), а разбор байт — `exiftool -OpcodeList2`.

Сквозная проверка (нужны собранный `dng_validate` и плоское поле, снятое приложением):

```sh
~/.cache/fasticamera/isp-reference-venv/bin/python -I tools/dng-check/lens_shading_e2e.py \
    ~/fasticamera-captures/14f/app-anchor/17884.dng --camera 2 \
    --run-dir ~/fasticamera-captures/c7-reference/captures/204802-f42af9
```

Скрипт берёт отсчёты плоского поля из файла, собирает пример `dng_from_capture` два DNG (без списка и со списком из
`device-profiles/oneplus-cph2447/lens-shading.json`), проявляет их `dng_validate` и печатает яркость углов к центру.
Результат 23.11.2026 (основная камера, 4096×3072, квадраты 10 % кадра):

| DNG | линейная стадия 2 (после OpcodeList2), углы/центр ЛВ ПВ ЛН ПН | итог sRGB (с гаммой) |
|---|---|---|
| без `OpcodeList2` | 0,242  0,233  0,235  0,230 | 0,34  0,33  0,34  0,33 |
| с `OpcodeList2` | 1,040  1,010  0,957  0,991 | 0,98  0,96  0,94  0,95 |

Углы без списка ≈ 0,23 центра, со списком — в пределах ±5 % (остаток — неравномерность самого плоского поля).
`exiftool -validate` — `OK`, `dng_validate -v` разбирает 4 опкода (`GainMap`, minVersion 1.3.0.0, flags 1) без ошибок.
