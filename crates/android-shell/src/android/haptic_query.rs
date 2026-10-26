//! Опрос возможностей вибромотора у Kotlin при подключении Activity.
//!
//! Kotlin-методы — прямые обёртки над `Vibrator` (`hasVibrator`, `hasAmplitudeControl`,
//! `arePrimitivesSupported`, `areEffectsSupported`) по одному коду за вызов: решения о
//! том, что из этого следует, принимает `haptic_plan`.

use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::haptic_plan::{CompositionPrimitive, EffectSupport, HapticCapabilities, PredefinedEffect};

pub fn query_haptic_capabilities(env: &mut Env<'_>, platform: &JObject<'_>) -> jni::errors::Result<HapticCapabilities> {
    let has_vibrator = env
        .call_method(platform, jni_str!("hasVibrator"), jni_sig!("()Z"), &[])?
        .z()?;
    if !has_vibrator {
        return Ok(HapticCapabilities::absent());
    }
    let has_amplitude_control = env
        .call_method(platform, jni_str!("hasAmplitudeControl"), jni_sig!("()Z"), &[])?
        .z()?;
    let mut primitives = Vec::with_capacity(CompositionPrimitive::ALL.len());
    for primitive in CompositionPrimitive::ALL {
        let supported = env
            .call_method(
                platform,
                jni_str!("isPrimitiveSupported"),
                jni_sig!("(I)Z"),
                &[JValue::Int(primitive.android_code())],
            )?
            .z()?;
        primitives.push((primitive, supported));
    }
    let mut effects = Vec::with_capacity(PredefinedEffect::ALL.len());
    for effect in PredefinedEffect::ALL {
        let code = env
            .call_method(
                platform,
                jni_str!("effectSupport"),
                jni_sig!("(I)I"),
                &[JValue::Int(effect.android_code())],
            )?
            .i()?;
        effects.push((effect, EffectSupport::from_android_code(code)));
    }
    Ok(HapticCapabilities {
        has_vibrator,
        has_amplitude_control,
        primitives,
        effects,
    })
}
