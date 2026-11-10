// A separable Gaussian. Both passes reflect samples at the backdrop edge.
struct Backdrop {
    bounds: vec4<f32>,
    radii: vec4<f32>,
    tint: vec4<f32>,
    base: vec4<f32>,
    sigma: f32,
    saturation: f32,
    srgb_target: u32,
    opacity: f32,
    working_size: vec2<f32>,
    working_pad: vec2<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> effect: Backdrop;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn reflect_at_edge(position: vec2<f32>) -> vec2<f32> {
    let size = effect.working_size;
    let reflected = abs(fract(position / (2.0 * size) + 0.5) * 2.0 - 1.0);
    return clamp(reflected * size, vec2(0.5), size - 0.5);
}

fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    let normalized = distance / sigma;
    return exp(-0.5 * normalized * normalized);
}

fn blur(position: vec2<f32>, direction: vec2<f32>) -> vec4<f32> {
    let resolution = vec2<f32>(textureDimensions(source));
    let sigma = effect.sigma * dot(effect.working_size / effect.bounds.zw, direction);
    let radius = ceil(sigma * 3.0);
    var sum = textureSampleLevel(source, linear_sampler, position / resolution, 0.0);
    var total = 1.0;
    for (var tap = 1.0; tap <= radius; tap += 2.0) {
        // Linear texture filtering combines neighboring Gaussian taps.
        let first = gaussian_weight(tap, sigma);
        let second = select(0.0, gaussian_weight(tap + 1.0, sigma), tap < radius);
        let weight = first + second;
        let offset = direction * (tap + second / max(weight, 0.0001));
        let before = reflect_at_edge(position - offset);
        let after = reflect_at_edge(position + offset);
        sum += weight * (
            textureSampleLevel(source, linear_sampler, before / resolution, 0.0) +
            textureSampleLevel(source, linear_sampler, after / resolution, 0.0)
        );
        total += 2.0 * weight;
    }
    return sum / total;
}

@fragment
fn fs_downsample(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let resolution = vec2<f32>(textureDimensions(source));
    let scale = effect.bounds.zw / effect.working_size;
    let center = effect.bounds.xy + pos.xy * scale;
    let step = scale * 0.25;
    let low = effect.bounds.xy + 0.5;
    let high = effect.bounds.xy + effect.bounds.zw - 0.5;
    let sample = (
        textureSampleLevel(source, linear_sampler, clamp(center + vec2(-step.x, -step.y), low, high) / resolution, 0.0) +
        textureSampleLevel(source, linear_sampler, clamp(center + vec2(step.x, -step.y), low, high) / resolution, 0.0) +
        textureSampleLevel(source, linear_sampler, clamp(center + vec2(-step.x, step.y), low, high) / resolution, 0.0) +
        textureSampleLevel(source, linear_sampler, clamp(center + step, low, high) / resolution, 0.0)
    ) * 0.25;
    return sample + effect.base * (1.0 - sample.a);
}

@fragment
fn fs_horizontal(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(pos.xy, vec2(1.0, 0.0));
}

@fragment
fn fs_vertical(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(pos.xy, vec2(0.0, 1.0));
}

fn round_rect_coverage(position: vec2<f32>) -> f32 {
    let half_size = effect.bounds.zw * 0.5;
    let point = position - effect.bounds.xy - half_size;
    let radius = select(
        select(effect.radii.x, effect.radii.y, point.x > 0.0),
        select(effect.radii.w, effect.radii.z, point.x > 0.0),
        point.y > 0.0,
    );
    let corner = abs(point) - half_size + radius;
    let distance = length(max(corner, vec2(0.0))) + min(max(corner.x, corner.y), 0.0) - radius;
    return clamp(0.5 - distance, 0.0, 1.0);
}

fn to_srgb(linear: vec3<f32>) -> vec3<f32> {
    return select(12.92 * linear, 1.055 * pow(linear, vec3(1.0 / 2.4)) - 0.055, linear > vec3(0.0031308));
}

fn to_linear(srgb: vec3<f32>) -> vec3<f32> {
    return select(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)), srgb > vec3(0.04045));
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let relative = (pos.xy - effect.bounds.xy) / effect.bounds.zw;
    let sample_pos = clamp(relative * effect.working_size, vec2(0.5), effect.working_size - 0.5);
    let sample = textureSampleLevel(source, linear_sampler, sample_pos / vec2<f32>(textureDimensions(source)), 0.0);
    var color = clamp(sample.rgb / max(sample.a, 0.0001), vec3(0.0), vec3(1.0));
    if effect.srgb_target != 0u {
        color = to_srgb(color);
    }
    let gray = dot(color, vec3(0.213, 0.715, 0.072));
    let saturated = clamp(mix(vec3(gray), color, effect.saturation), vec3(0.0), vec3(1.0));
    let tint = effect.tint;
    let tinted_alpha = tint.a + sample.a * (1.0 - tint.a);
    let tinted_color = tint.rgb * tint.a + saturated * sample.a * (1.0 - tint.a);
    let coverage = round_rect_coverage(pos.xy) * effect.opacity;
    if effect.srgb_target != 0u {
        return vec4(to_linear(tinted_color / max(tinted_alpha, 0.0001)) * tinted_alpha, tinted_alpha) * coverage;
    }
    return vec4(tinted_color, tinted_alpha) * coverage;
}

@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(source, vec2<i32>(pos.xy), 0);
}
