// Filter → Dither's ordered, halftone, pattern and scanline styles; mirrors `effects/dither.rs`.
// The including shader defines `dither_source` (straight eight-bit pixels), `dither_size` and
// `dither_param`, whose five entries are:
// 0: style, levels, density gamma, contrast slope
// 1: halftone cell, angle (radians), light on dark, original colors
// 2: chunky pixel size, round dots, scanline dots (0–1), wobble
// 3: dark color (black unless Two Colors), scanline spacing
// 4: light color

var<private> BAYER8: array<u32, 64> = array<u32, 64>(
    0u, 32u, 8u, 40u, 2u, 34u, 10u, 42u, 48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u,
    12u, 44u, 4u, 36u, 14u, 46u, 6u, 38u, 60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
    3u, 35u, 11u, 43u, 1u, 33u, 9u, 41u, 51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u,
    15u, 47u, 7u, 39u, 13u, 45u, 5u, 37u, 63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u);
var<private> BAYER4: array<u32, 16> = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
var<private> BAYER2: array<u32, 4> = array<u32, 4>(0u, 2u, 3u, 1u);
// Mac patterns, four rows per word with the first row in the low byte.
var<private> PATTERNS: array<u32, 34> = array<u32, 34>(
    0x00000000u, 0x00000000u, 0x00000080u, 0x00000008u, 0x00220088u, 0x00220088u,
    0x10204080u, 0x01020408u, 0x22882288u, 0x22882288u, 0x0000ff00u, 0x0000ff00u,
    0x88442211u, 0x88442211u, 0x00aa00aau, 0x00aa00aau, 0x55225588u, 0x55225588u,
    0x808080ffu, 0x080808ffu, 0x55aa55aau, 0x55aa55aau, 0x18244281u, 0x81422418u,
    0xaaddaa77u, 0xaaddaa77u, 0x77bbddeeu, 0x77bbddeeu, 0xffddff77u, 0xffddff77u,
    0xffffff7fu, 0xfffffff7u, 0xffffffffu, 0xffffffffu);

// Rust's `round`: halves away from zero (WGSL's `round` takes them to even).
fn dither_round(v: f32) -> f32 {
    return sign(v) * floor(abs(v) + 0.5);
}

fn dither_byte(v: vec3<f32>) -> vec3<f32> {
    return floor(clamp(v, vec3(0.0), vec3(1.0)) * 255.0 + 0.5) / 255.0;
}

fn dither_adjust(value: f32) -> f32 {
    let settings = dither_param(0u);
    var v = clamp(value, 0.0, 1.0);
    if v > 0.0 { v = pow(v, settings.z); }
    return clamp((v - 0.5) * settings.w + 0.5, 0.0, 1.0);
}

fn dither_luma(c: vec3<f32>) -> f32 {
    return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
}

// The chunky pixel at `cell`: the block averaged, colors weighted by coverage, in eight bits.
fn dither_chunk(cell: vec2<i32>) -> vec4<f32> {
    let block = i32(dither_param(2u).x);
    if block <= 1 { return dither_source(cell); }
    let size = dither_size();
    var sum = vec4(0.0);
    var n = 0.0;
    for (var y = cell.y * block; y < min((cell.y + 1) * block, size.y); y++) {
        for (var x = cell.x * block; x < min((cell.x + 1) * block, size.x); x++) {
            let p = floor(dither_source(vec2(x, y)) * 255.0 + 0.5);
            sum += vec4(p.rgb * p.a, p.a);
            n += 1.0;
        }
    }
    if sum.a <= 0.0 { return vec4(0.0); }
    return floor(vec4(sum.rgb / sum.a, sum.a / n) + 0.5) / 255.0;
}

// Tones after density and contrast: one per channel for original colors, else luminance.
fn dither_tone(c: vec4<f32>) -> vec3<f32> {
    if dither_param(1u).w != 0.0 {
        return vec3(dither_adjust(c.r), dither_adjust(c.g), dither_adjust(c.b));
    }
    return vec3(dither_adjust(dither_luma(c.rgb)));
}

fn dither_threshold(style: u32, x: u32, y: u32) -> f32 {
    if style == 2u { return (f32(BAYER2[(y & 1u) * 2u + (x & 1u)]) + 0.5) / 4.0; }
    if style == 3u { return (f32(BAYER4[(y & 3u) * 4u + (x & 3u)]) + 0.5) / 16.0; }
    return (f32(BAYER8[(y & 7u) * 8u + (x & 7u)]) + 0.5) / 64.0;
}

fn dither_ordered(v: vec3<f32>, threshold: f32, levels: f32) -> vec3<f32> {
    let steps = levels - 1.0;
    return min(floor(clamp(v, vec3(0.0), vec3(1.0)) * steps + threshold), vec3(steps)) / steps;
}

fn dither_spot(style: u32, u: f32, v: f32) -> f32 {
    if style == 5u { return 3.14159265 * (u * u + v * v); }
    if style == 6u { return abs(v) * 2.0; }
    return abs(u) + abs(v);
}

fn dither_palette(t: vec3<f32>) -> vec3<f32> {
    if dither_param(1u).w != 0.0 { return t; }
    let dark = dither_param(3u).rgb;
    return dark + (dither_param(4u).rgb - dark) * t.x;
}

// One scanline's tone at column `x`: the average over the rows it covers, shifted by wobble.
fn dither_scan(x: i32, top: i32, bottom: i32, shift: i32) -> vec3<f32> {
    let sx = x - shift;
    var sum = vec3(0.0);
    var n = 0.0;
    if sx >= 0 && sx < dither_size().x {
        for (var y = top; y < bottom; y++) {
            let p = dither_source(vec2(sx, y));
            if p.a > 0.0 {
                sum += dither_tone(p);
                n += 1.0;
            }
        }
    }
    if n == 0.0 { return vec3(0.0); }
    return sum / n;
}

fn dither_scanlines(position: vec2<i32>, pixel: vec4<f32>) -> vec4<f32> {
    let size = dither_size();
    let spacing = max(i32(dither_param(3u).w), 2);
    let middle = f32(spacing) / 2.0;
    let dots = clamp(dither_param(2u).z, 0.0, 1.0);
    let line = position.y / spacing;
    let top = line * spacing;
    let bottom = min(top + spacing, size.y);
    let wave = sin(f32(line) * 0.45) * 0.7 + sin(f32(line) * 1.7 + 1.3) * 0.3;
    let shift = i32(dither_round(dither_param(2u).w * wave));
    let offset = abs(f32(position.y - top) + 0.5 - middle);
    let along = (f32(position.x) + 0.5) % f32(spacing) - middle;
    let centered = i32(clamp(dither_round(f32(position.x) - along * dots), 0.0, f32(size.x - 1)));
    let scan = dither_scan(centered, top, bottom, shift);
    let original = dither_param(1u).w != 0.0;
    var t = scan.x;
    if original { t = dither_luma(scan); }
    let rgb = dither_palette(scan) * 1.35;
    let beam = middle * (0.2 + 0.5 * sqrt(clamp(t, 0.0, 1.0)));
    let across = along * dots;
    let cover = clamp(beam - sqrt(offset * offset + across * across) + 0.5, 0.0, 1.0);
    var screen = dither_param(3u).rgb;
    if original { screen = vec3(0.0); }
    return vec4(screen + (rgb - screen) * cover, pixel.a);
}

fn dither_pixel(position: vec2<i32>) -> vec4<f32> {
    let settings = dither_param(0u);
    let style = u32(settings.x);
    if style == 10u {
        let pixel = dither_source(position);
        if pixel.a <= 0.0 { return pixel; }
        return dither_scanlines(position, pixel);
    }
    let block = max(i32(dither_param(2u).x), 1);
    let cell = position / block;
    let chunk = dither_chunk(cell);
    if chunk.a <= 0.0 { return chunk; }
    let tone = dither_tone(chunk);
    let at = vec2<u32>(cell);
    var rgb = vec3(0.0);
    if style <= 4u {
        rgb = dither_palette(dither_ordered(tone, dither_threshold(style, at.x, at.y), settings.y));
    } else {
        let marks_settings = dither_param(1u);
        let light_on_dark = marks_settings.z != 0.0;
        let original = marks_settings.w != 0.0;
        var t = tone.x;
        if original { t = dither_luma(tone); }
        var coverage = 1.0 - t;
        if light_on_dark { coverage = t; }
        var amount = 0.0;
        if style == 8u {
            let index = u32(min(floor(coverage * 16.0 + 0.5), 16.0));
            let word = PATTERNS[index * 2u + (at.y & 7u) / 4u];
            let row = (word >> (((at.y & 7u) % 4u) * 8u)) & 255u;
            amount = f32((row >> (7u - (at.x & 7u))) & 1u);
        } else if style <= 7u {
            let f = vec2<f32>(at) + 0.5;
            let c = cos(marks_settings.y);
            let s = sin(marks_settings.y);
            let cell_size = max(marks_settings.x, 2.0);
            var u = (f.x * c + f.y * s) / cell_size;
            var v = (-f.x * s + f.y * c) / cell_size;
            u -= floor(u) + 0.5;
            v -= floor(v) + 0.5;
            amount = select(0.0, 1.0, coverage > dither_spot(style, u, v));
        }
        if original {
            var paper = 1.0;
            if light_on_dark { paper = 0.0; }
            rgb = paper + (chunk.rgb - paper) * amount;
        } else {
            var ink = dither_param(3u).rgb;
            var paper = dither_param(4u).rgb;
            if light_on_dark {
                ink = dither_param(4u).rgb;
                paper = dither_param(3u).rgb;
            }
            rgb = paper + (ink - paper) * amount;
        }
    }
    rgb = dither_byte(rgb);
    if block > 1 && dither_param(2u).y != 0.0 {
        // Round dots: the gap shows the dark color (black unless Two Colors).
        let middle = f32(block) / 2.0;
        let d = vec2<f32>(position % vec2(block)) + 0.5 - middle;
        let cover = clamp(f32(block) * 0.42 - length(d) + 0.5, 0.0, 1.0);
        rgb = dither_byte(rgb * cover + dither_param(3u).rgb * (1.0 - cover));
    }
    return vec4(rgb, chunk.a);
}
