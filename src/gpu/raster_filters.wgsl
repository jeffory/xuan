// Vignette, Bloom / Glow, Tonal Contrast and Dither of straight pixels (`stylize.wgsl` and
// `dither.wgsl`); config[0] is the size and `auxiliary` the blurred copy where one is needed.
@compute @workgroup_size(8, 8)
fn vignette(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(config[0].xy);
    if (any(id.xy >= size)) {
        return;
    }
    let i = id.y * size.x + id.x;
    let mask = vignette_mask(vec2<f32>(id.xy) + 0.5, vec2<f32>(size), config[1].xyz);
    result[i] = packed(vignette_pixel(rgba(i), mask, config[2].x, config[2].y, config[3].rgb, config[2].z != 0.0));
}

@compute @workgroup_size(8, 8)
fn bloom(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(config[0].xy);
    if (any(id.xy >= size)) {
        return;
    }
    let i = id.y * size.x + id.x;
    let s = rgba(i);
    let b = unpack4x8unorm(auxiliary[i]);
    let out = bloom_pixel(vec4(s.rgb * s.a, s.a), vec4(b.rgb * b.a, b.a), config[1].x);
    result[i] = packed(vec4(out.rgb / max(out.a, 0.000001), out.a));
}

@compute @workgroup_size(8, 8)
fn tonal_contrast(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(config[0].xy);
    if (any(id.xy >= size)) {
        return;
    }
    let i = id.y * size.x + id.x;
    result[i] = packed(tonal_contrast_pixel(rgba(i), unpack4x8unorm(auxiliary[i]), config[1].x, config[1].yzw));
}

fn dither_source(position: vec2<i32>) -> vec4<f32> {
    return rgba(u32(position.y) * u32(config[0].x) + u32(position.x));
}
fn dither_size() -> vec2<i32> { return vec2<i32>(config[0].xy); }
fn dither_param(index: u32) -> vec4<f32> { return config[1u + index]; }

@compute @workgroup_size(8, 8)
fn dither(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(config[0].xy);
    if (any(id.xy >= size)) {
        return;
    }
    result[id.y * size.x + id.x] = packed(dither_pixel(vec2<i32>(id.xy)));
}
