@group(0) @binding(0) var previous: texture_2d<f32>;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var coverage: texture_2d<f32>;
@group(0) @binding(3) var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<uniform> params: Parameters;
@group(0) @binding(5) var display: texture_storage_2d<rgba8unorm, write>;
// A Color Lookup layer's table, one entry per element (`GpuCompositor::lut`).
@group(0) @binding(6) var<storage, read> lut: array<vec4<f32>>;

fn source_pixel(pixel: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source));
    let p = textureLoad(source, clamp(pixel, vec2(0), size - 1), 0);
    return vec4(p.rgb * p.a, p.a);
}

fn sample_source(uv: vec2<f32>) -> vec4<f32> {
    if any(uv < vec2(0.0)) || any(uv >= vec2(1.0)) { return vec4(0.0); }
    let point = uv * vec2<f32>(textureDimensions(source)) - 0.5;
    let low = vec2<i32>(floor(point));
    let fraction = fract(point);
    let a = mix(source_pixel(low), source_pixel(low + vec2(1, 0)), fraction.x);
    let b = mix(source_pixel(low + vec2(0, 1)), source_pixel(low + vec2(1, 1)), fraction.x);
    let p = mix(a, b, fraction.y);
    return vec4(select(vec3(0.0), p.rgb / max(p.a, 0.000001), p.a > 0.0), p.a);
}

fn motion_pixel(requested: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(source));
    // Sides at the canvas edge repeat edge texels (params.first: left, top,
    // right, bottom); the others are padded with transparent texels.
    var pixel = requested;
    if params.first.x > 0.5 { pixel.x = max(pixel.x, 0); }
    if params.first.y > 0.5 { pixel.y = max(pixel.y, 0); }
    if params.first.z > 0.5 { pixel.x = min(pixel.x, size.x - 1); }
    if params.first.w > 0.5 { pixel.y = min(pixel.y, size.y - 1); }
    if any(pixel < vec2(0)) || any(pixel >= size) { return vec4(0.0); }
    let p = textureLoad(source, pixel, 0);
    return vec4(p.rgb * p.a, p.a);
}

fn sample_motion_blur(uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let extent = abs(params.appearance.zw) * 0.5 + 0.5 / size;
    if any(uv < -extent) || any(uv > 1.0 + extent) { return vec4(0.0); }
    let steps = u32(params.appearance.y);
    var sum = vec4(0.0);
    for (var i = 0u; i < steps; i++) {
        let offset = (f32(i) + 0.5) / f32(steps) - 0.5;
        let point = (uv + offset * params.appearance.zw) * size - 0.5;
        let low = vec2<i32>(floor(point));
        let fraction = fract(point);
        let a = mix(motion_pixel(low), motion_pixel(low + vec2(1, 0)), fraction.x);
        let b = mix(motion_pixel(low + vec2(0, 1)), motion_pixel(low + vec2(1, 1)), fraction.x);
        sum += mix(a, b, fraction.y);
    }
    return vec4(sum.rgb / max(sum.a, 0.000001), sum.a / f32(steps));
}

@compute @workgroup_size(8, 8)
fn composite(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= vec2<u32>(params.canvas.xy)) { return; }
    let position = vec2<i32>(id.xy);
    if params.flags.y == 102u {
        textureStore(output, position, vec4(0.0));
        return;
    }
    let dst = textureLoad(previous, position, 0);
    if params.flags.y >= 100u {
        textureStore(display, position, select(vec4(dst.rgb * dst.a, dst.a), dst, params.flags.y == 101u));
        return;
    }
    let point = (vec2<f32>(id.xy) + 0.5) / params.canvas.xy * params.canvas.zw;
    var amount = params.appearance.x;
    if params.flags.z != 0u { amount *= textureLoad(coverage, position, 0).r; }
    if params.flags.y == 12u {
        var backdrop = vec4(0.0);
        if params.flags.w != 0u { backdrop = textureLoad(source, position, 0); }
        let strength = 1.0 - params.appearance.x + amount;
        let alpha = mix(backdrop.a, dst.a, strength);
        let color = mix(backdrop.rgb * backdrop.a, dst.rgb * dst.a, strength) / max(alpha, 0.000001);
        textureStore(output, position, vec4(color, alpha));
        return;
    }
    if params.flags.y == 13u {
        let filtered = textureLoad(source, position, 0);
        let alpha = mix(dst.a, filtered.a, amount);
        let color = mix(dst.rgb * dst.a, filtered.rgb * filtered.a, amount) / max(alpha, 0.000001);
        textureStore(output, position, vec4(color, alpha));
        return;
    }
    if params.flags.y == 16u {
        textureStore(output, position, vec4(mix(dst.rgb, clamp(color_lookup(dst.rgb), vec3(0.0), vec3(1.0)), amount), dst.a));
        return;
    }
    if params.flags.y != 0u {
        textureStore(output, position, vec4(mix(dst.rgb, clamp(adjust(dst.rgb, point + params.origin.xy), vec3(0.0), vec3(1.0)), amount), dst.a));
        return;
    }
    let local = point - params.bounds.xy - params.bounds.zw * 0.5;
    var uv = vec2(local.x * params.rotation.x + local.y * params.rotation.y,
        -local.x * params.rotation.y + local.y * params.rotation.x) / params.bounds.zw;
    uv = uv * params.rotation.zw + 0.5;
    if params.flags.w != 0u {
        let homogeneous = vec3(uv, 1.0);
        let divisor = dot(params.points[2].xyz, homogeneous);
        uv = vec2(dot(params.points[0].xyz, homogeneous), dot(params.points[1].xyz, homogeneous)) / divisor;
    }
    var src = vec4(0.0);
    if params.appearance.y > 0.0 {
        src = sample_motion_blur(uv);
    } else {
        src = sample_source(uv);
    }
    src.a *= amount;
    // The layer's Fill (params.second.x), as `blend::composite_filled`.
    let fill = params.second.x;
    if fill < 1.0 && special_fill(params.flags.x) {
        let shown = src.a * fill * (1.0 - dst.a);
        let alpha = shown + dst.a;
        let mixed = blend_filled(dst.rgb, src.rgb, params.flags.x, fill);
        let color = (shown * src.rgb + dst.a * (src.a * mixed + (1.0 - src.a) * dst.rgb))
            / max(alpha, 0.000001);
        textureStore(output, position, select(vec4(0.0), vec4(color, alpha), alpha > 0.0));
        return;
    }
    src.a *= fill;
    if params.flags.x == 13u {
        src.a = select(0.0, 1.0, dissolve_value(vec2<i32>(floor(point + params.origin.xy))) < src.a);
    }
    let alpha = src.a + dst.a * (1.0 - src.a);
    let color = ((1.0 - src.a) * dst.a * dst.rgb + (1.0 - dst.a) * src.a * src.rgb
        + dst.a * src.a * blend(dst.rgb, src.rgb, params.flags.x)) / max(alpha, 0.000001);
    textureStore(output, position, vec4(color, alpha));
}

// Color Burn, Linear Burn, Color Dodge, Linear Dodge, Vivid Light, Linear Light, Hard Mix and
// Difference: `BlendMode::fill_is_special`.
fn special_fill(mode: u32) -> bool {
    return mode == 6u || mode == 7u || mode == 8u || mode == 14u || mode == 16u
        || mode == 20u || mode == 21u || mode == 23u;
}

// `blend::blend_channel_filled`: the layer's colour moves toward the mode's neutral colour by
// the fill, except Hard Mix, whose threshold softens into a ramp.
fn blend_filled(d: vec3<f32>, s: vec3<f32>, mode: u32, fill: f32) -> vec3<f32> {
    switch mode {
        case 8u, 14u: { return blend(d, 1.0 + (s - 1.0) * fill, mode); }
        case 20u, 21u: { return blend(d, 0.5 + (s - 0.5) * fill, mode); }
        case 23u: { return clamp((d - fill * (1.0 - s)) / (1.0 - fill), vec3(0.0), vec3(1.0)); }
        default: { return blend(d, s * fill, mode); }
    }
}

fn lut_entry(index: u32) -> vec3<f32> { return lut[index].rgb; }

// `Lut::apply`: the domain and size in `params.first`, the domain's top and the mode (0 a 1D
// table, 1 trilinear, 2 tetrahedral) in `params.second`.
fn color_lookup(rgb: vec3<f32>) -> vec3<f32> {
    let size = u32(params.first.w);
    let last = f32(size - 1u);
    let unit = (rgb - params.first.xyz) / (params.second.xyz - params.first.xyz);
    let position = select(vec3(0.0), min(unit, vec3(1.0)) * last, unit >= vec3(0.0));
    let base = min(vec3<u32>(floor(position)), vec3(size - 2u));
    let f = position - vec3<f32>(base);
    let mode = params.second.w;
    if mode < 0.5 {
        let low = vec3(lut_entry(base.x).r, lut_entry(base.y).g, lut_entry(base.z).b);
        let high = vec3(lut_entry(base.x + 1u).r, lut_entry(base.y + 1u).g, lut_entry(base.z + 1u).b);
        return low + (high - low) * f;
    }
    let n = size;
    let origin = base.x + base.y * n + base.z * n * n;
    let c000 = lut_entry(origin);
    let c100 = lut_entry(origin + 1u);
    let c010 = lut_entry(origin + n);
    let c110 = lut_entry(origin + 1u + n);
    let c001 = lut_entry(origin + n * n);
    let c101 = lut_entry(origin + 1u + n * n);
    let c011 = lut_entry(origin + n + n * n);
    let c111 = lut_entry(origin + 1u + n + n * n);
    if mode < 1.5 {
        let c00 = c000 + (c100 - c000) * f.x;
        let c10 = c010 + (c110 - c010) * f.x;
        let c01 = c001 + (c101 - c001) * f.x;
        let c11 = c011 + (c111 - c011) * f.x;
        let c0 = c00 + (c10 - c00) * f.y;
        let c1 = c01 + (c11 - c01) * f.y;
        return c0 + (c1 - c0) * f.z;
    }
    var first = c010;
    var second = c110;
    var t = vec3(f.y, f.x, f.z);
    if f.x > f.y {
        if f.y > f.z { first = c100; second = c110; t = f; }
        else if f.x > f.z { first = c100; second = c101; t = vec3(f.x, f.z, f.y); }
        else { first = c001; second = c101; t = vec3(f.z, f.x, f.y); }
    } else if f.z > f.y { first = c001; second = c011; t = vec3(f.z, f.y, f.x); }
    else if f.z > f.x { first = c010; second = c011; t = vec3(f.y, f.z, f.x); }
    return c000 * (1.0 - t.x) + first * (t.x - t.y) + second * (t.y - t.z) + c111 * t.z;
}
