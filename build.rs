use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/rdp/freerdp_bridge.c");
    println!("cargo:rerun-if-changed=src/rdp/freerdp_bridge.h");

    // Dynamic resolution of project directory
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let bundled_freerdp = manifest_dir.join("freerdp-3.32");

    // Check for bundled FreeRDP (e.g., fetched via scripts/fetch-freerdp.sh)
    if bundled_freerdp.exists() {
        let arch_triplets = [
            "x86_64-linux-gnu",
            "aarch64-linux-gnu",
            "arm-linux-gnueabihf",
            "",
        ];
        for triplet in &arch_triplets {
            let lib_dir = if triplet.is_empty() {
                bundled_freerdp.join("lib")
            } else {
                bundled_freerdp.join("lib").join(triplet)
            };

            let pc_dir = lib_dir.join("pkgconfig");
            if pc_dir.exists() {
                let existing = env::var("PKG_CONFIG_PATH").unwrap_or_default();
                let new_val = if existing.is_empty() {
                    pc_dir.to_string_lossy().to_string()
                } else {
                    format!("{}:{}", pc_dir.display(), existing)
                };
                env::set_var("PKG_CONFIG_PATH", new_val);
            }

            if lib_dir.exists() {
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
                // Relative RPATH so binary can find bundled libraries if installed adjacent to freerdp-3.32
                if let Ok(rel) = lib_dir.strip_prefix(&manifest_dir) {
                    println!(
                        "cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../{}",
                        rel.display()
                    );
                }
            }
        }
    }

    // Add ~/.local/lib paths dynamically if they exist
    if let Ok(home) = env::var("HOME") {
        let home_path = PathBuf::from(home);
        let user_lib = home_path.join(".local").join("lib");
        if user_lib.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", user_lib.display());
            let user_arch = user_lib.join("x86_64-linux-gnu");
            if user_arch.exists() {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", user_arch.display());
            }
        }
    }

    let freerdp = pkg_config::probe_library("freerdp3").expect("freerdp3 library not found");
    let freerdp_client =
        pkg_config::probe_library("freerdp-client3").expect("freerdp-client3 library not found");
    let winpr = pkg_config::probe_library("winpr3").expect("winpr3 library not found");

    let mut builder = cc::Build::new();
    builder.file("src/rdp/freerdp_bridge.c");
    builder.warnings(false);

    for path in freerdp
        .include_paths
        .iter()
        .chain(freerdp_client.include_paths.iter())
        .chain(winpr.include_paths.iter())
    {
        builder.include(path);
    }

    builder.compile("freerdp_bridge");
}
