#!/bin/sh
# Собирает dng_validate из Adobe DNG SDK 1.7.1 под Linux (без XMP, libjpeg и libjxl: наши DNG их не используют).
# Результат: ~/.cache/fasticamera/tools/dng_build/dng_validate. Подробности — README.md.
set -eu
TOOLS="$HOME/.cache/fasticamera/tools"
SDK="$TOOLS/dng_sdk_src/dng_sdk_1_7_1"
BUILD="$TOOLS/dng_build"
HERE="$(cd "$(dirname "$0")" && pwd)"

if [ ! -d "$SDK" ]; then
    mkdir -p "$TOOLS/dng_sdk_dl" "$TOOLS/dng_sdk_src"
    curl -sL -o "$TOOLS/dng_sdk_dl/dng_sdk_1_7_1.zip" https://download.adobe.com/pub/adobe/dng/dng_sdk_1_7_1.zip
    unzip -q "$TOOLS/dng_sdk_dl/dng_sdk_1_7_1.zip" -d "$TOOLS/dng_sdk_src"
fi

SRC="$SDK/dng_sdk/source"
# qLinux/qMacOS/qWinOS: без них dng_flags.h ищет RawEnvironment.h (его в SDK нет).
FLAGS="-std=c++17 -O1 -w -fpermissive -DqLinux=1 -DqMacOS=0 -DqWinOS=0 -DqDNGUseStdInt=1 -DqDNGValidateTarget=1 \
-DqDNGUseXMP=0 -DqDNGXMPFiles=0 -DqDNGXMPDocOps=0 -DqDNGUseLibJPEG=0 -DqDNGThreadSafe=1 \
-I$SRC -I$SDK/libjxl/libjxl/lib/include -I$SDK/libjxl/client_projects/include"

mkdir -p "$BUILD/obj"
# dng_xmp*.cpp — только с XMP SDK; dng_update_meta.cpp требует XMP; dng_jxl.cpp заменён заглушкой.
ls "$SRC"/*.cpp | grep -v 'dng_jxl.cpp\|dng_xmp\|dng_update_meta' | \
    xargs -P "$(nproc)" -I{} sh -c 'o="'"$BUILD"'/obj/$(basename {} .cpp).o"; [ -f "$o" ] || g++ '"$FLAGS"' -c {} -o "$o"'
g++ $FLAGS -c "$HERE/jxl_stub.cpp" -o "$BUILD/obj/jxl_stub.o"
g++ "$BUILD"/obj/*.o -o "$BUILD/dng_validate" -lpthread -lz
echo "готово: $BUILD/dng_validate"
