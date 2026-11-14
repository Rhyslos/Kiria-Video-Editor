// One picture effect: a picture the compositor made (premultiplied working
// values) read whole and written again, changed, into a picture of the same
// size. Effects work on light: in a frame made in display values the
// picture is turned into light first and back after.

struct Pass {
    // Which effect (x: 0 vignette), then three of its numbers (yzw).
    kind_numbers: vec4<f32>,
    // 1 when the picture is in linear light (x); its width and height in
    // pixels (yz).
    linear_size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> it: Pass;
@group(0) @binding(1) var smooth_sampler: sampler;
@group(0) @binding(2) var source: texture_2d<f32>;

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) at: vec2<f32>,
};

@vertex
fn whole(@builtin(vertex_index) index: u32) -> Corner {
    // A strip of two triangles covering the target: 0-1 across and down.
    let across = f32(index & 1u);
    let down = f32(index >> 1u);
    var out: Corner;
    out.position = vec4<f32>(across * 2.0 - 1.0, 1.0 - down * 2.0, 0.0, 1.0);
    out.at = vec2<f32>(across, down);
    return out;
}

// Darkens towards the edges in an ellipse of the picture's shape: from
// `size` of the way to an edge's middle, fully `amount` darker `softness`
// further on.
fn vignette(light: vec3<f32>, at: vec2<f32>) -> vec3<f32> {
    let amount = it.kind_numbers.y;
    let size = it.kind_numbers.z;
    let softness = max(it.kind_numbers.w, 0.0001);
    let from_middle = length((at - vec2<f32>(0.5)) * 2.0);
    let dark = smoothstep(size, size + softness, from_middle);
    return light * (1.0 - amount * dark);
}

@fragment
fn effect(in: Corner) -> @location(0) vec4<f32> {
    let s = textureSample(source, smooth_sampler, in.at);
    let linear = it.linear_size.x > 0.5;
    let alpha = s.a;
    var rgb = s.rgb / max(alpha, 0.00001);
    if !linear {
        rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.4));
    }
    rgb = vignette(rgb, in.at);
    if !linear {
        rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
    }
    return vec4<f32>(rgb * alpha, alpha);
}
