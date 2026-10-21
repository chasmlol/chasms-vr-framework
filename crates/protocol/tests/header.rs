//! Keeps `include/rustvr_protocol.h` in step with the Rust layout. Run with `RUSTVR_BLESS=1` to
//! rewrite the header after changing the protocol.

use std::path::Path;

#[test]
fn c_header_is_current() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header_path = crate_dir.join("include").join("rustvr_protocol.h");
    let config = cbindgen::Config {
        language: cbindgen::Language::C,
        include_guard: Some("RUSTVR_PROTOCOL_H".into()),
        header: Some(
            "/* RustVR shared-memory protocol. Generated from crates/protocol (cbindgen): do not edit.\n \
             * See crates/protocol/src/lib.rs for what every field means. */"
                .into(),
        ),
        sys_includes: vec!["stdint.h".into()],
        no_includes: true,
        cpp_compat: true,
        usize_is_size_t: true,
        enumeration: cbindgen::EnumConfig { prefix_with_name: true, ..Default::default() },
        export: cbindgen::ExportConfig {
            prefix: Some("RVR_".into()),
            exclude: vec!["Link".into(), "Security".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let bindings = cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen");
    let mut generated = Vec::new();
    bindings.write(&mut generated);
    let generated = String::from_utf8(generated).unwrap();
    let current = std::fs::read_to_string(&header_path).unwrap_or_default();
    if std::env::var_os("RUSTVR_BLESS").is_some() || current.is_empty() {
        std::fs::create_dir_all(header_path.parent().unwrap()).unwrap();
        std::fs::write(&header_path, &generated).unwrap();
        return;
    }
    assert!(
        current.replace("\r\n", "\n") == generated.replace("\r\n", "\n"),
        "include/rustvr_protocol.h is out of date: rerun this test with RUSTVR_BLESS=1"
    );
}
