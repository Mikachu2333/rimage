# Prepends the MSVC toolchain and Windows SDK to PATH/LIB for this shell only.
#
# Why this exists: Git Bash ships GNU coreutils `link.exe`, which shadows MSVC's
# linker and makes every `cargo build` fail with "link: extra operand". The MSVC
# environment (INCLUDE/LIB/PATH) is normally set up by vcvarsall.bat, which is a
# cmd.exe script and is therefore awkward to use from bash. This file sets the
# same variables directly.
#
# It only affects the current shell. Nothing is written to the persistent
# environment, the registry, or any profile.
#
# The MSVC and SDK versions are auto-detected from the filesystem. If the
# detection fails, override them by setting RIMAGE_MSVC_ROOT and
# RIMAGE_SDK_VER before sourcing this file.
#
# Usage: source ci/msvc-env.sh

_sdk_root='/c/Program Files (x86)/Windows Kits/10'

# --- Auto-detect the MSVC tools root --------------------------------------
# Looks for the highest-versioned directory under the VS BuildTools path.
# Override: set RIMAGE_MSVC_ROOT to the full MSVC tools path before sourcing.
if [ -z "${RIMAGE_MSVC_ROOT:-}" ]; then
    _vs_base='/c/Program Files (x86)/Microsoft Visual Studio'
    _msvc_root=""
    if [ -d "$_vs_base" ]; then
        # Search for any edition (BuildTools, Community, Professional, Enterprise)
        # under VS 17 or 18, then pick the highest-numbered MSVC version.
        for _edition in "$_vs_base"/*/BuildTools \
                        "$_vs_base"/*/Community \
                        "$_vs_base"/*/Professional \
                        "$_vs_base"/*/Enterprise; do
            _vc_tools="$_edition/VC/Tools/MSVC"
            if [ -d "$_vc_tools" ]; then
                # Pick the highest version directory (lexicographic = numeric
                # for dotted version strings like 14.51.36231).
                _found=$(ls -1 "$_vc_tools" 2>/dev/null | sort -V | tail -1)
                if [ -n "$_found" ]; then
                    _msvc_root="$_vc_tools/$_found"
                    break
                fi
            fi
        done
    fi
    if [ -z "$_msvc_root" ]; then
        echo "msvc-env: could not auto-detect MSVC tools root." >&2
        echo "msvc-env: set RIMAGE_MSVC_ROOT to the full path, e.g.:" >&2
        echo "msvc-env:   export RIMAGE_MSVC_ROOT='/c/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.51.36231'" >&2
        return 1 2>/dev/null || exit 1
    fi
else
    _msvc_root="$RIMAGE_MSVC_ROOT"
fi

# --- Auto-detect the Windows SDK version ---------------------------------
# Override: set RIMAGE_SDK_VER to the desired SDK version string.
if [ -z "${RIMAGE_SDK_VER:-}" ]; then
    _sdk_ver=""
    if [ -d "$_sdk_root/Lib" ]; then
        # Pick the highest version that has both Lib and bin directories.
        _sdk_ver=$(ls -1 "$_sdk_root/Lib" 2>/dev/null | sort -V | tail -1)
    fi
    if [ -z "$_sdk_ver" ]; then
        echo "msvc-env: could not auto-detect Windows SDK version under $_sdk_root." >&2
        echo "msvc-env: set RIMAGE_SDK_VER to the desired version, e.g.:" >&2
        echo "msvc-env:   export RIMAGE_SDK_VER='10.0.26100.0'" >&2
        return 1 2>/dev/null || exit 1
    fi
else
    _sdk_ver="$RIMAGE_SDK_VER"
fi

echo "msvc-env: detected MSVC root: $_msvc_root"
echo "msvc-env: detected SDK ver:    $_sdk_ver"

for _dir in \
    "$_msvc_root/bin/Hostx64/x64" \
    "$_msvc_root/lib/x64" \
    "$_sdk_root/Lib/$_sdk_ver/ucrt/x64" \
    "$_sdk_root/Lib/$_sdk_ver/um/x64" \
    "$_sdk_root/bin/$_sdk_ver/x64"
do
    if [ ! -d "$_dir" ]; then
        echo "msvc-env: missing directory: $_dir" >&2
        return 1 2>/dev/null || exit 1
    fi
done

export PATH="$_sdk_root/bin/$_sdk_ver/x64:$_msvc_root/bin/Hostx64/x64:$PATH"

# link.exe cannot read Git Bash's /c/... paths, so LIB and INCLUDE must use
# native Windows paths with backslashes. `cygpath -w` does the conversion.
_win() { cygpath -w "$1"; }

# LIB tells link.exe where to resolve kernel32.lib & friends.
export LIB="$(_win "$_msvc_root/lib/x64");$(_win "$_sdk_root/Lib/$_sdk_ver/ucrt/x64");$(_win "$_sdk_root/Lib/$_sdk_ver/um/x64")${LIB:+;$LIB}"

# INCLUDE is needed when a -sys crate compiles C sources.
export INCLUDE="$(_win "$_msvc_root/include");$(_win "$_sdk_root/Include/$_sdk_ver/ucrt");$(_win "$_sdk_root/Include/$_sdk_ver/um");$(_win "$_sdk_root/Include/$_sdk_ver/shared")${INCLUDE:+;$INCLUDE}"

# winresource (build.rs of this crate) resolves rc.exe from its own toolkit_path
# instead of PATH, and derives that path from registry lookups that do not work
# reliably here. RC_PATH short-circuits that lookup with an explicit path.
export RC_PATH="$(_win "$_sdk_root/bin/$_sdk_ver/x64/rc.exe")"

_RC_PATH="$RC_PATH"
unset _vs_base _edition _vc_tools _found _msvc_root _sdk_root _sdk_ver _dir
unset RIMAGE_MSVC_ROOT RIMAGE_SDK_VER 2>/dev/null || true

echo "msvc-env: link -> $(command -v link)"
echo "msvc-env: rc   -> $_RC_PATH"
unset _RC_PATH
