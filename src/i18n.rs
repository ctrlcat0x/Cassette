use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
};

static LANGUAGE: AtomicU8 = AtomicU8::new(0);
static SCALE: AtomicU32 = AtomicU32::new(1.0f32.to_bits());
static MESSAGES: OnceLock<HashMap<String, [String; 2]>> = OnceLock::new();
pub fn set_language(language: &str) {
    LANGUAGE.store(
        match language {
            "hi" => 1,
            "es" => 2,
            _ => 0,
        },
        Ordering::Relaxed,
    );
}
pub fn set_scale(scale: f32) {
    SCALE.store(
        if scale.is_finite() {
            scale.clamp(0.75, 1.75)
        } else {
            1.
        }
        .to_bits(),
        Ordering::Relaxed,
    );
}
pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}
pub fn font() -> &'static str {
    if LANGUAGE.load(Ordering::Relaxed) == 1 {
        "Noto Sans Devanagari"
    } else {
        "Manrope"
    }
}
pub fn t(text: &str) -> &str {
    let index = LANGUAGE.load(Ordering::Relaxed);
    if index == 0 {
        return text;
    }
    MESSAGES
        .get_or_init(|| {
            serde_json::from_str(include_str!("../assets/locales/messages.json"))
                .expect("Bundled translations are valid")
        })
        .get(text)
        .map_or(text, |values| values[usize::from(index - 1)].as_str())
}
pub fn formatted(template: &str, rendered: &str) -> String {
    let translated = t(template);
    if translated == template {
        return rendered.into();
    }
    let mut values = Vec::new();
    let mut source = template;
    let mut result = rendered;
    while let Some((prefix, after)) = source.split_once('{') {
        let Some((key, tail)) = after.split_once('}') else {
            return rendered.into();
        };
        let Some(after_prefix) = result.strip_prefix(prefix) else {
            return rendered.into();
        };
        let suffix = tail.split('{').next().unwrap_or_default();
        let end = if suffix.is_empty() {
            after_prefix.len()
        } else {
            let Some(end) = after_prefix.find(suffix) else {
                return rendered.into();
            };
            end
        };
        values.push((
            key.split(':').next().unwrap_or_default(),
            after_prefix[..end].to_owned(),
        ));
        result = &after_prefix[end..];
        source = tail;
    }
    let mut output = String::new();
    let mut remaining = translated;
    let mut index = 0;
    while let Some((prefix, after)) = remaining.split_once('{') {
        let Some((key, tail)) = after.split_once('}') else {
            return rendered.into();
        };
        let key = key.split(':').next().unwrap_or_default();
        let value = if key.is_empty() {
            values.get(index)
        } else {
            values.iter().find(|(name, _)| *name == key)
        };
        let Some((_, value)) = value else {
            return rendered.into();
        };
        output.push_str(prefix);
        output.push_str(value);
        remaining = tail;
        index += 1;
    }
    output.push_str(remaining);
    output
}
#[macro_export]
macro_rules! tr {
    ($template:literal $($tail:tt)*) => { $crate::i18n::formatted($template, &format!($template $($tail)*)) };
}
