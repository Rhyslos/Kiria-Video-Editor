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
    // plane, 1 brightness plus two colour planes, 2 RGBA; 1 when the frame
    // is in linear light (w).
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
};

@group(0) @binding(0) var<uniform> it: Layer;
@group(0) @binding(1) var smooth_sampler: sampler;
// Brightness, or the whole RGBA picture.
@group(0) @binding(2) var first: texture_2d<f32>;
// Two colour planes: U here and V in the next. One two-channel colour
// plane: the same texture in both. RGBA: unused.
@group(0) @binding(3) var second: texture_2d<f32>;
@group(0) @binding(4) var third: texture_2d<f32>;

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

@fragment
fn layer(in: Corner) -> @location(0) vec4<f32> {
    let a = textureSample(first, smooth_sampler, in.at);
    let b = textureSample(second, smooth_sampler, in.at);
    let c = textureSample(third, smooth_sampler, in.at);
    let kind = it.from_down_kind_linear.z;
    var rgb = a.rgb;
    var alpha = a.a;
    if kind < 1.5 {
        let uv = mix(b.rg, vec2<f32>(b.r, c.r), kind);
        let yuv = vec4<f32>(a.r, uv, 1.0);
        rgb = vec3<f32>(dot(it.red, yuv), dot(it.green, yuv), dot(it.blue, yuv));
        alpha = 1.0;
    }
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let linear = it.from_down_kind_linear.w > 0.5;
    let coloured = it.to_down_opacity.w > 0.5;
    if linear || coloured {
        // The reference display's curve (BT.1886): video values to light.
        rgb = pow(rgb, vec3<f32>(2.4));
    }
    if coloured {
        rgb = rgb * it.gains_saturation.rgb;
        // Contrast turns around middle grey, which stays where it is.
        rgb = 0.18 * pow(rgb / 0.18, vec3<f32>(it.weights_contrast.a));
        let grey = dot(rgb, it.weights_contrast.rgb);
        rgb = max(mix(vec3<f32>(grey), rgb, it.gains_saturation.a), vec3<f32>(0.0));
        if !linear {
            rgb = pow(min(rgb, vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
        }
    }
    alpha = alpha * it.to_down_opacity.z;
    return vec4<f32>(rgb * alpha, alpha);
}
