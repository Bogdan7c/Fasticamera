//! Номера тегов TIFF/DNG, которые пишет наш DNG (DNG 1.7.1.0 «DNG Tags»; TIFF 6.0 / TIFF-EP).

// Базовые теги TIFF (TIFF 6.0, разделы 3, 8, 14).
pub const NEW_SUBFILE_TYPE: u16 = 254;
pub const IMAGE_WIDTH: u16 = 256;
pub const IMAGE_LENGTH: u16 = 257;
pub const BITS_PER_SAMPLE: u16 = 258;
pub const COMPRESSION: u16 = 259;
pub const PHOTOMETRIC_INTERPRETATION: u16 = 262;
pub const SAMPLES_PER_PIXEL: u16 = 277;
pub const ROWS_PER_STRIP: u16 = 278;
pub const PLANAR_CONFIGURATION: u16 = 284;
pub const EXIF_IFD_POINTER: u16 = 34665;

// CFA (TIFF-EP, раздел 3.2; DNG 1.7.1.0 CFAPlaneColor/CFALayout).
pub const CFA_REPEAT_PATTERN_DIM: u16 = 33421;
pub const CFA_PATTERN: u16 = 33422;

// Теги DNG.
pub const DNG_VERSION: u16 = 50706;
pub const DNG_BACKWARD_VERSION: u16 = 50707;
pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
pub const CFA_PLANE_COLOR: u16 = 50710;
pub const CFA_LAYOUT: u16 = 50711;
pub const BLACK_LEVEL_REPEAT_DIM: u16 = 50713;
pub const BLACK_LEVEL: u16 = 50714;
pub const WHITE_LEVEL: u16 = 50717;
pub const DEFAULT_CROP_ORIGIN: u16 = 50719;
pub const DEFAULT_CROP_SIZE: u16 = 50720;
pub const COLOR_MATRIX_1: u16 = 50721;
pub const COLOR_MATRIX_2: u16 = 50722;
pub const CAMERA_CALIBRATION_1: u16 = 50723;
pub const CAMERA_CALIBRATION_2: u16 = 50724;
pub const AS_SHOT_NEUTRAL: u16 = 50728;
pub const BASELINE_EXPOSURE: u16 = 50730;
pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
pub const FORWARD_MATRIX_1: u16 = 50964;
pub const FORWARD_MATRIX_2: u16 = 50965;
pub const NOISE_PROFILE: u16 = 51041;

/// `PhotometricInterpretation` = 32803: CFA (DNG 1.7.1.0, TIFF-EP).
pub const PHOTOMETRIC_CFA: u16 = 32803;
