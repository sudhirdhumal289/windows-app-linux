//! Compiles the FreeRDP C glue (`src/rdp/freerdp_bridge.c`) against FreeRDP 3 found via
//! pkg-config. A FreeRDP tree bundled at `./freerdp-3.32` (see `scripts/fetch-freerdp.sh`)
//! takes precedence over the system libraries. On macOS FreeRDP comes from Homebrew
//! (`brew install freerdp`). On Windows the first build downloads a prebuilt FreeRDP SDK
//! (pinned in `packaging/windows/freerdp-sdk.txt`), and the FreeRDP DLLs are copied next
//! to the executable, so `cargo build` / `cargo run` need no setup.
//!
//! For the WinUI frontend on MSVC the application manifest is embedded in the executable,
//! and the Windows App SDK runtime is bundled next to it (`windows-reactor-setup`).
//!
//! When cross-compiling for another OS without FreeRDP for that target (for example
//! `cargo check --target x86_64-pc-windows-msvc` on Linux), the C glue is skipped with a
//! warning: type checking works, linking does not.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let runtime_bundled = bundle_windows_app_sdk(&target_os);
    embed_windows_manifest(&manifest_dir, &target_os, runtime_bundled);
    let host = env::var("HOST").unwrap_or_default();
    let cross_os = !match target_os.as_str() {
        "macos" => host.contains("darwin"),
        os => host.contains(os),
    };
    if target_os == "windows" && !cross_os {
        use_windows_freerdp(&manifest_dir);
    }

    let probed: Result<Vec<pkg_config::Library>, (String, String)> =
        ["freerdp3", "freerdp-client3", "winpr3"]
            .iter()
            .map(|pkg| {
                pkg_config::Config::new()
                    .atleast_version("3.0")
                    .probe(pkg)
                    .map_err(|e| ((*pkg).to_string(), e.to_string()))
            })
            .collect();
    let libs = match probed {
        Ok(libs) => libs,
        Err((pkg, e)) if cross_os => {
            println!(
                "cargo:warning=Skipping FreeRDP bridge for cross target {target_os}: {pkg} not found ({e})."
            );
            return;
        }
        Err((pkg, e)) => {
            panic!(
                "\n\
                ========================================================================\n\
                Required FreeRDP 3 library '{pkg}' was not found.\n\
                pkg-config error: {e}\n\
                ========================================================================\n"
            );
        }
    };

    let mut rpaths: Vec<PathBuf> = libs
        .iter()
        .flat_map(|lib| &lib.link_paths)
        .filter(|&p| !is_system_lib_dir(p))
        .cloned()
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
    for dir in &rpaths {
        if target_os == "macos" {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        } else {
            println!("cargo:rustc-link-arg=-Wl,-rpath={}", dir.display());
        }
    }
    // Origin-relative rpath on Linux so the binary finds sibling libraries.
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath=$ORIGIN/../lib/windows-app");
        println!("cargo:rustc-link-arg=-Wl,-rpath=$ORIGIN/../lib/windows-app-linux");
        println!("cargo:rustc-link-arg=-Wl,-rpath=$ORIGIN/../lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath=$ORIGIN");
    }

    let mut builder = cc::Build::new();
    builder
        .file("src/rdp/freerdp_bridge.c")
        .warnings(true)
        .extra_warnings(true);
    for (key, value) in libs.iter().flat_map(|lib| &lib.defines) {
        match value {
            Some(v) => builder.define(key, v.as_str()),
            None => builder.define(key, None),
        };
    }
    for path in libs.iter().flat_map(|lib| &lib.include_paths) {
        builder.include(path);
    }
    builder.compile("freerdp_bridge");
    emit_link_flags(&libs);
    if target_os == "windows" {
        copy_windows_dlls(&libs);
    } else if target_os == "linux" {
        copy_linux_so(&libs);
    }
}

/// Links FreeRDP after the static bridge library. GNU ld resolves archives (including
/// Windows import libraries) left to right, so the libraries the bridge uses must follow it.
fn emit_link_flags(libs: &[pkg_config::Library]) {
    for lib in libs {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &lib.libs {
            println!("cargo:rustc-link-lib={name}");
        }
        for flag in &lib.ld_args {
            println!("cargo:rustc-link-arg=-Wl,{}", flag.join(","));
        }
    }
}

/// Embeds `packaging/windows/windows-app.manifest` (per-monitor DPI, Common Controls 6)
/// in the WinUI executable. MSVC links it directly; other toolchains need a resource
/// compiler, so they go without (the app still runs, with system DPI scaling). When the
/// Windows App SDK runtime is bundled, the linker merges it with that runtime's manifest.
fn embed_windows_manifest(manifest_dir: &Path, target_os: &str, runtime_bundled: bool) {
    if target_os != "windows" || env::var_os("CARGO_FEATURE_WINUI").is_none() {
        return;
    }
    let manifest = manifest_dir.join("packaging/windows/windows-app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        if !runtime_bundled {
            println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        }
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}

/// Makes WinUI builds self-contained: the Windows App SDK runtime (downloaded from NuGet
/// once) is copied next to the executable and registered in its manifest, so users do not
/// install it. Needs a Windows host and an MSVC-style linker; returns whether it ran.
fn bundle_windows_app_sdk(target_os: &str) -> bool {
    if target_os != "windows" || env::var_os("CARGO_FEATURE_WINUI").is_none() {
        return false;
    }
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    if cfg!(windows) && (target_env == "msvc" || (target_env == "gnu" && target_abi == "llvm")) {
        #[cfg(windows)]
        windows_reactor_setup::as_self_contained();
        return true;
    }
    println!(
        "cargo:warning=The Windows App SDK runtime is not bundled (needs an MSVC build on Windows); install it to run the app."
    );
    false
}

/// Points pkg-config at FreeRDP for Windows, in this order:
/// 1. `PKG_CONFIG_PATH`, if set (FreeRDP from elsewhere, such as MSYS2);
/// 2. an existing vcpkg install in `vcpkg_installed/` (or `VCPKG_INSTALLED_DIR`) or in the
///    `installed` tree of `VCPKG_ROOT`;
/// 3. the prebuilt SDK pinned in `packaging/windows/freerdp-sdk.txt`, downloaded once.
fn use_windows_freerdp(manifest_dir: &Path) {
    for var in [
        "PKG_CONFIG",
        "VCPKG_ROOT",
        "VCPKG_INSTALLED_DIR",
        "VCPKG_DEFAULT_TRIPLET",
        "WINDOWS_APP_NO_PREBUILT",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed={SDK_PIN_FILE}");
    if env::var_os("PKG_CONFIG_PATH").is_some_and(|p| !p.is_empty()) {
        return;
    }
    let triplet = vcpkg_triplet();
    let install_root = env::var_os("VCPKG_INSTALLED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("vcpkg_installed"));
    let found = std::iter::once(install_root.join(&triplet))
        .chain(
            vcpkg_roots()
                .into_iter()
                .map(|root| root.join("installed").join(&triplet)),
        )
        .find(|dir| has_freerdp(dir));
    let prebuilt = || {
        if env::var_os("WINDOWS_APP_NO_PREBUILT").is_some() {
            return None;
        }
        let pin = fs::read_to_string(manifest_dir.join(SDK_PIN_FILE)).ok()?;
        let (url, sha256) = sdk_pin(&pin, &triplet)?;
        fetch_prebuilt_sdk(url, sha256)
            .map_err(|e| {
                println!("cargo:warning=Prebuilt FreeRDP SDK download failed: {e}");
            })
            .ok()
    };
    let installed = match found.or_else(prebuilt) {
        Some(dir) => dir,
        None => {
            panic!(
                "\n\
                ========================================================================\n\
                FreeRDP 3 development libraries were not found for target '{triplet}'.\n\
                \n\
                Options to resolve:\n\
                1. Ensure network access to automatically download the prebuilt SDK\n\
                   pinned in '{SDK_PIN_FILE}'.\n\
                2. Or set PKG_CONFIG_PATH pointing to your FreeRDP 3 pkgconfig directory.\n\
                3. Or build FreeRDP with vcpkg: 'vcpkg install --triplet={triplet}'.\n\
                ========================================================================\n"
            );
        }
    };
    let pc_dir = installed.join("lib").join("pkgconfig");
    println!(
        "cargo:rerun-if-changed={}",
        pc_dir.join("freerdp3.pc").display()
    );
    env::set_var("PKG_CONFIG_PATH", &pc_dir);
    let pkgconf = installed.join("tools").join("pkgconf").join("pkgconf.exe");
    if env::var_os("PKG_CONFIG").is_none() && pkgconf.is_file() {
        env::set_var("PKG_CONFIG", pkgconf);
    }
}

const SDK_PIN_FILE: &str = "packaging/windows/freerdp-sdk.txt";

fn has_freerdp(dir: &Path) -> bool {
    dir.join("lib/pkgconfig/freerdp3.pc").is_file()
}

/// The download URL and SHA-256 pinned for `triplet` (`<triplet> <url> <sha256>` lines).
fn sdk_pin<'a>(pin: &'a str, triplet: &str) -> Option<(&'a str, &'a str)> {
    pin.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == triplet).then_some(())?;
            let (url, sha256) = (fields.next()?, fields.next()?);
            let valid = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
            valid.then_some((url, sha256))
        })
}

/// Downloads and unpacks a prebuilt FreeRDP SDK (a zipped vcpkg install tree) into
/// `%LOCALAPPDATA%\windows-app-linux\freerdp-sdk\<sha256>`, after checking its SHA-256.
/// Uses the `curl.exe`, `tar.exe` and `certutil.exe` that ship with Windows 10 and later.
fn fetch_prebuilt_sdk(url: &str, sha256: &str) -> Result<PathBuf, String> {
    let sha256 = sha256.to_ascii_lowercase();
    let cache = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("windows-app")
        .join("freerdp-sdk");
    let dir = cache.join(&sha256);
    if has_freerdp(&dir) {
        return Ok(dir);
    }
    fs::create_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let zip = cache.join(format!("{sha256}.zip"));
    let staging = cache.join(format!("{sha256}.partial"));
    let _ = fs::remove_dir_all(&staging);
    let result = (|| {
        eprintln!("Downloading the prebuilt FreeRDP SDK from {url}");
        quiet(
            Command::new("curl.exe")
                .args(["-fsSL", "--retry", "3", "-o"])
                .arg(&zip)
                .arg(url),
        )?;
        let actual = file_sha256(&zip)?;
        if actual != sha256 {
            return Err(format!(
                "checksum mismatch: expected {sha256}, got {actual}"
            ));
        }
        fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        quiet(
            Command::new("tar.exe")
                .arg("-xf")
                .arg(&zip)
                .arg("-C")
                .arg(&staging),
        )?;
        if !has_freerdp(&staging) {
            return Err("the archive has no lib/pkgconfig/freerdp3.pc".into());
        }
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&staging, &dir).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(&zip);
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|()| dir)
}

/// SHA-256 of `path` as lowercase hex, via `certutil -hashfile`.
fn file_sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("certutil.exe")
        .arg("-hashfile")
        .arg(path)
        .arg("SHA256")
        .output()
        .map_err(|e| format!("certutil: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(|line| line.replace(' ', "").to_ascii_lowercase())
        .find(|line| line.len() == 64 && line.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("certutil: unexpected output: {text}"))
}

/// Runs a helper command, sending its output to the build log; errors instead of panicking.
fn quiet(command: &mut Command) -> Result<(), String> {
    let status = command
        .stdout(std::io::stderr())
        .status()
        .map_err(|e| format!("{command:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} failed ({status})"))
    }
}

fn vcpkg_triplet() -> String {
    if let Ok(triplet) = env::var("VCPKG_DEFAULT_TRIPLET") {
        return triplet;
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("gnu") => format!("{arch}-mingw-dynamic"),
        _ => format!("{arch}-windows"),
    }
}

/// vcpkg checkouts named by `VCPKG_ROOT` and, on GitHub runners, `VCPKG_INSTALLATION_ROOT`.
fn vcpkg_roots() -> Vec<PathBuf> {
    ["VCPKG_ROOT", "VCPKG_INSTALLATION_ROOT"]
        .iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .collect()
}

/// Windows has no rpath: copies the DLLs and provider modules of the libraries FreeRDP was found in
/// (the `bin` directory beside each `lib`) next to the executables and test binaries.
fn copy_windows_dlls(libs: &[pkg_config::Library]) {
    let Some(profile_dir) = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| out.ancestors().nth(3).map(Path::to_path_buf))
    else {
        return;
    };
    let mut bin_dirs: Vec<PathBuf> = libs
        .iter()
        .flat_map(|lib| &lib.link_paths)
        .filter_map(|lib_dir| Some(lib_dir.parent()?.join("bin")))
        .filter(|dir| dir.is_dir())
        .collect();
    bin_dirs.sort();
    bin_dirs.dedup();
    for bin in bin_dirs {
        let Ok(entries) = fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dll = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
            if is_dll {
                for dest in [&profile_dir, &profile_dir.join("deps")] {
                    let target = dest.join(entry.file_name());
                    if fs::create_dir_all(dest).is_ok() && fs::copy(&path, &target).is_err() {
                        // Usually the app is running and holds the DLL; the old copy stays.
                        println!("cargo:warning=could not update {}", target.display());
                    }
                }
            } else if path.is_dir() && entry.file_name() == "ossl-modules" {
                // Copy OpenSSL 3 provider modules (e.g. legacy.dll for NTLM MD4 support)
                for dest in [&profile_dir, &profile_dir.join("deps")] {
                    let target_dir = dest.join("ossl-modules");
                    let _ = fs::create_dir_all(&target_dir);
                    if let Ok(mod_entries) = fs::read_dir(&path) {
                        for mod_entry in mod_entries.flatten() {
                            let _ =
                                fs::copy(mod_entry.path(), target_dir.join(mod_entry.file_name()));
                        }
                    }
                }
            }
        }
    }
}

/// Copies non-system shared libraries next to executables and test binaries so that
/// origin-relative RPATH ($ORIGIN) resolves bundled FreeRDP without requiring LD_LIBRARY_PATH.
fn copy_linux_so(libs: &[pkg_config::Library]) {
    let Some(profile_dir) = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out| out.ancestors().nth(3).map(Path::to_path_buf))
    else {
        return;
    };
    for lib in libs {
        for lib_dir in &lib.link_paths {
            if is_system_lib_dir(lib_dir) {
                continue;
            }
            let Ok(entries) = fs::read_dir(lib_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.contains(".so") {
                    for dest in [&profile_dir, &profile_dir.join("deps")] {
                        let target = dest.join(&name);
                        let _ = fs::create_dir_all(dest);
                        let _ = fs::remove_file(&target);
                        #[cfg(unix)]
                        if let Ok(link_dest) = fs::read_link(&path) {
                            let _ = std::os::unix::fs::symlink(&link_dest, &target);
                        } else {
                            let _ = fs::copy(&path, &target);
                        }
                        #[cfg(not(unix))]
                        let _ = fs::copy(&path, &target);
                    }
                }
            }
        }
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
        if pc_dir.is_dir() {
            println!("cargo:rerun-if-changed={}", pc_dir.display());
            let current = env::var_os("PKG_CONFIG_PATH").unwrap_or_default();
            let mut paths = env::split_paths(&current).collect::<Vec<_>>();
            paths.insert(0, pc_dir);
            if let Ok(joined) = env::join_paths(paths) {
                env::set_var("PKG_CONFIG_PATH", joined);
            }
            break;
        }
    }
}

fn is_system_lib_dir(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    SYSTEM_LIB_DIRS.iter().any(|&sys| {
        path_str == sys
            || path_str.strip_prefix(sys).is_some_and(|rest| {
                rest.starts_with('/')
                    && !rest.contains("/app/")
                    && !rest.contains("/opt/")
                    && !rest.contains("/freerdp")
            })
    })
}
