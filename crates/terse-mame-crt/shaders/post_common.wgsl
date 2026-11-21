// Tersenet: the interface of vs_post.wgsl and the four post-processing
// fragment shaders, written for this translation (Apache-2.0). It stands in
// for MAME's varying.def.sc (not copied: it has no licence header) with the
// names the BSD-3-Clause shaders use.

// The full-screen quad's vertex (MAME's ScreenVertex: position and texture
// coordinate; its colour is unused by these shaders).
struct PostInput {
    @location(0) a_position: vec2<f32>,
    @location(1) a_texcoord0: vec2<f32>,
}

struct PostVaryings {
    @builtin(position) gl_Position: vec4<f32>,
    @location(0) v_texcoord0: vec2<f32>,
}
