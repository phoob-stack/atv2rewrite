// The original's quad and rider lighting (see lit.rs): SunLight.asm per
// pixel, then TextureColVertCol or ShinyThingWithDirt, on gamma-encoded
// colours.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear};
@if(DISTANCE_FOG)
import bevy_pbr::render::{mesh_view_bindings::fog, pbr_functions::apply_fog};

struct Lit {
    ambient: vec4<f32>,
    sun: vec4<f32>,
    specular: vec4<f32>,
    to_sun: vec4<f32>,
    opacity: f32,
    flags: u32,
};

const GLOSS: u32 = 1u;
const CUTOUT: u32 = 2u;
const DIRT: u32 = 4u;

// c95.z
const SPECULAR_POWER: f32 = 16.0;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> lit: Lit;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var lit_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var lit_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.0);
    @if(VERTEX_UVS_A)
    uv = in.uv;
    var colour = vec4<f32>(1.0);
    @if(VERTEX_COLORS)
    colour = in.color;
    let tex = textureSample(lit_texture, lit_sampler, uv);

    var alpha = lit.opacity;
    if (lit.flags & CUTOUT) != 0u {
        if tex.a < 0.5 {
            discard;
        }
    }
    if (lit.flags & DIRT) != 0u {
        alpha = alpha * tex.a * colour.a;
    }

    // Both sides are drawn, lit by the normal as the original does.
    let n = normalize(in.world_normal);
    let l = normalize(lit.to_sun.xyz);
    let n_dot_l = dot(n, l);

    let vc = linear_to_srgb(colour.rgb);
    let diffuse = saturate(vc * (lit.ambient.rgb + max(n_dot_l, 0.0) * lit.sun.rgb));
    var rgb = linear_to_srgb(tex.rgb) * diffuse * 2.0;

    if (lit.flags & GLOSS) != 0u && n_dot_l > 0.0 {
        let to_eye = normalize(view.world_from_view[2].xyz);
        let h = normalize(l + to_eye);
        let spec = saturate(pow(max(dot(n, h), 0.0), SPECULAR_POWER) * lit.specular.rgb);
        rgb = rgb + spec * tex.a;
    }

    var out = vec4<f32>(srgb_to_linear(saturate(rgb)), alpha);
    @if(DISTANCE_FOG)
    out = apply_fog(fog, out, in.world_position.xyz, view.world_position.xyz, in.position.xy);
    return out;
}
