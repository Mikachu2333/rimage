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
# Usage: source ci/msvc-env.sh

_msvc_root='/c/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.51.36231'
_sdk_root='/c/Program Files (x86)/Windows Kits/10'
_sdk_ver='10.0.26100.0'

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
unset _msvc_root _sdk_root _sdk_ver _dir

echo "msvc-env: link -> $(command -v link)"
echo "msvc-env: rc   -> $_RC_PATH"
unset _RC_PATH
