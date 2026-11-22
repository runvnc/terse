// Tersenet: the interface between vs_beam.wgsl and fs_beam.wgsl, written for
// this translation (Apache-2.0, like the rest of Tersenet). It stands in for
// bgfx's uniforms and for MAME's varying.def.sc, which was not copied (it has
// no licence header): the names are the ones the two BSD-3-Clause shaders use.

// bgfx's predefined u_viewProj and the beam effect's two uniforms
// (bgfx/effects/vector-crt/beam.json).
struct BeamUniforms {
    u_viewProj: mat4x4<f32>,
    // x = frame interval (seconds), y = phosphor persistence (seconds),
    // z = beam-energy gain, w = halo strength.
    u_vector_params: vec4<f32>,
    u_target_dims: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: BeamUniforms;

// The unit quad's corner, and the three per-instance vectors packed by
// beam_instance in vectorrenderer.cpp (i_data0..2).
struct BeamInput {
    @location(0) a_position: vec2<f32>,
    @location(1) i_data0: vec4<f32>,
    @location(2) i_data1: vec4<f32>,
    @location(3) i_data2: vec4<f32>,
    // Tersenet: i_data2.w is the intensity at the segment's end (MAME's
    // unused); i_data3.xy are 1 where the start, the end, is a butt end.
    @location(4) i_data3: vec4<f32>,
}

struct BeamVaryings {
    @builtin(position) gl_Position: vec4<f32>,
    @location(0) v_beam: vec4<f32>,
    @location(1) v_beam_color: vec4<f32>,
    @location(2) v_beam_timing: vec4<f32>,
    // Tersenet: the end's intensity, the butt ends.
    @location(3) v_beam_ends: vec4<f32>,
}
