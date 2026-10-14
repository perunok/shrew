// shrew's two instanced quad pipelines: rects (rounded via SDF, optional border) and atlas glyphs.
// Colors arrive as sRGB-encoded unorm. By default the target is a non-sRGB format and blending happens on the encoded
// values (what native toolkits do for text). When only an sRGB target exists, `srgb_target` makes the shaders output
// linear values so the hardware encode returns the intended colors.

struct Globals {
    viewport: vec2<f32>,
    srgb_target: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var mask_atlas: texture_2d<f32>;
@group(0) @binding(2) var color_atlas: texture_2d<f32>;

// Triangle strip corners for vertex 0..4: (0,0) (1,0) (0,1) (1,1).
fn corner(vi: u32) -> vec2<f32> {
    return vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
}

fn to_ndc(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Straight-alpha color in, premultiplied out (the blend state is premultiplied "over").
fn finish(c: vec4<f32>) -> vec4<f32> {
    var rgb = c.rgb;
    if g.srgb_target == 1u {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb * c.a, c.a);
}

fn clipped(p: vec2<f32>, clip: vec4<f32>) -> bool {
    return p.x < clip.x || p.y < clip.y || p.x >= clip.z || p.y >= clip.w;
}

// ---- rects ----

struct RectIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) params: vec2<f32>,
    @location(4) clip: vec4<f32>,
};

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) border_color: vec4<f32>,
    @location(2) @interpolate(flat) rect: vec4<f32>,
    @location(3) @interpolate(flat) params: vec2<f32>,
    @location(4) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn rect_vs(@builtin(vertex_index) vi: u32, r: RectIn) -> RectOut {
    var o: RectOut;
    o.pos = to_ndc(r.rect.xy + corner(vi) * r.rect.zw);
    o.color = r.color;
    o.border_color = r.border_color;
    o.rect = r.rect;
    o.params = r.params;
    o.clip = r.clip;
    return o;
}

fn rounded_box_sdf(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn rect_fs(i: RectOut) -> @location(0) vec4<f32> {
    let p = i.pos.xy;
    if clipped(p, i.clip) {
        discard;
    }
    let half = i.rect.zw * 0.5;
    let d = rounded_box_sdf(p - (i.rect.xy + half), half, i.params.x);
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    var c = i.color;
    let border = i.params.y;
    if border > 0.0 {
        // 1 inside the border band, 0 in the interior, antialiased at the inner edge.
        let t = clamp(d + border + 0.5, 0.0, 1.0);
        c = mix(c, i.border_color, t);
    }
    let a = c.a * coverage;
    if a <= 0.0 {
        discard;
    }
    return finish(vec4<f32>(c.rgb, a));
}

// ---- glyphs ----

struct GlyphIn {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) kind: u32,
    @location(5) clip: vec4<f32>,
};

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
    @location(3) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn glyph_vs(@builtin(vertex_index) vi: u32, gi: GlyphIn) -> GlyphOut {
    var o: GlyphOut;
    let c = corner(vi);
    o.pos = to_ndc(gi.pos + c * gi.size);
    o.uv = gi.uv + c * gi.size;
    o.color = gi.color;
    o.kind = gi.kind;
    o.clip = gi.clip;
    return o;
}

@fragment
fn glyph_fs(i: GlyphOut) -> @location(0) vec4<f32> {
    if clipped(i.pos.xy, i.clip) {
        discard;
    }
    // Quads sit on whole pixels and map 1:1 onto atlas texels, so a plain load needs no sampler or filtering.
    let texel = vec2<i32>(floor(i.uv));
    if i.kind == 0u {
        let a = textureLoad(mask_atlas, texel, 0).r * i.color.a;
        return finish(vec4<f32>(i.color.rgb, a));
    }
    // The color atlas holds premultiplied texels.
    let c = textureLoad(color_atlas, texel, 0);
    var rgb = c.rgb;
    if g.srgb_target == 1u && c.a > 0.0 {
        rgb = srgb_to_linear(rgb / c.a) * c.a;
    }
    return vec4<f32>(rgb, c.a) * i.color.a;
}
