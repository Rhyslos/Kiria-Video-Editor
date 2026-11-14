// Picture effects as passes: each reads pictures the compositor made
// (premultiplied working values) and writes one of the same size. Effects
// work on light: in a frame made in display values the picture is turned
// into light first and back after. Sizes are shares of the picture's
// height (1080 lines is the reference), so a preview drawn smaller looks
// the same as the export.

struct Pass {
    // Which pass (x), then three of its numbers (yzw).
    kind_numbers: vec4<f32>,
    // 1 when the pictures are in linear light (x); their width and height
    // in pixels (yz); a number that changes from frame to frame (w).
    linear_size_seed: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Pass;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var source: texture_2d<f32>;
// A second picture, for passes that combine two (otherwise the first again).
@group(0) @binding(3) var other: texture_2d<f32>;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) at: vec2<f32>,
};

@vertex
fn whole(@builtin(vertex_index) index: u32) -> Corner {
    let across = f32(index & 1u);
    let down = f32(index >> 1u);
    var out: Corner;
    out.position = vec4<f32>(across * 2.0 - 1.0, 1.0 - down * 2.0, 0.0, 1.0);
    out.at = vec2<f32>(across, down);
    return out;
}

// Pixels per reference pixel (one of 1080 lines).
fn scale() -> f32 {
    return it.linear_size_seed.z / 1080.0;
}

fn texel() -> vec2<f32> {
    return vec2<f32>(1.0) / it.linear_size_seed.yz;
}

// Unpremultiplied light, and alpha.
fn light_of(s: vec4<f32>) -> vec4<f32> {
    var rgb = s.rgb / max(s.a, 0.00001);
    if it.linear_size_seed.x < 0.5 {
        rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.4));
    }
    return vec4<f32>(rgb, s.a);
}

// Back to premultiplied working values.
fn stored(rgb_in: vec3<f32>, alpha: f32) -> vec4<f32> {
    var rgb = max(rgb_in, vec3<f32>(0.0));
    if it.linear_size_seed.x < 0.5 {
        rgb = pow(min(rgb, vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
    }
    return vec4<f32>(rgb * alpha, alpha);
}

fn sample_light(at: vec2<f32>) -> vec4<f32> {
    return light_of(textureSampleLevel(source, smooth_sampler, at, 0.0));
}

// 0: darker towards the edges in an ellipse of the picture's shape: from
// `size` of the way to an edge's middle, fully `amount` darker `softness`
// further on.
fn vignette(p: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let amount = it.kind_numbers.y;
    let size = it.kind_numbers.z;
    let softness = max(it.kind_numbers.w, 0.0001);
    let from_middle = length((at - vec2<f32>(0.5)) * 2.0);
    return p.rgb * (1.0 - amount * smoothstep(size, size + softness, from_middle));
}

// A whole number hashed to 0-1.
fn hash(n: u32) -> f32 {
    var x = n;
    x = (x ^ 61u) ^ (x >> 16u);
    x = x * 9u;
    x = x ^ (x >> 4u);
    x = x * 0x27d4eb2du;
    x = x ^ (x >> 15u);
    return f32(x & 0xffffffu) / 16777215.0;
}

// 1: film grain: brightness noise in cells `size` reference pixels across,
// new every frame, strongest in the middle tones as film's is.
fn grain(p: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let amount = it.kind_numbers.y;
    let cell = max(it.kind_numbers.z * scale(), 1.0);
    let pixel = at * it.linear_size_seed.yz;
    let c = vec2<u32>(floor(pixel / cell));
    let seed = u32(it.linear_size_seed.w);
    let n = hash(c.x * 73856093u ^ c.y * 19349663u ^ seed * 83492791u) - 0.5;
    let grey = dot(p.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let middle = 4.0 * grey * (1.0 - min(grey, 1.0));
    return p.rgb * (1.0 + n * amount * 0.8 * (0.25 + middle));
}

// 2: sharpen: the picture pushed away from a blur of its neighbours
// `radius` reference pixels away, by `amount`.
fn sharpen(p: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let amount = it.kind_numbers.y;
    let d = texel() * max(it.kind_numbers.z * scale(), 0.5);
    let around = (sample_light(at + vec2<f32>(d.x, 0.0)).rgb + sample_light(at - vec2<f32>(d.x, 0.0)).rgb
        + sample_light(at + vec2<f32>(0.0, d.y)).rgb + sample_light(at - vec2<f32>(0.0, d.y)).rgb) * 0.25;
    return p.rgb + (p.rgb - around) * amount * 2.0;
}

// 3: what is brighter than `threshold`, the rest black: what glows.
fn bright(p: vec4<f32>) -> vec3<f32> {
    let threshold = it.kind_numbers.y;
    let grey = dot(p.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let over = max(grey - threshold, 0.0) / max(grey, 0.00001);
    return p.rgb * over;
}

// 4 and 5: a gaussian blur `radius` reference pixels wide, across (4) or
// down (5), in 25 samples.
fn blur(at: vec2<f32>, across: bool) -> vec4<f32> {
    let radius = max(it.kind_numbers.y * scale(), 0.5);
    let step = select(vec2<f32>(0.0, texel().y), vec2<f32>(texel().x, 0.0), across) * radius / 6.0;
    var sum = vec4<f32>(0.0);
    var weights = 0.0;
    for (var i = -12; i <= 12; i++) {
        let x = f32(i) / 6.0;
        let w = exp(-0.5 * x * x * 4.0);
        // Premultiplied values blur correctly as they are.
        sum += textureSampleLevel(source, smooth_sampler, at + step * f32(i), 0.0) * w;
        weights += w;
    }
    return sum / weights;
}

// 6: the second picture's light added to the first's by `amount` (glow).
fn add(p: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let glow = light_of(textureSampleLevel(other, smooth_sampler, at, 0.0));
    return p.rgb + glow.rgb * glow.a * it.kind_numbers.y;
}

@fragment
fn effect(in: Corner) -> @location(0) vec4<f32> {
    let kind = i32(round(it.kind_numbers.x));
    if kind == 4 || kind == 5 {
        let blurred = blur(in.at, kind == 4);
        // Blurring is done on the stored values: a blur in display values
        // would darken, so frames in display values only glow roughly.
        return blurred;
    }
    let p = sample_light(in.at);
    var rgb = p.rgb;
    switch kind {
        case 0: { rgb = vignette(p, in.at); }
        case 1: { rgb = grain(p, in.at); }
        case 2: { rgb = sharpen(p, in.at); }
        case 3: { rgb = bright(p); }
        case 6: { rgb = add(p, in.at); }
        default: {}
    }
    return stored(rgb, p.a);
}
