// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/vs_post.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Generates a full-screen post-processing quad and forwards its texture
// coordinates for the decay, downsample, blur, and composite passes.

// void main()  ->  vs_post
@vertex
fn vs_post(in: PostInput) -> PostVaryings
{
	var out: PostVaryings;
	out.gl_Position = vec4<f32>(in.a_position.x * 2.0 - 1.0, 1.0 - in.a_position.y * 2.0, 0.0, 1.0);
	out.v_texcoord0 = in.a_texcoord0;
	return out;
}
