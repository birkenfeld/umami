// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

fn main() {
    println!("cargo:rerun-if-env-changed=JUMPSD_LIB_DIR");

    // The "jumiom" feature calls into DriverJumiom's already-built,
    // system-installed libjumpsd.so (same as the legacy Jumiom C stack does,
    // e.g. Jumiom/LibHelper/Makefile's `-ljumpsd`); no vendored sources.
    if std::env::var_os("CARGO_FEATURE_JUMIOM").is_some() {
        // If not on a standard linker search path, JUMPSD_LIB_DIR can point
        // at the directory containing libjumpsd.so (or at the .so file
        // itself, for convenience).
        if let Some(dir) = std::env::var_os("JUMPSD_LIB_DIR") {
            let path = std::path::Path::new(&dir);
            let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
        println!("cargo:rustc-link-lib=dylib=jumpsd");
    }
}
