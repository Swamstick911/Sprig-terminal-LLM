//! Build script: make `memory.x` visible to the linker.
//!
//! cortex-m-rt's `link.x` does `INCLUDE memory.x`, so the file must be on the
//! linker search path. We copy it into `OUT_DIR` (which is always on the path)
//! and tell Cargo to relink if it changes.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Copy memory.x into OUT_DIR so the linker can find it via -L.
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // Relink whenever the layout or the linker invocation changes.
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
