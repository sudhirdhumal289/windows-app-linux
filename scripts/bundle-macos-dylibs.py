#!/usr/bin/env python3
"""Copy every non-system dylib an app executable needs into Contents/Frameworks.

The release binary links FreeRDP (and through it ffmpeg, OpenSSL, SDL, X11, ...) from
Homebrew by absolute path. Without bundling, the app only starts on Macs that have the
exact same Homebrew packages installed; everywhere else dyld aborts the launch and macOS
reports "check with the developer to make sure Windows App works with this version of
macOS".

Every bundled library is referenced as @executable_path/../Frameworks/<name> and all
LC_RPATH entries are dropped, so nothing outside the bundle and the OS is loaded.
Rewriting load commands invalidates code signatures: sign the dylibs and the bundle
afterwards.

Usage: bundle-macos-dylibs.py <path/to/App.app/Contents/MacOS/executable>
Tools can be overridden with the OTOOL and INSTALL_NAME_TOOL environment variables.
"""

import os
import re
import shutil
import subprocess
import sys

OTOOL = os.environ.get("OTOOL", "otool")
INSTALL_NAME_TOOL = os.environ.get("INSTALL_NAME_TOOL", "install_name_tool")
SYSTEM_PREFIXES = ("/usr/lib/", "/System/")
BUNDLED_PREFIX = "@executable_path/../Frameworks/"


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def load_commands(path):
    """Returns (dylib id or None, {dependency install name: is weak}, rpaths) of a Mach-O file."""
    ident, deps, rpaths = None, {}, []
    cmd = None
    for line in run(OTOOL, "-l", path).splitlines():
        line = line.strip()
        if line.startswith("cmd "):
            cmd = line.split()[1]
        elif line.startswith(("name ", "path ")) and cmd:
            value = re.sub(r" \(offset \d+\)$", "", line.split(None, 1)[1])
            if cmd == "LC_ID_DYLIB":
                ident = value
            elif cmd in (
                "LC_LOAD_DYLIB",
                "LC_LOAD_WEAK_DYLIB",
                "LC_REEXPORT_DYLIB",
                "LC_LOAD_UPWARD_DYLIB",
                "LC_LAZY_LOAD_DYLIB",
            ):
                deps[value] = deps.get(value, True) and cmd == "LC_LOAD_WEAK_DYLIB"
            elif cmd == "LC_RPATH":
                rpaths.append(value)
            cmd = None
    return ident, deps, rpaths


def expand(path, loader_dir, exe_dir):
    if path.startswith("@loader_path"):
        return loader_dir + path[len("@loader_path") :]
    if path.startswith("@executable_path"):
        return exe_dir + path[len("@executable_path") :]
    return path


def resolve(name, loader_dir, rpath_dirs, exe_dir):
    """Finds the file an install name refers to, mirroring dyld's lookup."""
    if name.startswith("@rpath/"):
        rest = name[len("@rpath/") :]
        for directory in rpath_dirs:
            candidate = os.path.join(directory, rest)
            if os.path.exists(candidate):
                return os.path.realpath(candidate)
        return None
    candidate = expand(name, loader_dir, exe_dir)
    return os.path.realpath(candidate) if os.path.exists(candidate) else None


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    exe = os.path.abspath(sys.argv[1])
    exe_dir = os.path.dirname(exe)
    frameworks = os.path.normpath(os.path.join(exe_dir, "..", "Frameworks"))
    os.makedirs(frameworks, exist_ok=True)

    _, _, exe_rpaths = load_commands(exe)
    exe_rpath_dirs = [expand(r, exe_dir, exe_dir) for r in exe_rpaths]

    by_source = {}  # real source path -> bundled file name
    by_name = {}  # bundled file name -> real source path
    # (file to patch, directory it was loaded from originally)
    queue = [(exe, exe_dir)]
    patched = []

    while queue:
        target, origin_dir = queue.pop()
        ident, deps, rpaths = load_commands(target)
        # dyld searches the loading image's rpaths first, then the executable's.
        rpath_dirs = [expand(r, origin_dir, exe_dir) for r in rpaths] + exe_rpath_dirs
        args = []
        for dep, weak in deps.items():
            if dep.startswith(SYSTEM_PREFIXES) or dep.startswith(BUNDLED_PREFIX):
                continue
            source = resolve(dep, origin_dir, rpath_dirs, exe_dir)
            if source is None and weak:
                print(f"warning: skipping missing weak dependency {dep}")
                continue
            if source is None:
                sys.exit(f"error: {os.path.basename(target)} needs {dep}, which was not found")
            name = by_source.get(source)
            if name is None:
                name = os.path.basename(dep)
                if by_name.get(name, source) != source:
                    sys.exit(f"error: {by_name[name]} and {source} would both be bundled as {name}")
                by_source[source] = name
                by_name[name] = source
                dest = os.path.join(frameworks, name)
                shutil.copyfile(source, dest)
                os.chmod(dest, 0o755)
                print(f"bundled {name} <- {source}")
                queue.append((dest, os.path.dirname(source)))
            args += ["-change", dep, BUNDLED_PREFIX + name]
        if ident is not None:
            args += ["-id", BUNDLED_PREFIX + os.path.basename(target)]
        if args:
            run(INSTALL_NAME_TOOL, *args, target)
        # install_name_tool removes one LC_RPATH per path at a time; repeat for duplicates.
        while rpaths:
            delete = [a for r in dict.fromkeys(rpaths) for a in ("-delete_rpath", r)]
            run(INSTALL_NAME_TOOL, *delete, target)
            rpaths = load_commands(target)[2]
        patched.append(target)

    # Every remaining reference must point into the bundle or the OS.
    for target in patched:
        _, deps, rpaths = load_commands(target)
        stray = [d for d, weak in deps.items() if not weak and not d.startswith(SYSTEM_PREFIXES + (BUNDLED_PREFIX,))]
        if stray or rpaths:
            sys.exit(f"error: {target} still references {stray + rpaths}")
    print(f"bundled {len(by_source)} libraries into {frameworks}")


if __name__ == "__main__":
    main()
