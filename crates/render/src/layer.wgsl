// One layer of the timeline's picture: a decoded picture's pixels placed
// into the frame. The pixels come as the decoder's planes (converted to RGB
// here) or as RGBA; the result is premultiplied by the layer's opacity, for
// the "over" blend.

struct Layer {
    // Red, green and blue as weights of (Y, U, V, 1), samples in 0-1.
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
    // Where the rectangle's top left corner is in the stored picture (xy)
    // and where going right across it leads (zw): the shown part, turned
    // upright.
    from_origin_across: vec4<f32>,
    // Where going down the rectangle leads in the stored picture (xy); how
    // the pixels are stored (z): 0 brightness plus a two-channel colour
    // plane, 1 brightness plus two colour planes, 2 RGBA, 3 a picture this
    // compositor made (working values, premultiplied); 1 when the frame is
    // in linear light (w).
    from_down_kind_linear: vec4<f32>,
    // Where the rectangle's top left corner goes in the frame (xy) and where
    // its top edge leads (zw), in clip space.
    to_origin_across: vec4<f32>,
    // Where its left edge leads (xy), its opacity (z), and 1 when the quick
    // colour below changes anything (w).
    to_down_opacity: vec4<f32>,
    // Quick colour, worked in linear light: what red, green and blue are
    // multiplied by (rgb), and how much colour is kept (a: 0 none, 1 as it is).
    gains_saturation: vec4<f32>,
    // How much red, green and blue count towards brightness (rgb), and the
    // power values are raised to around middle grey (a).
    weights_contrast: vec4<f32>,
    // The share of the second picture mixed in, in linear light (x): 0
    // shows the first alone (frame blending; yzw unused).
    blend: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Layer;
@group(0) @binding(1) var smooth_sampler: sampler;
// Brightness, or the whole RGBA picture.
@group(0) @binding(2) var first: texture_2d<f32>;
// Two colour planes: U here and V in the next. One two-channel colour
// plane: the same texture in both. RGBA: unused.
@group(0) @binding(3) var second: texture_2d<f32>;
@group(0) @binding(4) var third: texture_2d<f32>;
// The second picture, stored as the first: the next recorded frame.
@group(0) @binding(5) var next_first: texture_2d<f32>;
@group(0) @binding(6) var next_second: texture_2d<f32>;
@group(0) @binding(7) var next_third: texture_2d<f32>;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) at: vec2<f32>,
};

@vertex
fn place(@builtin(vertex_index) index: u32) -> Corner {
    // A strip of two triangles: top left, top right, bottom left, bottom right.
    let across = f32(index & 1u);
    let down = f32(index >> 1u);
    var out: Corner;
    let xy = it.to_origin_across.xy + across * it.to_origin_across.zw + down * it.to_down_opacity.xy;
    out.position = vec4<f32>(xy, 0.0, 1.0);
    out.at = it.from_origin_across.xy + across * it.from_origin_across.zw + down * it.from_down_kind_linear.xy;
    return out;
}

// One picture's colour and alpha at `at`, in video values: the planes
// converted to RGB, or RGBA as it is.
fn picture(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> vec4<f32> {
    let kind = it.from_down_kind_linear.z;
    if kind < 1.5 {
        let uv = mix(b.rg, vec2<f32>(b.r, c.r), kind);
        let yuv = vec4<f32>(a.r, uv, 1.0);
        let rgb = vec3<f32>(dot(it.red, yuv), dot(it.green, yuv), dot(it.blue, yuv));
        return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    return vec4<f32>(clamp(a.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), a.a);
}

// A picture this compositor made, drawn again (a nested timeline's, or what
// is below an adjustment layer): its working values are unpremultiplied,
// coloured in linear light, and premultiplied by its alpha and the layer's
// opacity again.
fn made(s: vec4<f32>) -> vec4<f32> {
    let linear = it.from_down_kind_linear.w > 0.5;
    let coloured = it.to_down_opacity.w > 0.5;
    let alpha = s.a;
    var rgb = s.rgb / max(alpha, 0.00001);
    if !linear && coloured {
        rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.4));
    }
    if coloured {
        rgb = quick_colour(rgb);
    }
    if !linear && coloured {
        rgb = pow(min(rgb, vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
    }
    let a = alpha * it.to_down_opacity.z;
    return vec4<f32>(rgb * a, a);
}

// Quick colour on linear light.
fn quick_colour(light: vec3<f32>) -> vec3<f32> {
    var rgb = light * it.gains_saturation.rgb;
    // Contrast turns around middle grey, which stays where it is.
    rgb = 0.18 * pow(max(rgb, vec3<f32>(0.0)) / 0.18, vec3<f32>(it.weights_contrast.a));
    let grey = dot(rgb, it.weights_contrast.rgb);
    return max(mix(vec3<f32>(grey), rgb, it.gains_saturation.a), vec3<f32>(0.0));
}

@fragment
fn layer(in: Corner) -> @location(0) vec4<f32> {
    let first_sample = textureSample(first, smooth_sampler, in.at);
    var shown = picture(
        first_sample,
        textureSample(second, smooth_sampler, in.at),
        textureSample(third, smooth_sampler, in.at),
    );
    // Sampled whether used or not: texture sampling must not depend on a
    // value that differs between pixels.
    let next = picture(
        textureSample(next_first, smooth_sampler, in.at),
        textureSample(next_second, smooth_sampler, in.at),
        textureSample(next_third, smooth_sampler, in.at),
    );
    if it.from_down_kind_linear.z > 2.5 {
        return made(first_sample);
    }
    var rgb = shown.rgb;
    var alpha = shown.a;
    let linear = it.from_down_kind_linear.w > 0.5;
    let coloured = it.to_down_opacity.w > 0.5;
    let blending = it.blend.x > 0.0;
    if linear || coloured || blending {
        // The reference display's curve (BT.1886): video values to light.
        rgb = pow(rgb, vec3<f32>(2.4));
    }
    if blending {
        // Two recorded frames mixed as light mixes.
        rgb = mix(rgb, pow(next.rgb, vec3<f32>(2.4)), it.blend.x);
        alpha = mix(alpha, next.a, it.blend.x);
    }
    if coloured {
        rgb = quick_colour(rgb);
    }
    if !linear && (coloured || blending) {
        rgb = pow(min(rgb, vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
    }
    alpha = alpha * it.to_down_opacity.z;
    return vec4<f32>(rgb * alpha, alpha);
}
