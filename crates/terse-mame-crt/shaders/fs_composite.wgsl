// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/fs_composite.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Combines persistent phosphor and bloom, then applies exposure, luminance
// tone mapping, gamma correction, and optional edge vignetting for display.

// SAMPLER2D(s_accum, 0), SAMPLER2D(s_bloom, 1)  ->  textures and a sampler
@group(0) @binding(0) var s_accum: texture_2d<f32>;
@group(0) @binding(1) var s_accum_sampler: sampler;
@group(0) @binding(2) var<uniform> u_composite: vec4<f32>;
@group(0) @binding(3) var s_bloom: texture_2d<f32>;

// void main()  ->  fs_composite; texture2D  ->  textureSample;
// vec3_splat(v)  ->  vec3<f32>(v); vec2_splat(v)  ->  vec2<f32>(v)
@fragment
fn fs_composite(in: PostVaryings) -> @location(0) vec4<f32>
{
	let phosphor = max(
			textureSample(s_accum, s_accum_sampler, in.v_texcoord0).rgb,
			vec3<f32>(0.0));

	let bloom = max(
			textureSample(s_bloom, s_accum_sampler, in.v_texcoord0).rgb,
			vec3<f32>(0.0));

	let hdr =
			(phosphor + bloom * u_composite.x) *
			u_composite.y;

	// Tone-map luminance rather than each RGB channel separately.
	// This retains the saturation and hue of bright vector colors.
	let luminance = dot(
			hdr,
			vec3<f32>(0.2126, 0.7152, 0.0722));

	let mappedLuminance =
			1.0 - exp(-luminance);

	// (GLSL's ?:  ->  select(false_value, true_value, condition))
	var mapped =
			select(
					vec3<f32>(0.0),
					hdr * (mappedLuminance / luminance),
					luminance > 0.0001);

	mapped = pow(
			max(mapped, vec3<f32>(0.0)),
			vec3<f32>(
					1.0 /
					max(u_composite.z, 0.001)));

	let edge =
			in.v_texcoord0 *
			(vec2<f32>(1.0) - in.v_texcoord0);

	let vignette =
			clamp(
					16.0 * edge.x * edge.y,
					0.0,
					1.0);

	mapped *= mix(
			1.0,
			vignette,
			u_composite.w);

	return vec4<f32>(mapped, 1.0);
}
