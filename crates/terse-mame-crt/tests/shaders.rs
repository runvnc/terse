//! The translated shaders parse and validate (naga, as wgpu would), with
//! no graphics adapter.

use terse_mame_crt::pipeline::shaders;

#[test]
fn translated_shaders_validate() {
    for (name, vs, fs, src) in shaders::programs() {
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for entry in [vs, fs] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "{name}: no entry point {entry}");
        }
    }
}

/// Every translated file keeps the original's licence and copyright lines
/// and says what it was translated from.
#[test]
fn translated_files_keep_their_notices() {
    for src in [shaders::VS_BEAM, shaders::FS_BEAM, shaders::VS_POST, shaders::FS_DECAY, shaders::FS_DOWNSAMPLE, shaders::FS_BLUR, shaders::FS_COMPOSITE] {
        assert!(src.starts_with("// license:BSD-3-Clause\n// copyright-holders:Hans Andersson\n// Translated from MAME "), "{}", &src[..80]);
        assert!(src.contains("for Tersenet"));
    }
}
