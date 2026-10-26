//! Compiles the FreeRDP C glue (`src/rdp/freerdp_bridge.c`) against FreeRDP 3 found via
//! pkg-config. A FreeRDP tree bundled at `./freerdp-3.32` (see `scripts/fetch-freerdp.sh`)
//! takes precedence over the system libraries. On macOS FreeRDP comes from Homebrew
//! (`brew install freerdp`), on Windows from vcpkg (`vcpkg install "freerdp[client]"`) with pkgconf.
//!
//! For the WinUI frontend on MSVC the application manifest is embedded in the executable.
//!
//! When cross-compiling for another OS without FreeRDP for that target (for example
//! `cargo check --target x86_64-pc-windows-msvc` on Linux), the C glue is skipped with a
//! warning: type checking works, linking does not.

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

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    embed_windows_manifest(&manifest_dir, &target_os);
    let host = env::var("HOST").unwrap_or_default();
    let cross_os = !match target_os.as_str() {
        "macos" => host.contains("darwin"),
        os => host.contains(os),
    };

    let probed: Result<Vec<pkg_config::Library>, (String, String)> =
        ["freerdp3", "freerdp-client3", "winpr3"]
            .iter()
            .map(|name| {
                // Link flags are emitted after the bridge is compiled (see `emit_link_flags`).
                pkg_config::Config::new()
                    .cargo_metadata(false)
                    .probe(name)
                    .map_err(|e| ((*name).to_string(), e.to_string()))
            })
            .collect();
    let libs = match probed {
        Ok(libs) => libs,
        Err((name, e)) if cross_os => {
            println!(
                "cargo:warning=FreeRDP for {target_os} not found ({name}: {e}); skipping the C bridge. Type checking works, linking will not."
            );
            return;
        }
        Err((name, e)) => panic!("{name} not found via pkg-config: {e}"),
    };

    // Non-system library directories (bundled tree, /usr/local, custom prefixes) need an
    // rpath so the binary runs without LD_LIBRARY_PATH.
    let mut rpaths: Vec<&PathBuf> = libs
        .iter()
        .flat_map(|lib| &lib.link_paths)
        .filter(|p| !is_system_lib_dir(p))
        .collect();
    rpaths.sort();
    rpaths.dedup();
    // Windows has no rpath: FreeRDP DLLs are copied next to the executable instead.
    if target_os == "windows" {
        rpaths.clear();
    }
    if !rpaths.is_empty() && target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    if target_os == "macos" {
        // Leaves room for scripts/bundle-macos-dylibs.py to rewrite library paths.
        println!("cargo:rustc-link-arg=-Wl,-headerpad_max_install_names");
    }
    let origin = if target_os == "macos" {
        "@executable_path"
    } else {
        "$ORIGIN"
    };
    for dir in rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        if let Ok(rel) = dir.strip_prefix(&manifest_dir) {
            // Lets a binary copied next to the bundled tree (target/<profile>/) find it.
            println!(
                "cargo:rustc-link-arg=-Wl,-rpath,{origin}/../../{}",
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
    emit_link_flags(&libs);
}

/// Links FreeRDP after the static bridge library. GNU ld resolves archives (including
/// Windows import libraries) left to right, so the libraries the bridge uses must follow it.
fn emit_link_flags(libs: &[pkg_config::Library]) {
    for lib in libs {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for path in &lib.framework_paths {
            println!("cargo:rustc-link-search=framework={}", path.display());
        }
        for name in &lib.libs {
            println!("cargo:rustc-link-lib={name}");
        }
        for name in &lib.frameworks {
            println!("cargo:rustc-link-lib=framework={name}");
        }
        for args in &lib.ld_args {
            println!("cargo:rustc-link-arg=-Wl,{}", args.join(","));
        }
    }
}

/// Embeds `packaging/windows/windows-app.manifest` (per-monitor DPI, Common Controls 6)
/// in the WinUI executable. MSVC links it directly; other toolchains need a resource
/// compiler, so they go without (the app still runs, with system DPI scaling).
fn embed_windows_manifest(manifest_dir: &Path, target_os: &str) {
    if target_os != "windows" || env::var_os("CARGO_FEATURE_WINUI").is_none() {
        return;
    }
    let manifest = manifest_dir.join("packaging/windows/windows-app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
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
