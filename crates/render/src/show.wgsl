// Shows a composed frame, or the part of it that is seen: one rectangle over
// the whole viewport, the frame's values turned into what the target expects.

struct Show {
    // 1 when the frame is in linear light (x); 1 when the target stores
    // sRGB itself and so wants linear values handed to it (y).
    flags: vec4<f32>,
    // The part of the frame shown: left and top (xy), right and bottom
    // (zw), 0 to 1 across and down. The whole frame is 0, 0, 1, 1.
    part: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Show;
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
    out.at = mix(it.part.xy, it.part.zw, vec2<f32>(across, down));
    return out;
}

// What an sRGB target expects: it applies the opposite curve when storing.
fn linear_from_gamma(rgb: vec3<f32>) -> vec3<f32> {
    let lower = rgb / vec3<f32>(12.92);
    let higher = pow((rgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, rgb < vec3<f32>(0.04045));
}

@fragment
fn show(in: Corner) -> @location(0) vec4<f32> {
    var rgb = clamp(textureSample(frame, smooth_sampler, in.at).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if it.flags.x > 0.5 {
        // Light back to video values, the way the layers came in.
        rgb = pow(rgb, vec3<f32>(1.0 / 2.4));
    }
    if it.flags.y > 0.5 {
        rgb = linear_from_gamma(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
