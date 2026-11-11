// The spike's two kinds of drawing: a decoded picture's planes placed into
// the frame (`place` + `layer`), and a whole picture drawn over a whole
// target (`whole` + `over`), which is both "lay this layer's own target over
// the frame" and "show the frame".

struct Layer {
    // Red, green and blue as weights of (Y, U, V, 1), samples in 0-1.
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
    // Where the rectangle's top left corner is in the frame (xy) and where
    // its top edge leads (zw), in clip space.
    origin_across: vec4<f32>,
    // Where its left edge leads (xy), its opacity (z), and 1 when the frame
    // is in linear light (w).
    down_opacity_linear: vec4<f32>,
    // The part of the picture shown: its top left corner (xy) and its size
    // (zw), both in 0-1 of the picture.
    crop: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Layer;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var luma: texture_2d<f32>;
@group(0) @binding(3) var chroma: texture_2d<f32>;

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
    let xy = it.origin_across.xy + across * it.origin_across.zw + down * it.down_opacity_linear.xy;
    out.position = vec4<f32>(xy, 0.0, 1.0);
    out.at = it.crop.xy + vec2<f32>(across, down) * it.crop.zw;
    return out;
}

@fragment
fn layer(in: Corner) -> @location(0) vec4<f32> {
    let y = textureSample(luma, smooth_sampler, in.at).r;
    let uv = textureSample(chroma, smooth_sampler, in.at).rg;
    let yuv = vec4<f32>(y, uv, 1.0);
    var rgb = vec3<f32>(dot(it.red, yuv), dot(it.green, yuv), dot(it.blue, yuv));
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if it.down_opacity_linear.w > 0.5 {
        // The reference display's curve (BT.1886): video values to light.
        rgb = pow(rgb, vec3<f32>(2.4));
    }
    let opacity = it.down_opacity_linear.z;
    // Premultiplied, for the "over" blend.
    return vec4<f32>(rgb * opacity, opacity);
}
