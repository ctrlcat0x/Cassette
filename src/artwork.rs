//! Small, reusable artwork palette extraction. Run decoding on artwork workers.
use image::DynamicImage;
use serde::{Deserialize, Serialize};

pub fn normalize_url(url: &str) -> String {
    if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_owned()
    }
}

/// Only unsigned Google size hints describe the cached image's intended shape.
pub fn aspect_ratio(url: &str) -> Option<f32> {
    if identity(url) == url || url.contains('?') {
        return None;
    }
    let hint = url.rsplit_once('=')?.1;
    let dimension = |axis: char| {
        hint.split('-')
            .find_map(|part| part.strip_prefix(axis)?.parse::<u32>().ok())
    };
    let (width, height) = (dimension('w')?, dimension('h')?);
    (width > 0 && height > 0).then_some(width as f32 / height as f32)
}

/// Thumbnail sizes from Google's image CDN share one cache identity. Signed or
/// unrelated URLs stay untouched; their query parameters may identify different art.
pub fn identity(url: &str) -> &str {
    if (url.starts_with("https://lh3.googleusercontent.com/")
        || url.starts_with("https://lh3.ggpht.com/")
        || url.starts_with("https://yt3.googleusercontent.com/")
        || url.starts_with("https://yt3.ggpht.com/"))
        && !url.contains('?')
        && let Some((base, size)) = url.rsplit_once('=')
        && !size.contains("sig")
        && (size.starts_with('w') || size.starts_with('s'))
        && size.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        return base;
    }
    url
}

/// Resize only known, unsigned Google artwork hints. Preserve all other URLs byte for byte.
pub fn at_size(url: &str, pixels: u32) -> String {
    if url.starts_with("//") {
        return at_size(&normalize_url(url), pixels);
    }
    let identity = identity(url);
    if identity == url {
        return url.into();
    }
    let Some((_, hint)) = url.rsplit_once('=') else {
        return url.into();
    };
    if url.contains('?') || hint.contains("sig") {
        return url.into();
    }
    let pixels = pixels.clamp(64, 1024);
    let mut parts: Vec<String> = hint.split('-').map(str::to_owned).collect();
    let dimension = |axis| {
        parts.iter().find_map(|part| {
            (part.as_bytes().first() == Some(&axis))
                .then(|| part[1..].parse::<u32>().ok())
                .flatten()
        })
    };
    let width = dimension(b'w');
    let height = dimension(b'h');
    let longest = width.unwrap_or(0).max(height.unwrap_or(0)).max(1);
    for part in &mut parts {
        if part.len() > 1
            && matches!(part.as_bytes()[0], b'w' | b'h' | b's')
            && part[1..].bytes().all(|c| c.is_ascii_digit())
        {
            let extent = match part.as_bytes()[0] {
                b'w' if height.is_some() => {
                    width.unwrap_or(longest).saturating_mul(pixels) / longest
                }
                b'h' if width.is_some() => {
                    height.unwrap_or(longest).saturating_mul(pixels) / longest
                }
                _ => pixels,
            }
            .max(1);
            *part = format!("{}{extent}", &part[..1]);
        }
    }
    format!("{identity}={}", parts.join("-"))
}

/// A single cached texture keeps artist headers sharp above a strong lower blur.
/// Generate on artwork workers, never during layout or painting.
pub fn artist_backdrop(image: &DynamicImage) -> DynamicImage {
    use image::imageops::FilterType;
    let (width, height) = (1024, 384);
    // Top alignment preserves faces instead of cropping the top of a portrait.
    // Crop before resizing so extreme input aspect ratios cannot allocate huge images.
    let crop_width = image
        .width()
        .min(image.height().saturating_mul(width) / height)
        .max(1);
    let crop_height = image
        .height()
        .min(image.width().saturating_mul(height) / width)
        .max(1);
    let clear = image
        .crop_imm((image.width() - crop_width) / 2, 0, crop_width, crop_height)
        .resize_exact(width, height, FilterType::Triangle);
    let soft = clear
        .thumbnail(128, 54)
        .blur(10.)
        .resize_exact(width, height, FilterType::Triangle)
        .to_rgb8();
    let mut result = clear.to_rgb8();
    for (x, y, pixel) in result.enumerate_pixels_mut() {
        let amount = ((y as f32 / (height - 1) as f32 - 0.32) / 0.58).clamp(0., 1.);
        let amount = amount * amount * (3. - 2. * amount);
        for channel in 0..3 {
            pixel.0[channel] = (pixel.0[channel] as f32 * (1. - amount)
                + soft.get_pixel(x, y).0[channel] as f32 * amount)
                .round() as u8;
        }
    }
    DynamicImage::ImageRgb8(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArtworkColors {
    pub dominant: u32,
    pub accent: u32,
    pub ambient: u32,
}

pub fn extract(image: &DynamicImage) -> ArtworkColors {
    let pixels = image.thumbnail(64, 64).to_rgb8();
    let mut bins = [[0u32; 4]; 4096];
    for pixel in pixels.pixels() {
        let [r, g, b] = pixel.0;
        let index = ((r as usize >> 4) << 8) | ((g as usize >> 4) << 4) | (b as usize >> 4);
        bins[index][0] += 1;
        bins[index][1] += u32::from(r);
        bins[index][2] += u32::from(g);
        bins[index][3] += u32::from(b);
    }
    let mean =
        |bin: &[u32; 4]| (bin[1] / bin[0]) << 16 | (bin[2] / bin[0]) << 8 | (bin[3] / bin[0]);
    let dominant = bins
        .iter()
        .max_by_key(|b| b[0])
        .filter(|b| b[0] > 0)
        .map(mean)
        .unwrap_or(0x252529);
    let accent = bins
        .iter()
        .filter(|bin| bin[0] > 0)
        .max_by(|a, b| {
            let score = |bin: &[u32; 4]| {
                let rgb = channels(mean(bin));
                let max = rgb.into_iter().fold(0f32, f32::max);
                let min = rgb.into_iter().fold(1f32, f32::min);
                let chroma = max - min;
                // Favour a sizeable colourful region; avoid black borders and white lettering.
                (bin[0] as f32).sqrt() * chroma * if (0.12..0.92).contains(&max) { 1. } else { 0.2 }
            };
            score(a).total_cmp(&score(b))
        })
        .map(mean)
        .unwrap_or(dominant);
    let rgb = channels(if contrast(accent, dominant) < 1.1 {
        dominant
    } else {
        accent
    });
    let grey = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
    let ambient = packed(rgb.map(|c| ((grey * 0.65 + c * 0.35) * 0.45 + 0.035).clamp(0.055, 0.24)));
    ArtworkColors {
        dominant,
        accent,
        ambient,
    }
}

fn channels(color: u32) -> [f32; 3] {
    [
        ((color >> 16) & 255) as f32 / 255.,
        ((color >> 8) & 255) as f32 / 255.,
        (color & 255) as f32 / 255.,
    ]
}
fn packed(channels: [f32; 3]) -> u32 {
    let [r, g, b] = channels.map(|c| (c.clamp(0., 1.) * 255.).round() as u32);
    (r << 16) | (g << 8) | b
}
pub fn luminance(color: u32) -> f32 {
    let [r, g, b] = channels(color).map(|c| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    r * 0.2126 + g * 0.7152 + b * 0.0722
}
pub fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
pub fn readable_accent(color: u32, background: u32) -> u32 {
    if contrast(color, background) >= 4.5 {
        return color;
    }
    let target = if luminance(background) > 0.35 { 0. } else { 1. };
    let original = channels(color);
    for step in 1..=100 {
        let factor = step as f32 / 100.;
        let adjusted = packed(original.map(|c| c + (target - c) * factor));
        if contrast(adjusted, background) >= 4.5 {
            return adjusted;
        }
    }
    if target == 0. { 0 } else { 0xffffff }
}
pub fn blend(from: u32, to: u32, fraction: f32) -> u32 {
    let a = channels(from);
    let b = channels(to);
    let f = fraction.clamp(0., 1.);
    packed([
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ])
}
pub fn on_accent(color: u32) -> u32 {
    if contrast(0, color) >= contrast(0xffffff, color) {
        0
    } else {
        0xffffff
    }
}
