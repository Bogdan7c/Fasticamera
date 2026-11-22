// Захват снимка (сессия 14, пакет P3): сырые отсчёты полезной области кадра камеры → буфер u16 little-endian.
//
// Одна нить = два соседних по строке отсчёта = одно 32-битное слово буфера (младшие 16 бит — левый отсчёт,
// старшие — правый): так буфер пишется целыми словами, а байты в памяти идут как little-endian u16 —
// ровно то, что ждёт `still_contract::shot::RawSamples16`. Значения НЕ масштабируются (RAW10 остаётся
// 0..1023). Ширина полезной области чётная (проверяет Rust).
//
// Включающий файл до #include определяет `uint raw_at(uint frame_x, uint frame_y)` — отсчёт кадра в
// координатах ВСЕГО кадра, и привязку 0 (источник).

layout(local_size_x_id = 0, local_size_y_id = 1, local_size_z = 1) in;

// Push-константы (20 байт; раскладку пишет Rust: photo::capture::CapturePush).
layout(push_constant) uniform CaptureGeometry {
    uint left;             // левый край полезной области в кадре
    uint top;              // верхний край
    uint width;            // ширина полезной области, отсчёты (чётная)
    uint height;           // высота полезной области, строки
    uint row_stride_bytes; // шаг строки RAW10, байт (для RAW16 не используется)
} capture;

layout(set = 0, binding = 1, std430) writeonly buffer CapturedSamples {
    uint words[];
} captured;

void main() {
    uint pairs_per_row = capture.width / 2u;
    uint pair = gl_GlobalInvocationID.x;
    uint row = gl_GlobalInvocationID.y;
    if (pair >= pairs_per_row || row >= capture.height) {
        return;
    }
    uint column = pair * 2u;
    uint frame_row = capture.top + row;
    uint first = raw_at(capture.left + column, frame_row) & 0xFFFFu;
    uint second = raw_at(capture.left + column + 1u, frame_row) & 0xFFFFu;
    captured.words[row * pairs_per_row + pair] = first | (second << 16u);
}
