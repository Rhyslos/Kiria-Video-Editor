// A whole picture drawn over a whole target: a layer's own target laid over
// the frame, or the frame shown.

struct Whole {
    // 1 when the picture is in linear light and the target wants video
    // values (x). The rest is unused.
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Whole;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var picture: texture_2d<f32>;

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

@fragment
fn over(in: Corner) -> @location(0) vec4<f32> {
    var colour = textureSample(picture, smooth_sampler, in.at);
    if it.flags.x > 0.5 {
        colour = vec4<f32>(pow(max(colour.rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)), colour.a);
    }
    return colour;
}
