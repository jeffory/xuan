// Vignette, Bloom / Glow and Tonal Contrast; mirrors `effects/stylize.rs`.

fn stylize_rec709(c: vec3<f32>) -> f32 {
    return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
}

fn stylize_smooth(t: f32) -> f32 {
    let u = clamp(t, 0.0, 1.0);
    return u * u * (3.0 - 2.0 * u);
}

// `shape` is [midpoint, roundness, feather].
fn vignette_mask(point: vec2<f32>, size: vec2<f32>, shape: vec3<f32>) -> f32 {
    let n = point / size * 2.0 - 1.0;
    let square = max(abs(n.x), abs(n.y));
    let circle = length(n) / sqrt(2.0);
    let distance = circle + (square - circle) * ((1.0 - shape.y / 100.0) * 0.5);
    let start = shape.x / 100.0 * 0.85;
    let soft = max(shape.z / 100.0, 0.05);
    return stylize_smooth((distance - start) / soft);
}

// Straight color in and out; `amount` and `highlights` in percent.
fn vignette_pixel(pixel: vec4<f32>, mask: f32, amount: f32, highlights: f32, color: vec3<f32>,
                  fills_clear: bool) -> vec4<f32> {
    if mask <= 0.0 || amount <= 0.0 || (pixel.a <= 0.0 && !fills_clear) { return pixel; }
    var bright = 0.0;
    if pixel.a > 0.0 { bright = clamp((stylize_rec709(pixel.rgb) - 0.45) / 0.55, 0.0, 1.0); }
    let effect = clamp(amount / 100.0, 0.0, 1.0) * mask * (1.0 - highlights / 100.0 * bright);
    if !fills_clear { return vec4(pixel.rgb + (color - pixel.rgb) * effect, pixel.a); }
    let out = pixel.a + effect * (1.0 - pixel.a);
    if out <= 0.0 { return pixel; }
    return vec4((color * effect + pixel.rgb * pixel.a * (1.0 - effect)) / out, out);
}

// Premultiplied in and out.
fn bloom_pixel(source: vec4<f32>, blurred: vec4<f32>, intensity: f32) -> vec4<f32> {
    let out = clamp(source + (max(source, blurred) - source) * intensity, vec4(0.0), vec4(1.0));
    return vec4(min(out.rgb, vec3(out.a)), out.a);
}

// Straight in and out; `strengths` are shadows, midtones and highlights in percent.
fn tonal_contrast_pixel(pixel: vec4<f32>, base: vec4<f32>, amount: f32, strengths: vec3<f32>) -> vec4<f32> {
    if pixel.a <= 0.0 || base.a <= 0.0 { return pixel; }
    let luminance = stylize_rec709(pixel.rgb);
    let base_luminance = stylize_rec709(base.rgb);
    let shadow = 1.0 - stylize_smooth((base_luminance - 0.15) / 0.35);
    let highlight = stylize_smooth((base_luminance - 0.5) / 0.35);
    let midtone = 1.0 - shadow - highlight;
    let weight = dot(strengths, vec3(shadow, midtone, highlight)) / 100.0;
    let detail = luminance - base_luminance;
    let delta = 0.18 * tanh(detail * 6.0) * weight * (amount / 50.0) * (4.0 * luminance * (1.0 - luminance));
    return vec4(clamp(pixel.rgb + delta, vec3(0.0), vec3(1.0)), pixel.a);
}
