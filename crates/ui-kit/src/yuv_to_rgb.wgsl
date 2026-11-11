// Draws a video picture from the decoder's YUV planes: one rectangle filling
// the viewport, each pixel converted to RGB here instead of on the CPU.

struct Picture {
    // Red, green and blue as weights of (Y, U, V, 1), samples in 0-1.
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
    // Where the rectangle's top left corner is in the stored picture (xy),
    // and where going right across the rectangle leads (zw): the turn that
    // shows the picture upright.
    origin_across: vec4<f32>,
    // Where going down the rectangle leads (xy); 1 when U and V share one
    // texture (z); 1 when the target encodes sRGB itself (w).
    down_flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> picture: Picture;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var luma: texture_2d<f32>;
// Planar pictures: U here and V in the next. Interleaved ones: the same
// two-channel texture in both.
@group(0) @binding(3) var chroma_u: texture_2d<f32>;
@group(0) @binding(4) var chroma_v: texture_2d<f32>;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) at: vec2<f32>,
};

@vertex
fn corner(@builtin(vertex_index) index: u32) -> Corner {
    // A strip of two triangles: top left, top right, bottom left, bottom right.
    let across = f32(index & 1u);
    let down = f32(index >> 1u);
    var out: Corner;
    out.position = vec4<f32>(across * 2.0 - 1.0, 1.0 - down * 2.0, 0.0, 1.0);
    out.at = picture.origin_across.xy + across * picture.origin_across.zw + down * picture.down_flags.xy;
    return out;
}

// What an sRGB target expects: it applies the opposite curve when storing.
fn linear_from_gamma(rgb: vec3<f32>) -> vec3<f32> {
    let lower = rgb / vec3<f32>(12.92);
    let higher = pow((rgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, rgb < vec3<f32>(0.04045));
}

@fragment
fn convert(in: Corner) -> @location(0) vec4<f32> {
    let y = textureSample(luma, smooth_sampler, in.at).r;
    let first = textureSample(chroma_u, smooth_sampler, in.at);
    let second = textureSample(chroma_v, smooth_sampler, in.at);
    let uv = mix(vec2<f32>(first.r, second.r), first.rg, picture.down_flags.z);
    let yuv = vec4<f32>(y, uv, 1.0);
    var rgb = vec3<f32>(dot(picture.red, yuv), dot(picture.green, yuv), dot(picture.blue, yuv));
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if picture.down_flags.w > 0.5 {
        rgb = linear_from_gamma(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
