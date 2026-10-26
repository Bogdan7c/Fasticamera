// Единственная точка входа bindgen: заголовки NDK, из которых camera-android берёт API.
// Что именно попадёт в биндинги, ограничивает список разрешённых имён в build.rs.
#include <android/hardware_buffer.h>
#include <camera/NdkCameraDevice.h>
#include <camera/NdkCameraError.h>
#include <camera/NdkCameraManager.h>
#include <camera/NdkCameraMetadata.h>
#include <camera/NdkCameraMetadataTags.h>
#include <camera/NdkCameraCaptureSession.h>
#include <camera/NdkCaptureRequest.h>
#include <media/NdkImage.h>
#include <media/NdkImageReader.h>
#include <media/NdkMediaError.h>
