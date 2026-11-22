// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/fs_decay.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Attenuates the previous HDR phosphor buffer over time to simulate the
// persistence and gradual decay of light emitted by a vector CRT phosphor.

// SAMPLER2D(s_accum, 0)  ->  a texture and its sampler
@group(0) @binding(0) var s_accum: texture_2d<f32>;
@group(0) @binding(1) var s_accum_sampler: sampler;
@group(0) @binding(2) var<uniform> u_decay: vec4<f32>;

// void main()  ->  fs_decay; texture2D  ->  textureSample
@fragment
fn fs_decay(in: PostVaryings) -> @location(0) vec4<f32>
{
	let previous = textureSample(s_accum, s_accum_sampler, in.v_texcoord0).rgb;
	return vec4<f32>(previous * u_decay.x, 1.0);
}
