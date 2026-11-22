// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/vs_beam.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Expands each instanced vector segment into a padded screen-space quad and
// passes beam-local distance, color, width, and timing data to the beam shader.

// void main()  ->  vs_beam
@vertex
fn vs_beam(in: BeamInput) -> BeamVaryings {
	var out: BeamVaryings;
	// Per-instance data is packed by beam_instance in vectorrenderer.cpp:
	//   i_data0 = segment endpoints in target-pixel coordinates
	//   i_data1 = linear RGB and Gaussian sigma in pixels
	//   i_data2 = normalized scan start/duration, intensity, unused
	// Scan time is proportional to accumulated segment length, approximating a
	// beam that traverses the display list at constant deflection speed.
	let p0 = in.i_data0.xy;
	let p1 = in.i_data0.zw;
	let delta = p1 - p0;
	let beamLength = length(delta);
	// GLSL's ?: on vectors  ->  select(false_value, true_value, condition)
	let direction = select(vec2<f32>(1.0, 0.0), delta / beamLength, beamLength > 0.0001);
	let normal = vec2<f32>(-direction.y, direction.x);
	let sigma = max(in.i_data1.w, 0.01);

	// a_position describes a unit segment quad: x selects an endpoint and y is
	// the signed side. Extend it by six sigma (at least one pixel) so the quad
	// contains the Gaussian tails and round endpoint caps evaluated per pixel.
	let padding = max(sigma * 6.0, 1.0);
	let along = mix(-padding, beamLength + padding, in.a_position.x);
	let across = in.a_position.y * padding;
	let world = p0 + direction * along + normal * across;

	// mul(u_viewProj, v)  ->  u_viewProj * v
	out.gl_Position = u.u_viewProj * vec4<f32>(world, 0.0, 1.0);
	// Beam-local coordinates let the fragment shader evaluate distance without
	// depending on target orientation. Timing is constant for the whole instance.
	out.v_beam = vec4<f32>(along, across, beamLength, sigma);
	out.v_beam_color = vec4<f32>(in.i_data1.rgb, in.i_data2.z);
	out.v_beam_timing = in.i_data2;
	// Tersenet: the intensity at the end (i_data2.w, unused in MAME) and
	// the butt ends (i_data3.xy), for fs_beam.
	out.v_beam_ends = vec4<f32>(in.i_data2.w, in.i_data3.x, in.i_data3.y, 0.0);
	return out;
}
