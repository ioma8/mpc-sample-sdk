#!/bin/sh
# Build one MPC Sample mod as an aarch64 cdylib, check it, and optionally pack it
# as an SD autorun zip.  The script is intentionally repo-independent: a mod
# points it at its own Cargo manifest and package.
set -eu

usage() {
    cat <<'EOF'
usage: build-mod.sh --package NAME [options]

required:
  --package NAME            Cargo package that builds the mod cdylib

options:
  --manifest-path PATH      Cargo.toml to build (default: Cargo.toml)
  --name NAME               output basename (default: the package name)
  --output-dir DIR          output directory (default: build)
  --target TARGET           Rust target (default: aarch64-unknown-linux-gnu)
  --features FEATURES       Cargo features to enable
  --require-symbol SYMBOL   fail if the ELF has no such symbol (repeatable)
  --pack                    also write build/<name>.zip
  --firmware-sha256 HASH    first-4KiB app hash for the autorun guard; required
                            with --pack
  --display-name NAME       name shown in the autorun prompt (default: --name)
  --env KEY=VALUE           extra Environment= line in the autorun (repeatable)
  -h, --help                this help

The build uses cargo, uv, llvm-nm (or nm), llvm-objcopy, llvm-objdump, zip and a
`zig cc` aarch64 linker.  The SDK checkout's scripts/zig-cc-aarch64-linux-gnu
is used when no linker is already configured.
EOF
}

manifest=Cargo.toml
package=
name=
output_dir=build
target=aarch64-unknown-linux-gnu
features=
pack=0
firmware=
display=
symbols=
envs=

need_value() {
    [ "$#" -ge 2 ] || { echo "missing value for $1" >&2; exit 2; }
}

while [ "$#" -gt 0 ]; do
    case $1 in
        --manifest-path) need_value "$@"; manifest=$2; shift 2 ;;
        --package) need_value "$@"; package=$2; shift 2 ;;
        --name) need_value "$@"; name=$2; shift 2 ;;
        --output-dir) need_value "$@"; output_dir=$2; shift 2 ;;
        --target) need_value "$@"; target=$2; shift 2 ;;
        --features) need_value "$@"; features=$2; shift 2 ;;
        --require-symbol) need_value "$@"; symbols="$symbols $2"; shift 2 ;;
        --pack) pack=1; shift ;;
        --firmware-sha256) need_value "$@"; firmware=$2; shift 2 ;;
        --display-name) need_value "$@"; display=$2; shift 2 ;;
        --env)
            need_value "$@"
            case $2 in
                *=*) ;;
                *) echo "--env must be KEY=VALUE: $2" >&2; exit 2 ;;
            esac
            key=${2%%=*}
            value=${2#*=}
            case $key in
                *[!A-Za-z0-9_]*|'') echo "invalid --env key: $key" >&2; exit 2 ;;
            esac
            case $value in
                *[!A-Za-z0-9_.:/@+-]*) echo "unsafe --env value: $value" >&2; exit 2 ;;
            esac
            envs="$envs $2"
            shift 2
            ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[ -n "$package" ] || { echo "--package is required" >&2; exit 2; }
[ -f "$manifest" ] || { echo "missing manifest: $manifest" >&2; exit 1; }
[ -n "$name" ] || name=$package
[ -n "$display" ] || display=$name

case $name in
    *[!A-Za-z0-9._-]*) echo "invalid --name: $name" >&2; exit 2 ;;
esac
case $target in
    *[!A-Za-z0-9._-]*) echo "invalid --target: $target" >&2; exit 2 ;;
esac
if [ "$pack" = 1 ]; then
    case $firmware in
        *[!0-9a-fA-F]*|'') echo "--pack requires a 64-hex --firmware-sha256" >&2; exit 2 ;;
    esac
    [ "${#firmware}" = 64 ] || { echo "--firmware-sha256 must be 64 hex characters" >&2; exit 2; }
fi

sdk_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
command -v uv >/dev/null 2>&1 || { echo "uv is required for the SDK's Python build helpers" >&2; exit 1; }

# Cargo needs an aarch64 linker.  Respect an explicit configuration; otherwise
# use the installed wrapper or the one shipped with the SDK.
if [ -z "${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-}" ]; then
    if command -v zig-cc-aarch64-linux-gnu >/dev/null 2>&1; then
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=$(command -v zig-cc-aarch64-linux-gnu)
    elif [ -x "$sdk_dir/scripts/zig-cc-aarch64-linux-gnu" ]; then
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$sdk_dir/scripts/zig-cc-aarch64-linux-gnu"
    else
        echo "no zig-cc-aarch64-linux-gnu on PATH and no SDK linker wrapper" >&2
        exit 1
    fi
fi
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER

case $output_dir in
    /*) ;;
    *) output_dir=$(pwd)/$output_dir ;;
esac
mkdir -p "$output_dir"

echo "== cargo build ($package) =="
if [ -n "$features" ]; then
    cargo build --release --manifest-path "$manifest" --target "$target" -p "$package" --lib --features "$features"
else
    cargo build --release --manifest-path "$manifest" --target "$target" -p "$package" --lib
fi

plan=$(uv run --script "$sdk_dir/tools/build_plan.py" "$manifest" "$package")
target_dir=$(printf '%s\n' "$plan" | sed -n '1p')
lib_name=$(printf '%s\n' "$plan" | sed -n '2p')
input="$target_dir/$target/release/lib$lib_name.so"
[ -f "$input" ] || { echo "cargo did not produce $input" >&2; exit 1; }

if [ -n "${LLVM_NM:-}" ]; then
    nm_tool=$LLVM_NM
elif command -v llvm-nm >/dev/null 2>&1; then
    nm_tool=$(command -v llvm-nm)
elif [ -x /opt/homebrew/opt/llvm/bin/llvm-nm ]; then
    nm_tool=/opt/homebrew/opt/llvm/bin/llvm-nm
else
    nm_tool=nm
fi

elf="$output_dir/$name.elf"
so="$output_dir/$name.so"
cp "$input" "$elf"
echo "== symbol map =="
"$nm_tool" "$elf" | awk 'NF >= 3 { print $1 " " $3 }' > "$output_dir/$name.map"

for symbol in $symbols; do
    grep -Fq " $symbol" "$output_dir/$name.map" || {
        echo "required symbol missing from the build: $symbol" >&2
        exit 1
    }
done

echo "== unresolved imports =="
unresolved=$("$nm_tool" -D "$elf" | grep ' U ' | grep -v '@' || true)
if [ -n "$unresolved" ]; then
    echo "unresolved symbols in the build:" >&2
    printf '%s\n' "$unresolved" >&2
    exit 1
fi

if [ -n "${LLVM_OBJCOPY:-}" ]; then
    objcopy=$LLVM_OBJCOPY
elif command -v llvm-objcopy >/dev/null 2>&1; then
    objcopy=$(command -v llvm-objcopy)
elif [ -x /opt/homebrew/opt/llvm/bin/llvm-objcopy ]; then
    objcopy=/opt/homebrew/opt/llvm/bin/llvm-objcopy
else
    echo "llvm-objcopy is required to strip the library" >&2
    exit 1
fi
"$objcopy" --strip-all "$elf" "$so"

if [ -n "${LLVM_OBJDUMP:-}" ]; then
    objdump=$LLVM_OBJDUMP
elif command -v llvm-objdump >/dev/null 2>&1; then
    objdump=$(command -v llvm-objdump)
elif [ -x /opt/homebrew/opt/llvm/bin/llvm-objdump ]; then
    objdump=/opt/homebrew/opt/llvm/bin/llvm-objdump
else
    echo "llvm-objdump is required for the glibc version check" >&2
    exit 1
fi
version=$("$objdump" -T "$so" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -u | sort -t. -k2 -n | tail -1 || true)
if [ -n "$version" ]; then
    v=${version#GLIBC_}
    major=${v%%.*}
    minor=${v#*.}
    if [ "$major" -gt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -gt 39 ]; }; then
        echo "built against $version, newer than the MPC Sample's glibc 2.39" >&2
        exit 1
    fi
    echo "glibc symbol versions: highest $version (device has 2.39)"
else
    echo "glibc symbol versions: none (static link?)"
fi

if [ "$pack" = 1 ]; then
    echo "== autorun package =="
    stage="$output_dir/pkg"
    rm -rf "$stage"
    mkdir -p "$stage/autorun"
    cp "$so" "$stage/autorun/$name.so"
    # Env values were validated above and are safe to pass as separate args.
    # shellcheck disable=SC2086
    uv run --script "$sdk_dir/tools/package_autorun.py" "$stage/autorun" "$name" "$display" "$firmware" $envs
    rm -f "$output_dir/$name.zip"
    (cd "$stage" && zip -qr "$output_dir/$name.zip" autorun)
    echo "built $output_dir/$name.zip"
fi

echo "built $so ($(wc -c < "$so") bytes)"
[ ! -f "$output_dir/$name.map" ] || echo "map: $output_dir/$name.map"
