// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/fs_blur.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Applies one axis of a separable Gaussian blur using texel-centred taps.

// SAMPLER2D(s_tex, 0)  ->  a texture and its sampler
@group(0) @binding(0) var s_tex: texture_2d<f32>;
@group(0) @binding(1) var s_tex_sampler: sampler;

// xy: one bloom-texture texel along the blur axis
// z:  Gaussian sigma in bloom-texture texels
@group(0) @binding(2) var<uniform> u_blur: vec4<f32>;

// void main()  ->  fs_blur; texture2D  ->  textureSample
@fragment
fn fs_blur(in: PostVaryings) -> @location(0) vec4<f32>
{
	let texel = u_blur.xy;
	let sigma = max(u_blur.z, 0.001);
	let denominator = 2.0 * sigma * sigma;

	let w0 = 1.0;
	let w1 = exp(-1.0 / denominator);
	let w2 = exp(-4.0 / denominator);
	let w3 = exp(-9.0 / denominator);
	let w4 = exp(-16.0 / denominator);

	let normalization = w0 + 2.0 * (w1 + w2 + w3 + w4);

	var color = textureSample(s_tex, s_tex_sampler, in.v_texcoord0).rgb * w0;

	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 + texel * 1.0).rgb * w1;
	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 - texel * 1.0).rgb * w1;

	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 + texel * 2.0).rgb * w2;
	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 - texel * 2.0).rgb * w2;

	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 + texel * 3.0).rgb * w3;
	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 - texel * 3.0).rgb * w3;

	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 + texel * 4.0).rgb * w4;
	color += textureSample(s_tex, s_tex_sampler, in.v_texcoord0 - texel * 4.0).rgb * w4;

	return vec4<f32>(color / normalization, 1.0);
}
