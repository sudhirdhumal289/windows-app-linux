//! Compiles the FreeRDP C glue (`src/rdp/freerdp_bridge.c`) against FreeRDP 3 found via
//! pkg-config. A FreeRDP tree bundled at `./freerdp-3.32` (see `scripts/fetch-freerdp.sh`)
//! takes precedence over the system libraries.

use std::env;
use std::path::{Path, PathBuf};

const SYSTEM_LIB_DIRS: &[&str] = &["/usr/lib", "/usr/lib64", "/lib", "/lib64"];

fn main() {
    println!("cargo:rerun-if-changed=src/rdp/freerdp_bridge.c");
    println!("cargo:rerun-if-changed=src/rdp/freerdp_bridge.h");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    prefer_bundled_freerdp(&manifest_dir.join("freerdp-3.32"));

    let libs: Vec<pkg_config::Library> = ["freerdp3", "freerdp-client3", "winpr3"]
        .iter()
        .map(|name| {
            pkg_config::probe_library(name)
                .unwrap_or_else(|e| panic!("{name} not found via pkg-config: {e}"))
        })
        .collect();

    // Non-system library directories (bundled tree, /usr/local, custom prefixes) need an
    // rpath so the binary runs without LD_LIBRARY_PATH.
    let mut rpaths: Vec<&PathBuf> = libs
        .iter()
        .flat_map(|lib| &lib.link_paths)
        .filter(|p| !is_system_lib_dir(p))
        .collect();
    rpaths.sort();
    rpaths.dedup();
    if !rpaths.is_empty() {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    for dir in rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        if let Ok(rel) = dir.strip_prefix(&manifest_dir) {
            // Lets a binary copied next to the bundled tree (target/<profile>/) find it.
            println!(
                "cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../{}",
                rel.display()
            );
        }
    }

    let mut builder = cc::Build::new();
    builder
        .file("src/rdp/freerdp_bridge.c")
        .std("c11")
        .warnings(true)
        // FreeRDP's public headers trigger these; they are not actionable here.
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-missing-field-initializers");
    for path in libs.iter().flat_map(|lib| &lib.include_paths) {
        builder.include(path);
    }
    builder.compile("freerdp_bridge");
}

/// Puts the bundled FreeRDP's pkg-config directory first on `PKG_CONFIG_PATH`.
fn prefer_bundled_freerdp(root: &Path) {
    if !root.exists() {
        return;
    }
    let lib_root = root.join("lib");
    let candidates = [
        "x86_64-linux-gnu",
        "aarch64-linux-gnu",
        "arm-linux-gnueabihf",
        "",
    ];
    for triplet in candidates {
        let pc_dir = if triplet.is_empty() {
            lib_root.join("pkgconfig")
        } else {
            lib_root.join(triplet).join("pkgconfig")
        };
        if pc_dir.exists() {
            let existing = env::var("PKG_CONFIG_PATH").unwrap_or_default();
            let joined = if existing.is_empty() {
                pc_dir.display().to_string()
            } else {
                format!("{}:{existing}", pc_dir.display())
            };
            env::set_var("PKG_CONFIG_PATH", joined);
            return;
        }
    }
}

fn is_system_lib_dir(dir: &Path) -> bool {
    SYSTEM_LIB_DIRS
        .iter()
        .any(|sys| dir == Path::new(sys) || dir.parent() == Some(Path::new(sys)))
}
