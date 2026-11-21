// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/shaders/chains/vector-crt/fs_beam.sc
// (commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b) to WGSL for Tersenet,
// line for line; the original is in third_party/mame-vector-crt/shaders/.
// Rasterizes each vector as an HDR Gaussian beam, including intensity-dependent
// core/halo width and scan-order timing, into the phosphor accumulation buffer.

// x = frame interval (seconds), y = phosphor persistence (seconds),
// z = beam-energy gain, w = halo strength.
// (uniform vec4 u_vector_params, u_target_dims: in BeamUniforms, beam_common.wgsl)

// Integrate the core and halo over the approximate pixel footprint. Adding
// the box-filter variance to the Gaussian variance prevents subpixel beams
// from aliasing; scaling by sigma/filteredSigma preserves integrated energy.
// (GLSL's dFdx/dFdy  ->  WGSL's dpdx/dpdy.)
fn beam_response_filtered(along: f32, across: f32, beamLength: f32, coreSigma: f32, haloSigma: f32, haloStrength: f32) -> f32
{
	// Outside either endpoint, include longitudinal distance to produce round
	// caps. Between the endpoints only perpendicular distance contributes.
	let pastEndpoint = max(max(-along, along - beamLength), 0.0);
	let pixelAcross = length(vec2<f32>(dpdx(across), dpdy(across)));
	let pixelVariance = pixelAcross * pixelAcross / 12.0;
	let filteredCoreSigma = sqrt(coreSigma * coreSigma + pixelVariance);
	let filteredHaloSigma = sqrt(haloSigma * haloSigma + pixelVariance);
	let coreScale = coreSigma / filteredCoreSigma;
	let haloScale = haloSigma / filteredHaloSigma;
	let distanceSquared = across * across + pastEndpoint * pastEndpoint;

	return
		coreScale * exp(-0.5 * distanceSquared / (filteredCoreSigma * filteredCoreSigma)) +
		haloStrength * haloScale * exp(-0.5 * distanceSquared / (filteredHaloSigma * filteredHaloSigma));
}

// void main()  ->  fs_beam; gl_FragColor  ->  the return value
@fragment
fn fs_beam(in: BeamVaryings) -> @location(0) vec4<f32>
{
	let along = in.v_beam.x;
	let across = in.v_beam.y;
	let beamLength = in.v_beam.z;

	let intensity = max(in.v_beam_color.a, 0.0);
	let intensityResponse = sqrt(clamp(intensity, 0.0, 1.0));

	// Brighter vectors get a slightly wider core and a somewhat wider halo.
	// Keep the response conservative so bright text does not become swollen.
	let baseSigma = max(in.v_beam.w, 0.01);
	let coreSigma = baseSigma * mix(1.0, 1.12, intensityResponse);
	let haloSigma = baseSigma * 3.5 * mix(1.0, 1.25, intensityResponse);

	let haloStrength =
		u.u_vector_params.w * mix(0.6, 1.0, intensityResponse);

	// Widen each Gaussian to account for the pixel's width across the beam.
	// Reduce its peak by the same ratio so the total beam energy stays constant.
	let radial = beam_response_filtered(
		along, across, beamLength, coreSigma, haloSigma, haloStrength);

	// The quad includes padded caps, so clamp the beam-local coordinate to the
	// physical segment. Put a degenerate segment at its temporal midpoint.
	// (GLSL's ?:  ->  select(false_value, true_value, condition))
	let segmentPosition =
		select(0.5, clamp(along / beamLength, 0.0, 1.0), beamLength > 0.0001);

	// v_beam_timing.xy is the segment's start and duration as fractions of the
	// complete display-list scan. Convert the fragment's arrival time into its
	// age, in seconds, at the end of the current frame interval.
	let arrival = clamp(
		in.v_beam_timing.x + in.v_beam_timing.y * segmentPosition,
		0.0,
		1.0);

	let age = max(u.u_vector_params.x * (1.0 - arrival), 0.0);

	// Use the physical phosphor persistence when it is long enough. The ten-frame
	// floor prevents short persistence settings from erasing vectors drawn early
	// in this frame; inter-frame decay still uses the unmodified persistence.
	let scanPersistence = max(u.u_vector_params.y, u.u_vector_params.x * 10.0);
	let temporal = exp(-age / max(scanPersistence, 0.001));

	let energy =
		in.v_beam_color.rgb *
		intensity *
		radial *
		temporal *
		u.u_vector_params.z;

	return vec4<f32>(energy, 1.0);
}
