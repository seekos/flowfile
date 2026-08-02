#!/bin/zsh

set -euo pipefail

script_dir=${0:A:h}
project_dir=${script_dir:h}
resources_dir=${project_dir}/resources
icon_source=${resources_dir}/FlowFile.svg
icon_output=${resources_dir}/FlowFile.icns
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
icon_only=${1:-}

case "$(uname -m)" in
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
    local work_dir
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-icon.XXXXXX")
    local iconset=${work_dir}/FlowFile.iconset
    mkdir -p "${iconset}"
    /usr/bin/qlmanage -t -s 1024 -o "${work_dir}" "${icon_source}" >/dev/null 2>&1
    local rendered=${work_dir}/FlowFile.svg.png
    if [[ -z "${rendered}" ]]; then
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
}

cd "${project_dir}"
version=$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)
build_icon
if [[ "${icon_only}" == "--icon-only" ]]; then
    print "Generated ${icon_output}"
    exit 0
fi

app_path=${project_dir}/target/release/bundle/osx/FlowFile.app
if cargo bundle --help >/dev/null 2>&1; then
    cargo bundle --release
else
    print "cargo-bundle is unavailable; using the offline macOS bundle fallback."
    cargo build --release
    if [[ -d "${app_path}" ]]; then
        /bin/rm -R "${app_path}"
    fi
    mkdir -p "${app_path}/Contents/MacOS" "${app_path}/Contents/Resources"
    cp "${project_dir}/target/release/flowfile" "${app_path}/Contents/MacOS/flowfile"
    cp "${project_dir}/resources/FlowFile.icns" \
        "${app_path}/Contents/Resources/FlowFile.icns"
    cp "${project_dir}/resources/Info.plist" "${app_path}/Contents/Info.plist"
    chmod 755 "${app_path}/Contents/MacOS/flowfile"
fi
if [[ ! -d "${app_path}" ]]; then
    print -u2 "Expected application bundle not found at ${app_path}"
    exit 1
fi
/usr/libexec/PlistBuddy -c \
    "Set :CFBundleShortVersionString ${version}" "${app_path}/Contents/Info.plist"

"${script_dir}/sign_macos_app.sh" "${app_path}"

dmg_stage=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-dmg.XXXXXX")
cp -R "${app_path}" "${dmg_stage}/FlowFile.app"
ln -s /Applications "${dmg_stage}/Applications"
dmg_path=${project_dir}/target/release/FlowFile-${version}.dmg
/usr/bin/hdiutil create -volname "FlowFile" -srcfolder "${dmg_stage}" \
    -ov -format UDZO "${dmg_path}"
/bin/rm -R "${dmg_stage}"

if [[ -n "${FLOWFILE_NOTARY_PROFILE:-}" ]]; then
    xcrun notarytool submit "${dmg_path}" \
        --keychain-profile "${FLOWFILE_NOTARY_PROFILE}" --wait
    xcrun stapler staple "${dmg_path}"
fi

print "Application: ${app_path}"
print "Disk image:  ${dmg_path}"
