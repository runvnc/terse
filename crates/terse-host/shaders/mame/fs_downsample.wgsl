// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/fs_downsample.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Reduces the full-resolution phosphor buffer with a four-sample box filter,
// producing the lower-resolution source used by the bloom blur passes.

// SAMPLER2D(s_accum, 0)  ->  a texture and its sampler
@group(0) @binding(0) var s_accum: texture_2d<f32>;
@group(0) @binding(1) var s_accum_sampler: sampler;
@group(0) @binding(2) var<uniform> u_source_texel: vec4<f32>;

// void main()  ->  fs_downsample; texture2D  ->  textureSample
@fragment
fn fs_downsample(in: PostVaryings) -> @location(0) vec4<f32>
{
	let offset = u_source_texel.xy * 0.5;
	var color = textureSample(s_accum, s_accum_sampler, in.v_texcoord0 + vec2<f32>(-offset.x, -offset.y)).rgb;
	color += textureSample(s_accum, s_accum_sampler, in.v_texcoord0 + vec2<f32>( offset.x, -offset.y)).rgb;
	color += textureSample(s_accum, s_accum_sampler, in.v_texcoord0 + vec2<f32>(-offset.x,  offset.y)).rgb;
	color += textureSample(s_accum, s_accum_sampler, in.v_texcoord0 + vec2<f32>( offset.x,  offset.y)).rgb;
	return vec4<f32>(color * 0.25, 1.0);
}
