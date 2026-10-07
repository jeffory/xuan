// Layer effects, after upstream Compositor's MetalLayerEffects (Rendering/MetalLayerEffects.swift).
// Planes of coverage are f32 values stored in u32 buffers. `layer_effects::render_cpu` runs the
// same passes on the CPU.

fn plane(index: u32) -> f32 { return bitcast<f32>(input[index]); }
fn second_plane(index: u32) -> f32 { return bitcast<f32>(auxiliary[index]); }
fn store_plane(index: u32, value: f32) { result[index] = bitcast<u32>(value); }

// config[0]: width, height, then pass-specific values.
fn inside_image(id: vec3<u32>) -> bool {
    return id.x < u32(config[0].x) && id.y < u32(config[0].y);
}

@compute @workgroup_size(8, 8)
fn fx_alpha(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let index = id.y * u32(config[0].x) + id.x;
    store_plane(index, f32(input[index] >> 24u) / 255.0);
}

// The largest (or, with config[0].w, the smallest) value within config[0].z along a row;
// past the edge there is nothing.
@compute @workgroup_size(8, 8)
fn fx_spread_rows(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let width = i32(config[0].x);
    let reach = i32(config[0].z);
    let smallest = config[0].w > 0.0;
    var best = select(0.0, 1.0, smallest);
    for (var offset = -reach; offset <= reach; offset++) {
        let x = i32(id.x) + offset;
        var value = 0.0;
        if x >= 0 && x < width { value = plane(id.y * u32(width) + u32(x)); }
        best = select(max(best, value), min(best, value), smallest);
    }
    store_plane(id.y * u32(width) + id.x, best);
}

@compute @workgroup_size(8, 8)
fn fx_spread_columns(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let width = u32(config[0].x);
    let height = i32(config[0].y);
    let reach = i32(config[0].z);
    let smallest = config[0].w > 0.0;
    var best = select(0.0, 1.0, smallest);
    for (var offset = -reach; offset <= reach; offset++) {
        let y = i32(id.y) + offset;
        var value = 0.0;
        if y >= 0 && y < height { value = plane(u32(y) * width + id.x); }
        best = select(max(best, value), min(best, value), smallest);
    }
    store_plane(id.y * width + id.x, best);
}

// The stroke's ring: between the shape (input) and the reached-out or pulled-in shape (auxiliary).
@compute @workgroup_size(8, 8)
fn fx_ring(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let index = id.y * u32(config[0].x) + id.x;
    let shape = plane(index);
    let moved = second_plane(index);
    store_plane(index, clamp(select(moved - shape, shape - moved, config[0].w > 0.0), 0.0, 1.0));
}

// The plane moved by config[0].zw, sampled between pixels.
@compute @workgroup_size(8, 8)
fn fx_shift(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let width = u32(config[0].x);
    let height = u32(config[0].y);
    let sx = f32(id.x) - config[0].z;
    let sy = f32(id.y) - config[0].w;
    var value = 0.0;
    if sx >= 0.0 && sy >= 0.0 && sx <= f32(width - 1u) && sy <= f32(height - 1u) {
        let x0 = u32(floor(sx));
        let y0 = u32(floor(sy));
        let x1 = min(x0 + 1u, width - 1u);
        let y1 = min(y0 + 1u, height - 1u);
        let fx = sx - f32(x0);
        let fy = sy - f32(y0);
        let top = plane(y0 * width + x0) + (plane(y0 * width + x1) - plane(y0 * width + x0)) * fx;
        let bottom = plane(y1 * width + x0) + (plane(y1 * width + x1) - plane(y1 * width + x0)) * fx;
        value = top + (bottom - top) * fy;
    }
    store_plane(id.y * width + id.x, value);
}

// A Gaussian of sigma config[0].z and half width config[0].w, clamped to the edge.
@compute @workgroup_size(8, 8)
fn fx_blur_rows(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let width = i32(config[0].x);
    let sigma = config[0].z;
    let radius = i32(config[0].w);
    var total = 0.0;
    var sum = 0.0;
    for (var offset = -radius; offset <= radius; offset++) {
        let weight = exp(-f32(offset * offset) / (2.0 * sigma * sigma));
        let x = clamp(i32(id.x) + offset, 0, width - 1);
        total += weight * plane(id.y * u32(width) + u32(x));
        sum += weight;
    }
    store_plane(id.y * u32(width) + id.x, total / sum);
}

@compute @workgroup_size(8, 8)
fn fx_blur_columns(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let width = u32(config[0].x);
    let height = i32(config[0].y);
    let sigma = config[0].z;
    let radius = i32(config[0].w);
    var total = 0.0;
    var sum = 0.0;
    for (var offset = -radius; offset <= radius; offset++) {
        let weight = exp(-f32(offset * offset) / (2.0 * sigma * sigma));
        let y = clamp(i32(id.y) + offset, 0, height - 1);
        total += weight * plane(u32(y) * width + id.x);
        sum += weight;
    }
    store_plane(id.y * width + id.x, total / sum);
}

// What lies outside the moved or softened shape (auxiliary), kept to the shape itself (input).
@compute @workgroup_size(8, 8)
fn fx_inside(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let index = id.y * u32(config[0].x) + id.x;
    store_plane(index, clamp(plane(index) * (1.0 - second_plane(index)), 0.0, 1.0));
}

fn over(current: vec4<f32>, paint: vec4<f32>, coverage: f32) -> vec4<f32> {
    let amount = clamp(coverage * paint.a, 0.0, 1.0);
    return vec4(paint.rgb * amount + current.rgb * (1.0 - amount), amount + current.a * (1.0 - amount));
}

// The layer's pixels (input, straight RGBA) with its effects. `auxiliary` holds five planes:
// the stroke's ring, the drop shadow, the inner shadow, the outer glow and the inner glow.
// config[1..7]: stroke, drop shadow, overlay, inner shadow, outer glow and inner glow colors
// with their opacity; config[7]: has stroke, inside stroke, has drop shadow, has inner shadow;
// config[8]: has overlay, has outer glow, has inner glow.
@compute @workgroup_size(8, 8)
fn fx_compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if !inside_image(id) { return; }
    let index = id.y * u32(config[0].x) + id.x;
    let count = u32(config[0].x) * u32(config[0].y);
    let pixel = unpack4x8unorm(input[index]);
    let shape = pixel.a;
    let flags = config[7];
    let more = config[8];
    var current = vec4(0.0);
    if flags.z > 0.0 { current = over(current, config[2], second_plane(count + index)); }
    if more.y > 0.0 { current = over(current, config[5], second_plane(3u * count + index) * (1.0 - shape)); }
    if flags.x > 0.0 && flags.y == 0.0 { current = over(current, config[1], second_plane(index)); }
    // A color overlay recolors the layer's own pixels and keeps their alpha.
    var face = pixel.rgb;
    if more.x > 0.0 { face = mix(face, config[3].rgb, clamp(config[3].a, 0.0, 1.0)); }
    current = vec4(face * pixel.a + current.rgb * (1.0 - pixel.a), pixel.a + current.a * (1.0 - pixel.a));
    if more.z > 0.0 { current = over(current, config[6], second_plane(4u * count + index)); }
    if flags.w > 0.0 { current = over(current, config[4], second_plane(2u * count + index)); }
    if flags.x > 0.0 && flags.y > 0.0 { current = over(current, config[1], second_plane(index)); }
    let alpha = clamp(current.a, 0.0, 1.0);
    if alpha <= 0.0 {
        result[index] = 0u;
        return;
    }
    result[index] = packed(vec4(current.rgb / alpha, alpha));
}
