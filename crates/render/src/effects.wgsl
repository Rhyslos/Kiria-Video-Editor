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
    // A box in the picture, its left, top, right and bottom in 0-1: where
    // blur and pixelate apply (the whole picture is 0, 0, 1, 1); where the
    // picture as it came is, for mirror, rounded corners and making room.
    // For a move: its turn in radians (x) and shift (yz).
    spot: vec4<f32>,
    // Pixels per reference pixel (x): one of 1080 lines of the picture as
    // it came, before any room was made around it.
    reference: vec4<f32>,
    // The colour an outline or a shadow paints with, as light (rgb); a
    // key's screen colour as shown.
    paint: vec4<f32>,
    // A grade's numbers (pass 10): its wheels (shadows, midtones,
    // highlights, overall: red, green, blue, brightness), then exposure,
    // contrast, pivot and saturation, then the white balance's gains and
    // highlights, then shadows.
    grade: array<vec4<f32>, 7>,
    // A grade's curves, sampled at 33 even places from 0 to 1: master,
    // red, green and blue at each (used when grade[6].y is 1).
    curves: array<vec4<f32>, 33>,
    // Where a grade applies (pass 10): [0] the colour range's hue, hue
    // width, softness and whether it is on; [1] its saturation and
    // lightness from and to; [2] the mask's middle and size (shares of
    // the picture); [3] its turn (radians), feather, whether inverted, and
    // its shape (0 none, 1 ellipse, 2 rectangle).
    limits: array<vec4<f32>, 4>,
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
    return it.reference.x;
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

// How far inside the box `at` is: 1 inside, 0 outside, softened over a
// couple of pixels at its edges.
fn inside(at: vec2<f32>) -> f32 {
    let edge = texel() * 1.5;
    let from_left = smoothstep(it.spot.x - edge.x, it.spot.x + edge.x, at.x);
    let from_right = 1.0 - smoothstep(it.spot.z - edge.x, it.spot.z + edge.x, at.x);
    let from_top = smoothstep(it.spot.y - edge.y, it.spot.y + edge.y, at.y);
    let from_bottom = 1.0 - smoothstep(it.spot.w - edge.y, it.spot.w + edge.y, at.y);
    return from_left * from_right * from_top * from_bottom;
}

// 7: the second picture (a blur of the first) inside the box, the first
// outside it.
fn within(s: vec4<f32>, at: vec2<f32>) -> vec4<f32> {
    let blurred = textureSampleLevel(other, smooth_sampler, at, 0.0);
    return mix(s, blurred, inside(at));
}

// 8: inside the box, blocks `size` reference pixels across, each the
// colour at its middle (the blocks line up with the box's corner).
fn pixelate(s: vec4<f32>, at: vec2<f32>) -> vec4<f32> {
    let cell = max(it.kind_numbers.y * scale(), 1.0) * texel();
    let from_corner = at - it.spot.xy;
    let middle = it.spot.xy + (floor(from_corner / cell) + 0.5) * cell;
    let block = textureSampleLevel(source, smooth_sampler, clamp(middle, it.spot.xy, it.spot.zw), 0.0);
    return mix(s, block, inside(at));
}

// 9: the picture moved inside its own frame: enlarged `scale` times
// about a point that stays where it is (yz of the numbers, 0-1), turned
// (spot.x, radians) and shifted (spot.yz, 0-1). What would come from
// outside the picture is see-through.
fn moved(at: vec2<f32>) -> vec4<f32> {
    let scale = max(it.kind_numbers.y, 0.01);
    let fixed = it.kind_numbers.zw;
    let turn = it.spot.x;
    // In pixels, so turning keeps the picture's shape.
    let size = it.linear_size_seed.yz;
    let d = (at - fixed) * size / scale;
    let c = cos(turn);
    let s = sin(turn);
    let back = vec2<f32>(c * d.x + s * d.y, -s * d.x + c * d.y) / size;
    let taken = fixed + back + it.spot.yz;
    let sampled = textureSampleLevel(source, smooth_sampler, taken, 0.0);
    let inside = all(taken >= vec2<f32>(0.0)) && all(taken <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), sampled, inside);
}

// 6: the second picture's light added to the first's by `amount` (glow).
fn add(p: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let glow = light_of(textureSampleLevel(other, smooth_sampler, at, 0.0));
    return p.rgb + glow.rgb * glow.a * it.kind_numbers.y;
}

// How bright, by BT.709's weights.
fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Curve `k` (0 master, 1 red, 2 green, 3 blue) at `v`, between its
// samples; past 1 it goes on as it ends.
fn curve(v: f32, k: i32) -> f32 {
    let x = clamp(v, 0.0, 1.0) * 32.0;
    let i = min(i32(floor(x)), 31);
    let f = x - f32(i);
    let y = mix(it.curves[i][k], it.curves[i + 1][k], f);
    return y + max(v - 1.0, 0.0);
}

// How far round the wheel `a` is from `b` (0 to a half).
fn hue_apart(a: f32, b: f32) -> f32 {
    let d = abs(fract(a - b + 0.5) - 0.5);
    return d;
}

// 1 inside [low, high], falling to 0 over `soft` outside it.
fn in_band(v: f32, low: f32, high: f32, soft: f32) -> f32 {
    let s = max(soft, 0.0001);
    return clamp(1.0 + min(v - low, high - v) / s, 0.0, 1.0);
}

// How much a grade applies to the light `light` at `at` (0 to 1 of the
// picture): its colour range times its mask.
fn grade_where(light: vec3<f32>, at: vec2<f32>) -> f32 {
    var weight = 1.0;
    let range = it.limits[0];
    if (range.w > 0.5) {
        let v = pow(clamp(light, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
        let most = max(v.r, max(v.g, v.b));
        let least = min(v.r, min(v.g, v.b));
        let chroma = most - least;
        var hue = 0.0;
        if (chroma > 0.00001) {
            if (most == v.r) {
                hue = fract(((v.g - v.b) / chroma) / 6.0);
            } else if (most == v.g) {
                hue = ((v.b - v.r) / chroma + 2.0) / 6.0;
            } else {
                hue = ((v.r - v.g) / chroma + 4.0) / 6.0;
            }
        }
        let lightness = (most + least) / 2.0;
        let saturation = select(0.0, chroma / max(1.0 - abs(2.0 * lightness - 1.0), 0.00001), chroma > 0.00001);
        let soft = range.z * 0.25;
        let by_hue = select(clamp(1.0 + (range.y - hue_apart(hue, range.x)) / max(soft, 0.0001), 0.0, 1.0), 1.0, range.y >= 0.5);
        let sl = it.limits[1];
        weight = by_hue * in_band(saturation, sl.x, sl.y, soft) * in_band(lightness, sl.z, sl.w, soft);
    }
    let shape = it.limits[3];
    if (shape.w > 0.5) {
        let place = it.limits[2];
        let size = vec2<f32>(it.linear_size_seed.y, it.linear_size_seed.z);
        // In the picture's pixels, so a turn keeps the shape's proportions.
        var d = (at - place.xy) * size;
        let c = cos(-shape.x);
        let s = sin(-shape.x);
        d = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
        let half = max(place.zw * size * 0.5, vec2<f32>(0.5));
        let q = abs(d) / half;
        var r = 0.0;
        if (shape.w < 1.5) {
            r = length(q);
        } else {
            r = max(q.x, q.y);
        }
        let feather = max(shape.y, 0.0001);
        var inside = clamp((1.0 - r) / feather + 0.5, 0.0, 1.0);
        if (shape.z > 0.5) {
            inside = 1.0 - inside;
        }
        weight = weight * inside;
    }
    return weight;
}

// 10: a grade's primary corrections, on light. Exposure and white balance
// as gains; highlights and shadows as stops by how bright a pixel is;
// contrast around the pivot; the wheels on display-like values (light to
// the power 1/2.4), where a wheel's push is even across the range it
// works on: shadows lift the dark end, highlights scale the bright end,
// midtones bend the middle and overall moves everything; then saturation.
fn graded(light: vec3<f32>) -> vec3<f32> {
    let shadows_w = it.grade[0];
    let midtones_w = it.grade[1];
    let highlights_w = it.grade[2];
    let overall_w = it.grade[3];
    let tone = it.grade[4];
    let balance = it.grade[5];
    let more = it.grade[6];
    var rgb = max(light, vec3<f32>(0.0)) * exp2(tone.x) * balance.rgb;
    let y = luma(rgb);
    let bright_part = smoothstep(0.18, 1.0, y);
    let dark_part = 1.0 - smoothstep(0.0, 0.18, y);
    rgb = rgb * exp2(balance.w * bright_part + more.x * dark_part);
    // Contrast: 0 leaves it, 1 is twice as hard, -1 half.
    let pivot = max(tone.z, 0.0001);
    rgb = pivot * pow(max(rgb, vec3<f32>(0.0)) / pivot, vec3<f32>(exp2(tone.y)));
    var v = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4));
    let lift = (shadows_w.rgb + shadows_w.w) * 0.25;
    let gain = vec3<f32>(1.0) + (highlights_w.rgb + highlights_w.w) * 0.5;
    let gamma = exp2(-(midtones_w.rgb + midtones_w.w));
    let offset = (overall_w.rgb + overall_w.w) * 0.25;
    v = v * gain + lift * (vec3<f32>(1.0) - v);
    v = pow(max(v, vec3<f32>(0.0)), gamma);
    v = v + offset;
    if (more.y > 0.5) {
        v = vec3<f32>(curve(v.r, 0), curve(v.g, 0), curve(v.b, 0));
        v = vec3<f32>(curve(v.r, 1), curve(v.g, 2), curve(v.b, 3));
    }
    rgb = pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.4));
    let grey = luma(rgb);
    return max(mix(vec3<f32>(grey), rgb, tone.w), vec3<f32>(0.0));
}

// 11: turned over inside the box: left for right (y of the numbers) and
// top for bottom (z).
fn mirrored(at: vec2<f32>) -> vec4<f32> {
    var taken = at;
    if it.kind_numbers.y > 0.5 {
        taken.x = it.spot.x + it.spot.z - at.x;
    }
    if it.kind_numbers.z > 0.5 {
        taken.y = it.spot.y + it.spot.w - at.y;
    }
    return textureSampleLevel(source, smooth_sampler, taken, 0.0);
}

// 12: the box's corners rounded off by `radius` (0-1 of half its shorter
// side), see-through outside the curve, its edge smoothed over a pixel.
fn rounded(s: vec4<f32>, at: vec2<f32>) -> vec4<f32> {
    let size = it.linear_size_seed.yz;
    let low = it.spot.xy * size;
    let high = it.spot.zw * size;
    let half = (high - low) * 0.5;
    let r = clamp(it.kind_numbers.y, 0.0, 1.0) * min(half.x, half.y);
    let p = abs(at * size - (low + high) * 0.5) - (half - vec2<f32>(r));
    let d = length(max(p, vec2<f32>(0.0))) + min(max(p.x, p.y), 0.0) - r;
    return s * clamp(0.5 - d, 0.0, 1.0);
}

// 13: the picture copied into a larger one, into the box; around it is
// see-through.
fn roomy(at: vec2<f32>) -> vec4<f32> {
    let taken = (at - it.spot.xy) / max(it.spot.zw - it.spot.xy, vec2<f32>(0.00001));
    let inside = all(taken >= vec2<f32>(0.0)) && all(taken <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), textureSampleLevel(source, smooth_sampler, taken, 0.0), inside);
}

// 14: a line `width` reference pixels thick of the paint colour around
// whatever the picture covers, outside it: the most it covers within that
// distance, looked for round sixteen directions.
fn outlined(s: vec4<f32>, at: vec2<f32>) -> vec4<f32> {
    let width = max(it.kind_numbers.y * scale(), 0.5);
    let steps = clamp(i32(ceil(width / 1.5)), 1, 10);
    var most = s.a;
    for (var i = 0; i < 16; i++) {
        let angle = f32(i) * 0.39269908;
        let way = vec2<f32>(cos(angle), sin(angle)) * texel();
        for (var k = 1; k <= steps; k++) {
            let far = width * f32(k) / f32(steps);
            most = max(most, textureSampleLevel(source, smooth_sampler, at + way * far, 0.0).a);
        }
    }
    let ring = stored(it.paint.rgb, most);
    return s + ring * (1.0 - s.a);
}

// 15: what the picture covers, filled with the paint colour at `amount`
// (y) and moved by the offset (zw, reference pixels): a shadow, before it
// is blurred.
fn silhouette(at: vec2<f32>) -> vec4<f32> {
    let back = at - it.kind_numbers.zw * scale() * texel();
    let inside = all(back >= vec2<f32>(0.0)) && all(back <= vec2<f32>(1.0));
    let covered = select(0.0, textureSampleLevel(source, smooth_sampler, back, 0.0).a, inside);
    return stored(it.paint.rgb, covered * clamp(it.kind_numbers.y, 0.0, 1.0));
}

// Blue-difference and red-difference of display-like values (BT.709).
fn chroma_of(v: vec3<f32>) -> vec2<f32> {
    let y = luma(v);
    return vec2<f32>((v.b - y) / 1.8556, (v.r - y) / 1.5748);
}

// 17: the screen's colour (the paint, as shown) made see-through. Colour
// is taken against brightness, so the screen's shadows count as the
// screen. A pixel's colour is measured along the screen's from grey, less
// twice how far it strays to the side, against the screen's own: 1 is the
// screen, 0 grey, below 0 another colour. What reaches 1 - 0.8 tolerance
// goes, solid again `softness` (z, up to half the scale) below; `spill`
// (w) of the screen's colour is taken off what stays, its brightness kept.
fn keyed(p: vec4<f32>) -> vec4<f32> {
    let v = pow(clamp(p.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
    let screen = chroma_of(it.paint.rgb) / (luma(it.paint.rgb) + 0.1);
    let strength = max(length(screen), 0.00001);
    let way = screen / strength;
    let y = luma(v);
    let c = chroma_of(v);
    let relative = c / (y + 0.1);
    let along = dot(relative, way);
    let aside = length(relative - way * along);
    let score = (along - 2.0 * aside) / strength;
    let reach = 1.0 - 0.8 * clamp(it.kind_numbers.y, 0.0, 1.0);
    let soft = 0.5 * clamp(it.kind_numbers.z, 0.0, 1.0);
    let gone = smoothstep(reach - soft - 0.0001, reach, score);
    // The spill: the part of its colour towards the screen's, taken away.
    let kept = c - way * max(dot(c, way), 0.0) * clamp(it.kind_numbers.w, 0.0, 1.0);
    let r = y + 1.5748 * kept.y;
    let b = y + 1.8556 * kept.x;
    let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
    let light = pow(max(vec3<f32>(r, g, b), vec3<f32>(0.0)), vec3<f32>(2.4));
    return stored(light, p.a * (1.0 - gone));
}

// 18: black bars over the box leaving a shape `ratio` (y) times as wide
// as tall in its middle: above and below, or at the sides; their edges
// smoothed over a pixel.
fn letterboxed(s: vec4<f32>, at: vec2<f32>) -> vec4<f32> {
    let size = it.linear_size_seed.yz;
    let low = it.spot.xy * size;
    let high = it.spot.zw * size;
    let span = high - low;
    let ratio = max(it.kind_numbers.y, 0.01);
    var opening = span;
    if span.x / max(span.y, 1.0) < ratio {
        opening.y = span.x / ratio;
    } else {
        opening.x = span.y * ratio;
    }
    let d = abs(at * size - (low + high) * 0.5) - opening * 0.5;
    let outside = clamp(max(d.x, d.y) + 0.5, 0.0, 1.0);
    // Inside the box only: what is around it (room made) stays.
    let inbox = all(at * size >= low) && all(at * size <= high);
    let black = select(0.0, outside, inbox);
    return mix(s, vec4<f32>(0.0, 0.0, 0.0, 1.0), black);
}

// 19-22: colour looks on light, each mixed in by `amount` (y). Black and
// white is grey of the same brightness; sepia a warm brown of it; duotone
// the brightness as shown, from the paint colour (shadows) to the spot
// row's (highlights); invert the negative of the values as shown.
fn looked(p: vec4<f32>, kind: i32) -> vec3<f32> {
    let amount = clamp(it.kind_numbers.y, 0.0, 1.0);
    let grey = luma(max(p.rgb, vec3<f32>(0.0)));
    var to = vec3<f32>(grey);
    if kind == 20 {
        let brown = vec3<f32>(1.0, 0.72, 0.42);
        to = grey * brown / luma(brown);
    } else if kind == 21 {
        let t = pow(clamp(grey, 0.0, 1.0), 1.0 / 2.4);
        to = mix(it.paint.rgb, it.spot.rgb, t);
    } else if kind == 22 {
        let v = pow(clamp(p.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
        to = pow(vec3<f32>(1.0) - v, vec3<f32>(2.4));
    }
    return mix(p.rgb, to, amount);
}

// 23: red taken from `offset` (zw, reference pixels) back, blue from as
// far the other way, green where it is: stored values moved, no light
// needed.
fn split(at: vec2<f32>) -> vec4<f32> {
    let d = it.kind_numbers.zw * scale() * texel();
    let red = textureSampleLevel(source, smooth_sampler, at - d, 0.0);
    let green = textureSampleLevel(source, smooth_sampler, at, 0.0);
    let blue = textureSampleLevel(source, smooth_sampler, at + d, 0.0);
    return vec4<f32>(red.r, green.g, blue.b, max(green.a, max(red.a, blue.a)));
}

// 24: smeared `length` (y, reference pixels) along `angle` (z, radians),
// half each way: 33 samples, stored values averaged as they are.
fn smeared(at: vec2<f32>) -> vec4<f32> {
    let length = it.kind_numbers.y * scale();
    let way = vec2<f32>(cos(it.kind_numbers.z), sin(it.kind_numbers.z)) * texel() * length;
    var sum = vec4<f32>(0.0);
    for (var i = -16; i <= 16; i++) {
        sum += textureSampleLevel(source, smooth_sampler, at + way * (f32(i) / 32.0), 0.0);
    }
    return sum / 33.0;
}

// 25: smeared towards a point (zw, 0-1), `amount` (y) of the way: 33
// samples from here towards it.
fn radial(at: vec2<f32>) -> vec4<f32> {
    let towards = (it.kind_numbers.zw - at) * clamp(it.kind_numbers.y, 0.0, 1.0);
    var sum = vec4<f32>(0.0);
    for (var i = 0; i <= 32; i++) {
        sum += textureSampleLevel(source, smooth_sampler, at + towards * (f32(i) / 32.0), 0.0);
    }
    return sum / 33.0;
}

// 26: a broken signal. Some frames glitch (more the more `often`, z), and
// in them some bands: each torn sideways by up to `amount` (y) of a fifth
// of the width, its red and blue drawn apart. The seed picks the frame's
// pattern, the same for the same frame every time.
fn glitched(at: vec2<f32>) -> vec4<f32> {
    let amount = clamp(it.kind_numbers.y, 0.0, 1.0);
    let often = clamp(it.kind_numbers.z, 0.0, 1.0);
    let seed = u32(it.linear_size_seed.w);
    let plain = textureSampleLevel(source, smooth_sampler, at, 0.0);
    if hash(seed * 2654435761u + 7u) > 0.25 + 0.75 * often {
        return plain;
    }
    // Bands of two sizes, so the tears are uneven.
    let coarse = u32(floor(at.y * 9.0));
    let fine = u32(floor(at.y * 41.0));
    let pick = hash(coarse * 7919u ^ seed * 83492791u) * hash(fine * 104729u ^ seed * 19349663u + 1u);
    if pick > 0.15 + 0.35 * often {
        return plain;
    }
    let tear = (hash(fine * 31u ^ seed * 73856093u + 3u) - 0.5) * 0.4 * amount;
    let apart = 0.012 * amount;
    let base = at + vec2<f32>(tear, 0.0);
    let red = textureSampleLevel(source, smooth_sampler, base + vec2<f32>(apart, 0.0), 0.0);
    let green = textureSampleLevel(source, smooth_sampler, base, 0.0);
    let blue = textureSampleLevel(source, smooth_sampler, base - vec2<f32>(apart, 0.0), 0.0);
    return vec4<f32>(red.r, green.g, blue.b, max(green.a, max(red.a, blue.a)));
}

// 27: bent as through a lens by `bend` (y, -1 to 1). Measured in pixels
// from the middle, a corner 1 away: the corners stay put and the middle is
// taken from up to 0.4 nearer (bulging) or further (pinched). What would
// come from outside the picture is see-through.
fn lensed(at: vec2<f32>) -> vec4<f32> {
    let size = it.linear_size_seed.yz;
    let d = (at - vec2<f32>(0.5)) * size;
    let corner = length(size * 0.5);
    let r2 = dot(d, d) / (corner * corner);
    let factor = 1.0 - 0.4 * clamp(it.kind_numbers.y, -1.0, 1.0) * (1.0 - r2);
    let taken = vec2<f32>(0.5) + d * factor / size;
    let inside = all(taken >= vec2<f32>(0.0)) && all(taken <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), textureSampleLevel(source, smooth_sampler, taken, 0.0), inside);
}

// 28: repeated `across` (y) times across and down.
fn tiled(at: vec2<f32>) -> vec4<f32> {
    let n = max(round(it.kind_numbers.y), 1.0);
    return textureSampleLevel(source, smooth_sampler, fract(at * n), 0.0);
}

// 29: mirrored round the middle in `wedges` (y) wedges turned `turn` (z,
// radians): each wedge is the first, every other one mirrored, so the
// edges meet. In pixels, so the wedges keep their angles.
fn kaleidoscope(at: vec2<f32>) -> vec4<f32> {
    let size = it.linear_size_seed.yz;
    let d = (at - vec2<f32>(0.5)) * size;
    let wedge = 6.2831853 / max(round(it.kind_numbers.y), 2.0);
    let turn = it.kind_numbers.z;
    var a = atan2(d.y, d.x) - turn;
    a = a - wedge * floor(a / wedge);
    if a > wedge * 0.5 {
        a = wedge - a;
    }
    a = a + turn;
    let taken = vec2<f32>(0.5) + vec2<f32>(cos(a), sin(a)) * length(d) / size;
    return textureSampleLevel(source, smooth_sampler, taken, 0.0);
}

// 30-32: looks on light. Posterise: `shades` (y) shades of each colour,
// as shown. Halftone: dots `size` (y, reference pixels) apart on a grid
// turned 45 degrees, each the colour at its middle on black, its area that
// colour's brightness as shown, mixed in by `amount` (z). Scanlines: dark
// lines `size` (z, reference pixels) apart, `amount` (y) dark at their
// middle, smoothly in between.
fn printed(p: vec4<f32>, at: vec2<f32>, kind: i32) -> vec3<f32> {
    if kind == 30 {
        let n = max(round(it.kind_numbers.y), 2.0) - 1.0;
        let v = pow(clamp(p.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.4));
        return pow(round(v * n) / n, vec3<f32>(2.4));
    }
    if kind == 31 {
        let size = it.linear_size_seed.yz;
        let cell = max(it.kind_numbers.y * scale(), 2.0);
        let px = at * size;
        // Into the turned grid, to the dot's middle, and back.
        let turned = vec2<f32>(px.x + px.y, px.y - px.x) * 0.70710678 / cell;
        let middle_turned = (floor(turned) + vec2<f32>(0.5)) * cell;
        let middle = vec2<f32>(middle_turned.x - middle_turned.y, middle_turned.x + middle_turned.y) * 0.70710678;
        let colour = light_of(textureSampleLevel(source, smooth_sampler, middle / size, 0.0));
        let shown = pow(clamp(luma(colour.rgb), 0.0, 1.0), 1.0 / 2.4);
        // The dot covers about as much of its cell as the colour is bright.
        let radius = sqrt(shown / 3.14159265) * cell;
        let inked = clamp(radius - length(px - middle) + 0.5, 0.0, 1.0);
        return mix(p.rgb, colour.rgb * inked, clamp(it.kind_numbers.z, 0.0, 1.0));
    }
    let apart = max(it.kind_numbers.z * scale(), 2.0);
    let y = at.y * it.linear_size_seed.z;
    let wave = 0.5 + 0.5 * cos(6.2831853 * y / apart);
    return p.rgb * (1.0 - clamp(it.kind_numbers.y, 0.0, 1.0) * wave);
}

// 33-36: light and action on light, in pixels so shapes keep their
// proportions. A light leak: two soft blobs of the paint colour added in,
// sliding along the top and the bottom as `phase` (z) goes from 0 to 1. A
// lens flare: a bright core at its point (zw), a streak across, and rings
// on the line from it through the middle. Speed lines: in 240 slices of
// the turn round their point (zw), a third of them, picked by the frame's
// seed, white from a random distance out to the edge, faint near its
// start. An impact frame: white above `threshold` (y), black below, the
// other way round with `invert` (z).
fn lit(p: vec4<f32>, at: vec2<f32>, kind: i32) -> vec3<f32> {
    let size = it.linear_size_seed.yz;
    let px = at * size;
    let tall = size.y;
    let amount = clamp(it.kind_numbers.y, 0.0, 1.0);
    if kind == 33 {
        let phase = it.kind_numbers.z;
        let one = vec2<f32>(fract(phase) * 1.4 - 0.2, 0.0) * size;
        let two = vec2<f32>(1.2 - fract(phase + 0.37) * 1.4, 1.0) * size;
        let reach = 0.55 * tall;
        let glow = exp(-dot(px - one, px - one) / (reach * reach)) + 0.8 * exp(-dot(px - two, px - two) / (reach * reach));
        return p.rgb + it.paint.rgb * glow * amount * 1.5;
    }
    if kind == 34 {
        let light = it.kind_numbers.zw * size;
        let d = px - light;
        let core = exp(-dot(d, d) / pow(0.04 * tall, 2.0)) * 4.0 + exp(-length(d) / (0.12 * tall)) * 0.6;
        let streak = exp(-abs(d.y) / (0.006 * tall)) * exp(-abs(d.x) / (0.45 * tall)) * 1.2;
        var rings = 0.0;
        let middle = size * 0.5;
        for (var i = 1; i <= 3; i++) {
            let along = light + (middle - light) * (0.6 * f32(i));
            let r = length(px - along);
            let ring = 0.03 * tall * f32(i);
            rings += exp(-pow((r - ring) / (0.008 * tall), 2.0)) * 0.25 + exp(-r * r / pow(ring * 0.6, 2.0)) * 0.12;
        }
        let warm = vec3<f32>(1.0, 0.85, 0.6);
        return p.rgb + (vec3<f32>(core + streak) + warm * rings) * amount;
    }
    if kind == 35 {
        let centre = it.kind_numbers.zw * size;
        let d = px - centre;
        let slice = u32(floor((atan2(d.y, d.x) / 6.2831853 + 0.5) * 240.0));
        let seed = u32(it.linear_size_seed.w);
        let shown = hash(slice * 2654435761u ^ seed * 40503u + 11u) < 0.35 * (0.3 + amount);
        let corner = length(max(centre, size - centre));
        let start = (0.35 + 0.55 * hash(slice * 7919u ^ seed * 31337u + 5u) * (1.2 - amount)) * corner;
        let r = length(d);
        let streak = select(0.0, smoothstep(start, start + 0.15 * corner, r), shown && r > start);
        return mix(p.rgb, vec3<f32>(1.0), streak * 0.9);
    }
    let shown = pow(clamp(luma(max(p.rgb, vec3<f32>(0.0))), 0.0, 1.0), 1.0 / 2.4);
    var white = shown > it.kind_numbers.y;
    if it.kind_numbers.z > 0.5 {
        white = !white;
    }
    return select(vec3<f32>(0.0), vec3<f32>(1.0), white);
}

// 37: only what is brighter, as shown, than 1 - `done` (y) stays, over an
// edge `softness` (z) wide: at 0 nothing shows, at 1 all of it, the
// brightest parts first.
fn revealed(p: vec4<f32>) -> vec4<f32> {
    let soft = max(it.kind_numbers.z, 0.001);
    let shown = pow(clamp(luma(max(p.rgb, vec3<f32>(0.0))), 0.0, 1.0), 1.0 / 2.4);
    let line = (1.0 - it.kind_numbers.y) * (1.0 + soft);
    let kept = clamp((shown + soft - line) / soft, 0.0, 1.0);
    return stored(p.rgb, p.a * kept);
}

// 38: smeared along a movement (motion blur). `back` (kind_numbers.yzw,
// spot.xyz: a, b, c, d, e, f) takes this place to where what is here was
// when the shutter opened; 24 samples along the way, none from outside
// the picture.
fn motion(at: vec2<f32>) -> vec4<f32> {
    let a = it.kind_numbers.yzw;
    let b = it.spot.xyz;
    let was = vec2<f32>(a.x * at.x + a.y * at.y + b.y, a.z * at.x + b.x * at.y + b.z);
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 24; i++) {
        let taken = mix(at, was, f32(i) / 23.0);
        let inside = all(taken >= vec2<f32>(0.0)) && all(taken <= vec2<f32>(1.0));
        sum += select(vec4<f32>(0.0), textureSampleLevel(source, smooth_sampler, taken, 0.0), inside);
    }
    return sum / 24.0;
}

@fragment
fn effect(in: Corner) -> @location(0) vec4<f32> {
    let kind = i32(round(it.kind_numbers.x));
    if kind == 9 {
        return moved(in.at);
    }
    if kind == 38 {
        return motion(in.at);
    }
    if kind == 11 {
        return mirrored(in.at);
    }
    if kind == 13 {
        return roomy(in.at);
    }
    if kind == 15 {
        return silhouette(in.at);
    }
    if kind == 23 {
        return split(in.at);
    }
    if kind == 24 {
        return smeared(in.at);
    }
    if kind == 25 {
        return radial(in.at);
    }
    if kind == 26 {
        return glitched(in.at);
    }
    if kind == 27 {
        return lensed(in.at);
    }
    if kind == 28 {
        return tiled(in.at);
    }
    if kind == 29 {
        return kaleidoscope(in.at);
    }
    if kind == 18 {
        return letterboxed(textureSampleLevel(source, smooth_sampler, in.at, 0.0), in.at);
    }
    if kind == 12 || kind == 14 || kind == 16 {
        let s = textureSampleLevel(source, smooth_sampler, in.at, 0.0);
        if kind == 12 {
            return rounded(s, in.at);
        }
        if kind == 14 {
            return outlined(s, in.at);
        }
        // 16: the picture over the second one (its shadow).
        let under = textureSampleLevel(other, smooth_sampler, in.at, 0.0);
        return s + under * (1.0 - s.a);
    }
    if kind == 7 || kind == 8 {
        // Mixing premultiplied values as they are: no light needed.
        let s = textureSampleLevel(source, smooth_sampler, in.at, 0.0);
        return select(pixelate(s, in.at), within(s, in.at), kind == 7);
    }
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
        case 10: { rgb = mix(p.rgb, graded(p.rgb), grade_where(p.rgb, in.at)); }
        case 17: { return keyed(p); }
        case 37: { return revealed(p); }
        case 19, 20, 21, 22: { rgb = looked(p, kind); }
        case 30, 31, 32: { rgb = printed(p, in.at, kind); }
        case 33, 34, 35, 36: { rgb = lit(p, in.at, kind); }
        default: {}
    }
    return stored(rgb, p.a);
}
