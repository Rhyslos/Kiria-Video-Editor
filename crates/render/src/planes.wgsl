// Turns a finished frame into what a video encoder takes: a plane of
// brightness at full size and a plane of the two colour samples at half
// size (BT.709, limited range).

struct Planes {
    // 1 when the frame is in linear light (x).
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Planes;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var frame: texture_2d<f32>;

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

// The frame's colour here as video values, 0-1. Read between pixels (as
// the half-size colour plane does) it is the mean of the pixels around.
fn video(at: vec2<f32>) -> vec3<f32> {
    var rgb = clamp(textureSample(frame, smooth_sampler, at).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if it.flags.x > 0.5 {
        rgb = pow(rgb, vec3<f32>(1.0 / 2.4));
    }
    return rgb;
}

const WEIGHTS: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

@fragment
fn luma(in: Corner) -> @location(0) vec4<f32> {
    let y = dot(video(in.at), WEIGHTS);
    return vec4<f32>((16.0 + 219.0 * y) / 255.0, 0.0, 0.0, 1.0);
}

@fragment
fn chroma(in: Corner) -> @location(0) vec4<f32> {
    let rgb = video(in.at);
    let y = dot(rgb, WEIGHTS);
    let cb = (rgb.b - y) / 1.8556;
    let cr = (rgb.r - y) / 1.5748;
    return vec4<f32>((128.0 + 224.0 * cb) / 255.0, (128.0 + 224.0 * cr) / 255.0, 0.0, 1.0);
}
