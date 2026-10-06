@group(0) @binding(0) var<storage, read> pixels: array<u32>;
@group(0) @binding(1) var<storage, read> original: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;
@group(0) @binding(3) var<storage, read> config: array<vec4<f32>>;
fn alpha_at(p: vec2<i32>, size: vec2<u32>, offset: u32) -> f32 {
    let q = vec2<u32>(clamp(p, vec2(0), vec2<i32>(size) - 1));
    return f32(pixels[offset + q.y * size.x + q.x] >> 24u) / 255.0;
}

fn coverage_at(position: vec2<u32>) -> f32 {
    var point = (vec2<f32>(position) + 0.5) / config[0].xy * config[0].zw;
    if (config[1].w != 0.0) {
        let base = u32(config[1].w);
        let h = vec3((vec2<f32>(position) + 0.5) / config[0].xy, 1.0);
        let uv = vec2(dot(config[base + 2u].xyz, h), dot(config[base + 3u].xyz, h)) /
                 dot(config[base + 4u].xyz, h);
        let local = (uv - 0.5) * config[base + 1u].zw * config[base].zw;
        let rotation = config[base + 1u];
        point = config[base].xy + config[base].zw * 0.5 +
                vec2(local.x * rotation.x - local.y * rotation.y,
                     local.x * rotation.y + local.y * rotation.x);
    }
    // A stack program; see `Coverage` in coverage.rs for the operations.
    var stack: array<f32, 32>;
    var top = 0u;
    stack[0] = config[1].x;
    for (var i = 0u; i < u32(config[1].y); i++) {
        let base = 2u + i * 6u;
        let op = u32(config[base + 3u].w);
        let parameter = config[base + 4u].w;
        switch op {
            case 1u: {
                top = min(top + 1u, 31u);
                stack[top] = parameter;
            }
            case 2u: {
                let a = stack[top];
                top = max(top, 1u) - 1u;
                stack[top] = a + stack[top] * (1.0 - a);
            }
            case 3u: {
                stack[top] *= parameter;
            }
            case 5u: {
                let a = stack[top];
                top = max(top, 1u) - 1u;
                stack[top] *= a;
            }
            default: {
                let value = sample_source(point, base);
                if (op == 4u) {
                    stack[top] *= 1.0 - parameter * (1.0 - value);
                } else {
                    stack[top] *= value;
                }
            }
        }
    }
    return stack[0];
}

// The alpha of the source described at `config[base]`, zero outside it.
fn sample_source(point: vec2<f32>, base: u32) -> f32 {
    let header = config[base];
    let bounds = config[base + 1u];
    let rotation = config[base + 2u];
    let local = point - bounds.xy - bounds.zw * 0.5;
    var uv = vec2(local.x * rotation.x + local.y * rotation.y,
                  -local.x * rotation.y + local.y * rotation.x) /
                 bounds.zw * rotation.zw +
             0.5;
    let h = vec3(uv, 1.0);
    uv = vec2(dot(config[base + 3u].xyz, h), dot(config[base + 4u].xyz, h)) /
         dot(config[base + 5u].xyz, h);
    if (any(uv < vec2(0.0)) || any(uv >= vec2(1.0))) {
        return 0.0;
    }
    let size = vec2<u32>(header.xy);
    let offset = bitcast<u32>(header.z);
    if (header.w == 0.0) {
        let p = vec2<u32>(uv * header.xy);
        let index = p.y * size.x + p.x;
        return f32((pixels[offset + index / 4u] >> ((index % 4u) * 8u)) & 255u) / 255.0;
    }
    let p = uv * header.xy - 0.5;
    let low = vec2<i32>(floor(p));
    let f = fract(p);
    return mix(mix(alpha_at(low, size, offset), alpha_at(low + vec2(1, 0), size, offset), f.x),
               mix(alpha_at(low + vec2(0, 1), size, offset),
                   alpha_at(low + vec2(1, 1), size, offset), f.x),
               f.y);
}

@compute @workgroup_size(8, 8)
fn layer_coverage(@builtin(global_invocation_id) id: vec3<u32>) {
    let stride = u32(config[1].z);
    if (id.x >= stride / 4u || id.y >= u32(config[0].y)) {
        return;
    }
    let transform = u32(config[1].w);
    if (transform != 0u && config[transform + 2u].w != 0.0) {
        let index = id.y * u32(config[0].x) + id.x;
        let alpha = coverage_at(id.xy);
        if (config[transform + 2u].w == 1.0) {
            let value = original[index];
            let a = u32(floor(f32(value >> 24u) * alpha + 0.5));
            output[index] = (value & 0x00ffffffu) | (a << 24u);
        } else {
            let value = (original[index / 4u] >> ((index % 4u) * 8u)) & 255u;
            output[index] = u32(floor(f32(value) * alpha + 0.5));
        }
        return;
    }

    var packed = 0u;
    for (var k = 0u; k < 4u; k++) {
        let x = id.x * 4u + k;
        if (x < u32(config[0].x)) {
            packed |= u32(floor(clamp(coverage_at(vec2(x, id.y)), 0.0, 1.0) * 255.0 + 0.5))
                      << (k * 8u);
        }
    }
    output[id.y * (stride / 4u) + id.x] = packed;
}
