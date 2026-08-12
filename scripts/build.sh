#!/bin/zsh

set -euo pipefail

script_dir=${0:A:h}
script_name=${0:t}
project_dir=${script_dir:h}
resources_dir=${project_dir}/resources
dist_dir=${project_dir}/dist
icon_source=${resources_dir}/FlowFile.svg
icon_output=${resources_dir}/FlowFile.icns
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
icon_only=false
version_override=""

usage() {
    print "Usage: ${script_name} [-v <version>] [--icon-only]"
    print "  -v, --version <version>  Override the package version (for example: v0.1.2)"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -v|--version)
            if [[ $# -lt 2 || -z "$2" ]]; then
                print -u2 "Missing version after $1"
                usage >&2
                exit 1
            fi
            version_override="$2"
            shift 2
            ;;
        --icon-only)
            icon_only=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            print -u2 "Unknown argument: $1"
            usage >&2
            exit 1
            ;;
    esac
done

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

sign_app() {
    local app_path=$1
    local identity=${FLOWFILE_CODESIGN_IDENTITY:-}
    local identity_label=${identity}

    if [[ -z "${identity}" ]]; then
        local identity_line
        identity_line=$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null |
            /usr/bin/grep -E -m 1 '"(Developer ID Application|Apple Development|Mac Developer):' || true)
        if [[ -n "${identity_line}" ]]; then
            identity=$(print -r -- "${identity_line}" |
                /usr/bin/sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]+([0-9A-F]+).*/\1/')
            identity_label=$(print -r -- "${identity_line}" |
                /usr/bin/sed -E 's/.*"([^"]+)".*/\1/')
        fi
    fi

    if [[ -n "${identity}" ]]; then
        local -a sign_args
        sign_args=(--force --deep --options runtime --sign "${identity}")
        if [[ "${identity_label}" == Developer\ ID\ Application:* ||
              "${FLOWFILE_CODESIGN_TIMESTAMP:-0}" == "1" ]]; then
            sign_args+=(--timestamp)
        fi
        /usr/bin/codesign "${sign_args[@]}" "${app_path}" >/dev/null 2>&1
        print "Signed FlowFile with stable identity: ${identity_label}"
    else
        if [[ "${FLOWFILE_REQUIRE_STABLE_SIGNING:-0}" == "1" ]]; then
            print -u2 "No Apple code-signing identity is installed."
            print -u2 "A stable identity is required so macOS can remember folder permissions."
            exit 1
        fi
        /usr/bin/codesign --force --deep --sign - "${app_path}" >/dev/null 2>&1
        print "Signed FlowFile with an ad-hoc development signature."
    fi

    /usr/bin/codesign --verify --deep --strict "${app_path}"
}

cd "${project_dir}"
version=${version_override:-$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)}
version=${version#v}
if [[ ! "${version}" =~ '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z][0-9A-Za-z.-]*)?$' ]]; then
    print -u2 "Invalid version: ${version_override:-${version}}"
    print -u2 "Expected a semantic version such as v0.1.2 or 0.1.2."
    exit 1
fi
build_icon
if [[ "${icon_only}" == true ]]; then
    print "Generated ${icon_output}"
    exit 0
fi

print "Packaging FlowFile v${version}"

built_app_path=${project_dir}/target/release/bundle/osx/FlowFile.app
if cargo bundle --help >/dev/null 2>&1; then
    cargo bundle --release
else
    print "cargo-bundle is unavailable; using the offline macOS bundle fallback."
    cargo build --release
    if [[ -d "${built_app_path}" ]]; then
        /bin/rm -R "${built_app_path}"
    fi
    mkdir -p "${built_app_path}/Contents/MacOS" "${built_app_path}/Contents/Resources"
    cp "${project_dir}/target/release/flowfile" "${built_app_path}/Contents/MacOS/flowfile"
    cp "${project_dir}/resources/FlowFile.icns" \
        "${built_app_path}/Contents/Resources/FlowFile.icns"
    cp "${project_dir}/resources/Info.plist" "${built_app_path}/Contents/Info.plist"
    chmod 755 "${built_app_path}/Contents/MacOS/flowfile"
fi
if [[ ! -d "${built_app_path}" ]]; then
    print -u2 "Expected application bundle not found at ${built_app_path}"
    exit 1
fi

mkdir -p "${dist_dir}"
app_path=${dist_dir}/FlowFile.app
if [[ -d "${app_path}" ]]; then
    /bin/rm -R "${app_path}"
fi
cp -R "${built_app_path}" "${app_path}"
/usr/libexec/PlistBuddy -c \
    "Set :CFBundleShortVersionString ${version}" "${app_path}/Contents/Info.plist"

sign_app "${app_path}"

dmg_stage=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-dmg.XXXXXX")
cp -R "${app_path}" "${dmg_stage}/FlowFile.app"
ln -s /Applications "${dmg_stage}/Applications"
dmg_path=${dist_dir}/FlowFile-${version}.dmg
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
