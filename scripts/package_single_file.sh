#!/bin/zsh

# FlowFile Single-File Packaging Script for macOS
# Packages FlowFile into single-file deliverables:
# 1. Standalone binary executable (dist/flowfile)
# 2. Single macOS DMG Disk Image (dist/FlowFile-<version>-<arch>.dmg)
# 3. Single ZIP archive containing FlowFile.app (dist/FlowFile-<version>-<arch>.zip)

set -euo pipefail

script_dir=${0:A:h}
project_dir=${script_dir:h}
resources_dir=${project_dir}/resources
icon_source=${resources_dir}/FlowFile.svg
icon_output=${resources_dir}/FlowFile.icns
dist_dir=${project_dir}/dist

target_mode="all"
if [[ $# -gt 0 ]]; then
    case "$1" in
        --binary-only)
            target_mode="binary"
            ;;
        --dmg-only)
            target_mode="dmg"
            ;;
        --zip-only)
            target_mode="zip"
            ;;
        --help|-h)
            print "Usage: $0 [--binary-only | --dmg-only | --zip-only]"
            exit 0
            ;;
        *)
            print -u2 "Unknown argument: $1"
            print -u2 "Usage: $0 [--binary-only | --dmg-only | --zip-only]"
            exit 1
            ;;
    esac
fi

sdk_path=$(xcrun --sdk macosx --show-sdk-path)
arch="$(uname -m)"

case "${arch}" in
    arm64)
        export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="--target=arm64-apple-macos11 -isysroot ${sdk_path}"
        ;;
    x86_64)
        export BINDGEN_EXTRA_CLANG_ARGS_x86_64_apple_darwin="--target=x86_64-apple-macos11 -isysroot ${sdk_path}"
        ;;
    *)
        print -u2 "FlowFile packaging supports arm64 and x86_64 macOS hosts."
        exit 1
        ;;
esac

build_icon() {
    if [[ -f "${icon_output}" && "${icon_output}" -nt "${icon_source}" ]]; then
        print "Using existing icon: ${icon_output}"
        return 0
    fi

    print "Building macOS icon set from ${icon_source}..."
    local work_dir
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-icon.XXXXXX")
    local iconset=${work_dir}/FlowFile.iconset
    mkdir -p "${iconset}"

    /usr/bin/qlmanage -t -s 1024 -o "${work_dir}" "${icon_source}" >/dev/null 2>&1
    local rendered=${work_dir}/FlowFile.svg.png
    if [[ ! -f "${rendered}" ]]; then
        print -u2 "Quick Look could not render ${icon_source}."
        exit 1
    fi

    for spec in \
        "16 icon_16x16.png" \
        "32 icon_16x16@2x.png" \
        "32 icon_32x32.png" \
        "64 icon_32x32@2x.png" \
        "128 icon_128x128.png" \
        "256 icon_128x128@2x.png" \
        "256 icon_256x256.png" \
        "512 icon_256x256@2x.png" \
        "512 icon_512x512.png" \
        "1024 icon_512x512@2x.png"; do
        local dimension=${spec%% *}
        local filename=${spec#* }
        /usr/bin/sips -z "${dimension}" "${dimension}" "${rendered}" \
            --out "${iconset}/${filename}" >/dev/null
    done

    /usr/bin/iconutil -c icns "${iconset}" -o "${icon_output}"
    /bin/rm -R "${work_dir}"
    print "Generated icon: ${icon_output}"
}

cd "${project_dir}"
version=$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)

print "=== Packaging FlowFile v${version} (${arch}) ==="

build_icon

print "Compiling release binary..."
cargo build --release

mkdir -p "${dist_dir}"

# 1. Standalone Binary Executable
binary_src="${project_dir}/target/release/flowfile"
binary_dist="${dist_dir}/flowfile"

if [[ "${target_mode}" == "all" || "${target_mode}" == "binary" ]]; then
    print "Creating standalone executable: ${binary_dist}..."
    cp "${binary_src}" "${binary_dist}"
    if command -v strip >/dev/null 2>&1; then
        strip "${binary_dist}" 2>/dev/null || true
    fi
    chmod 755 "${binary_dist}"
fi

# Assemble .app Bundle if needed for DMG or ZIP or default packaging
app_path="${dist_dir}/FlowFile.app"
if [[ "${target_mode}" == "all" || "${target_mode}" == "dmg" || "${target_mode}" == "zip" ]]; then
    print "Assembling FlowFile.app bundle..."
    if [[ -d "${app_path}" ]]; then
        /bin/rm -R "${app_path}"
    fi
    mkdir -p "${app_path}/Contents/MacOS" "${app_path}/Contents/Resources"
    cp "${binary_src}" "${app_path}/Contents/MacOS/flowfile"
    cp "${resources_dir}/FlowFile.icns" "${app_path}/Contents/Resources/FlowFile.icns"
    cp "${resources_dir}/Info.plist" "${app_path}/Contents/Info.plist"
    chmod 755 "${app_path}/Contents/MacOS/flowfile"

    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${app_path}/Contents/Info.plist"

    "${script_dir}/sign_macos_app.sh" "${app_path}"
fi

# 2. DMG Disk Image
dmg_dist="${dist_dir}/FlowFile-${version}-${arch}.dmg"
if [[ "${target_mode}" == "all" || "${target_mode}" == "dmg" ]]; then
    print "Creating DMG disk image: ${dmg_dist}..."
    if [[ -f "${dmg_dist}" ]]; then
        /bin/rm "${dmg_dist}"
    fi
    dmg_stage=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-dmg.XXXXXX")
    cp -R "${app_path}" "${dmg_stage}/FlowFile.app"
    ln -s /Applications "${dmg_stage}/Applications"

    /usr/bin/hdiutil create -volname "FlowFile" -srcfolder "${dmg_stage}" \
        -ov -format UDZO "${dmg_dist}" >/dev/null
    /bin/rm -R "${dmg_stage}"

    if [[ -n "${FLOWFILE_NOTARY_PROFILE:-}" ]]; then
        xcrun notarytool submit "${dmg_dist}" \
            --keychain-profile "${FLOWFILE_NOTARY_PROFILE}" --wait
        xcrun stapler staple "${dmg_dist}"
    fi
fi

# 3. ZIP Archive
zip_dist="${dist_dir}/FlowFile-${version}-${arch}.zip"
if [[ "${target_mode}" == "all" || "${target_mode}" == "zip" ]]; then
    print "Creating ZIP package: ${zip_dist}..."
    if [[ -f "${zip_dist}" ]]; then
        /bin/rm "${zip_dist}"
    fi
    (cd "${dist_dir}" && zip -r -q "FlowFile-${version}-${arch}.zip" "FlowFile.app")
fi

print "\n=== Packaging Completed Successfully ==="
print "Output directory: ${dist_dir}"
if [[ -f "${binary_dist}" ]]; then
    print "  - Standalone Binary: ${binary_dist}"
fi
if [[ -f "${dmg_dist}" ]]; then
    print "  - macOS DMG Image:   ${dmg_dist}"
fi
if [[ -f "${zip_dist}" ]]; then
    print "  - macOS ZIP Bundle:  ${zip_dist}"
fi
