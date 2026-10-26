//! Compiles the FreeRDP C glue (`src/rdp/freerdp_bridge.c`) against FreeRDP 3 found via
//! pkg-config. A FreeRDP tree bundled at `./freerdp-3.32` (see `scripts/fetch-freerdp.sh`)
//! takes precedence over the system libraries. On macOS FreeRDP comes from Homebrew
//! (`brew install freerdp`). On Windows the first build downloads a prebuilt FreeRDP SDK
//! (pinned in `packaging/windows/freerdp-sdk.txt`), or else installs `vcpkg.json` with
//! vcpkg, and the FreeRDP DLLs are copied next to the executable, so `cargo build` /
//! `cargo run` need no setup.
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
    if target_os == "windows" {
        copy_windows_dlls(&libs);
    }
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
/// 3. the prebuilt SDK pinned in `packaging/windows/freerdp-sdk.txt`, downloaded once
///    (unless `WINDOWS_APP_NO_PREBUILT` is set);
/// 4. `vcpkg install` of `vcpkg.json` (unless `WINDOWS_APP_NO_VCPKG` is set).
fn use_windows_freerdp(manifest_dir: &Path) {
    for var in [
        "PKG_CONFIG",
        "VCPKG_ROOT",
        "VCPKG_INSTALLED_DIR",
        "VCPKG_DEFAULT_TRIPLET",
        "WINDOWS_APP_NO_PREBUILT",
        "WINDOWS_APP_NO_VCPKG",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=vcpkg.json");
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
                println!("cargo:warning=Prebuilt FreeRDP SDK unavailable ({e}); using vcpkg");
            })
            .ok()
    };
    let installed = match found.or_else(prebuilt) {
        Some(dir) => dir,
        None if env::var_os("WINDOWS_APP_NO_VCPKG").is_some() => return,
        None => {
            let vcpkg = find_or_fetch_vcpkg();
            run(
                Command::new(&vcpkg)
                    .current_dir(manifest_dir)
                    .arg("install")
                    .arg(format!("--triplet={triplet}"))
                    .arg(format!("--x-install-root={}", install_root.display())),
                "vcpkg install (first build only: compiles FreeRDP, ~15-30 minutes)",
            );
            install_root.join(&triplet)
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
        .join("windows-app-linux")
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

/// A vcpkg with a ports tree (the copy bundled with Visual Studio has none), or a private
/// clone in `%LOCALAPPDATA%\windows-app-linux\vcpkg`, created on first use.
fn find_or_fetch_vcpkg() -> PathBuf {
    if let Some(exe) = vcpkg_roots()
        .into_iter()
        .filter(|root| root.join("ports").is_dir())
        .map(|root| root.join("vcpkg.exe"))
        .find(|exe| exe.is_file())
    {
        return exe;
    }
    let cache = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let root = cache.join("windows-app-linux").join("vcpkg");
    let exe = root.join("vcpkg.exe");
    if exe.is_file() {
        return exe;
    }
    if !root.join("ports").is_dir() {
        let _ = fs::remove_dir_all(&root);
        run(
            Command::new("git")
                .args([
                    "clone",
                    "--depth=1",
                    "https://github.com/microsoft/vcpkg.git",
                ])
                .arg(&root),
            "git clone vcpkg (git is needed to fetch FreeRDP)",
        );
    }
    run(
        Command::new("cmd")
            .arg("/C")
            .arg(root.join("bootstrap-vcpkg.bat"))
            .arg("-disableMetrics"),
        "bootstrap-vcpkg",
    );
    exe
}

/// Runs a setup command, sending its output to the build log (stdout is reserved for
/// cargo directives); panics with `what` on failure.
fn run(command: &mut Command, what: &str) {
    let status = command
        .stdout(std::io::stderr())
        .status()
        .unwrap_or_else(|e| panic!("{what}: could not start: {e}"));
    assert!(status.success(), "{what} failed ({status})");
}

/// Windows has no rpath: copies the DLLs of the libraries FreeRDP was found in (the `bin`
/// directory beside each `lib`) next to the executables and test binaries.
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
            if !is_dll {
                continue;
            }
            for dest in [profile_dir.clone(), profile_dir.join("deps")] {
                let target = dest.join(entry.file_name());
                if fs::create_dir_all(&dest).is_ok() && fs::copy(&path, &target).is_err() {
                    // Usually the app is running and holds the DLL; the old copy stays.
                    println!("cargo:warning=could not update {}", target.display());
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
