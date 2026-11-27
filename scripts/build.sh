#!/bin/zsh

set -euo pipefail

script_dir=${0:A:h}
script_name=${0:t}
project_dir=${script_dir:h}
resources_dir=${project_dir}/resources
dist_dir=${project_dir}/dist
icon_source=${resources_dir}/FlowFile.svg
icon_output=${resources_dir}/FlowFile.icns
version_file=${project_dir}/.flowfile-build-version

icon_only=false
skip_notarize=false
version_override=""
notary_profile="flowfile-notary"
signing_identity=""
identity_label=""
notary_auth_args=()

usage() {
    print "Usage: ${script_name} [-v <version>] [--skip-notarize] [--icon-only]"
    print "  -v, --version <version>  Override package version (e.g. v0.1.2)"
    print "      --skip-notarize      Build and sign locally without Apple notarization"
    print "      --icon-only          Only regenerate resources/FlowFile.icns"
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
        --skip-notarize)
            skip_notarize=true
            shift
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

read_toml_key() {
    local file_path=$1
    local key=$2
    /usr/bin/awk -v k="${key}" '
        /^[[:space:]]*#/ || /^[[:space:]]*$/ || /^[[:space:]]*\[/ { next }
        {
            sub(/[[:space:]]+#.*$/, "")
            if (match($0, "^[[:space:]]*" k "[[:space:]]*=")) {
                v = substr($0, RLENGTH + 1)
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", v)
                gsub(/^["'\'']|["'\'']$/, "", v)
                print v
            }
        }
    ' "${file_path}" | /usr/bin/tail -n 1
}

load_settings() {
    local toml_path=""
    if [[ -f "${script_dir}/notarize.toml" ]]; then
        toml_path=${script_dir}/notarize.toml
    elif [[ -f "${project_dir}/notarize.toml" ]]; then
        toml_path=${project_dir}/notarize.toml
    fi

    if [[ -n "${toml_path}" ]]; then
        local v=""
        v=$(read_toml_key "${toml_path}" "profile")
        if [[ -n "${v}" ]]; then notary_profile=${v}; fi
        v=$(read_toml_key "${toml_path}" "identity")
        if [[ -n "${v}" ]]; then signing_identity=${v}; fi
    fi

    if [[ -n "${FLOWFILE_NOTARY_PROFILE:-}" ]]; then
        notary_profile=${FLOWFILE_NOTARY_PROFILE}
    fi
    if [[ -n "${FLOWFILE_CODESIGN_IDENTITY:-}" ]]; then
        signing_identity=${FLOWFILE_CODESIGN_IDENTITY}
    fi
    if [[ "${FLOWFILE_SKIP_NOTARIZE:-0}" == "1" ]]; then
        skip_notarize=true
    fi
}

build_icon() {
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
}

resolve_identity() {
    local all_ids=""
    all_ids=$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null || true)

    if [[ -n "${signing_identity}" ]]; then
        local matched=""
        matched=$(print -r -- "${all_ids}" | /usr/bin/grep -F -m 1 "${signing_identity}" || true)
        if [[ -n "${matched}" ]]; then
            signing_identity=$(print -r -- "${matched}" | /usr/bin/sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]+([0-9A-F]+).*/\1/')
            identity_label=$(print -r -- "${matched}" | /usr/bin/sed -E 's/.*"([^"]+)".*/\1/')
        else
            identity_label=${signing_identity}
        fi
        return 0
    fi

    local pattern='"(Developer ID Application|Apple Development|Mac Developer):'
    if [[ "${skip_notarize}" == false ]]; then
        pattern='"Developer ID Application:'
    fi

    local line=""
    line=$(print -r -- "${all_ids}" | /usr/bin/grep -E -m 1 "${pattern}" || true)
    if [[ -n "${line}" ]]; then
        signing_identity=$(print -r -- "${line}" | /usr/bin/sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]+([0-9A-F]+).*/\1/')
        identity_label=$(print -r -- "${line}" | /usr/bin/sed -E 's/.*"([^"]+)".*/\1/')
    fi
}

ensure_notary_ready() {
    if [[ -z "${signing_identity}" || "${identity_label}" != Developer\ ID\ Application:* ]]; then
        print -u2 "公证打包需要本机安装 'Developer ID Application' 证书。"
        print -u2 "如仅做本地免公证测试，请运行: ./scripts/${script_name} --skip-notarize"
        exit 1
    fi

    if ! xcrun --find notarytool >/dev/null 2>&1 || ! xcrun --find stapler >/dev/null 2>&1; then
        print -u2 "公证需要完整 Xcode（含 notarytool 与 stapler）。"
        exit 1
    fi

    if [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
        notary_auth_args=(--apple-id "${APPLE_ID}" --password "${APPLE_APP_SPECIFIC_PASSWORD}" --team-id "${APPLE_TEAM_ID}")
        return 0
    fi

    local history_out=""
    set +e
    history_out=$(xcrun notarytool history --keychain-profile "${notary_profile}" 2>&1)
    local history_rc=$?
    set -e

    if [[ ${history_rc} -ne 0 ]]; then
        if print -r -- "${history_out}" | /usr/bin/grep -q "No Keychain password item found"; then
            if [[ -t 0 ]]; then
                print "首次公证需要将 Apple 公证凭据存入钥匙串（profile: ${notary_profile}）。"
                local team_id=""
                team_id=$(print -r -- "${identity_label}" | /usr/bin/sed -nE 's/.*\(([A-Z0-9]+)\).*/\1/p')
                local -a store_args
                store_args=("${notary_profile}")
                if [[ -n "${team_id}" ]]; then
                    store_args+=(--team-id "${team_id}")
                fi
                xcrun notarytool store-credentials "${store_args[@]}"
            else
                print -u2 "未找到公证钥匙串凭据 '${notary_profile}'。"
                print -u2 "请先运行一次: xcrun notarytool store-credentials \"${notary_profile}\""
                exit 1
            fi
        else
            print -u2 "${history_out}"
            exit 1
        fi
    fi

    notary_auth_args=(--keychain-profile "${notary_profile}")
}

sign_app() {
    local app_path=$1
    /usr/bin/xattr -cr "${app_path}"

    if [[ -n "${signing_identity}" ]]; then
        local -a sign_args
        sign_args=(--force --deep --options runtime --sign "${signing_identity}")
        if [[ "${skip_notarize}" == false || "${identity_label}" == Developer\ ID\ Application:* ]]; then
            sign_args+=(--timestamp)
        fi
        /usr/bin/codesign "${sign_args[@]}" "${app_path}"
        print "Signed FlowFile.app: ${identity_label}"
    else
        /usr/bin/codesign --force --deep --sign - "${app_path}" >/dev/null 2>&1
        print "Signed FlowFile.app with ad-hoc signature."
    fi

    /usr/bin/codesign --verify --deep --strict "${app_path}"
}

notarize_dmg() {
    local dmg_path=$1
    local app_path=$2

    /usr/bin/codesign --force --timestamp --sign "${signing_identity}" "${dmg_path}"
    /usr/bin/codesign --verify --strict "${dmg_path}"

    local submit_log
    submit_log=$(mktemp "${TMPDIR:-/tmp}/flowfile-notary.XXXXXX")
    print "Submitting ${dmg_path:t} for Apple notarization..."
    set +e
    xcrun notarytool submit "${dmg_path}" "${notary_auth_args[@]}" --wait 2>&1 | tee "${submit_log}"
    local rc=${pipestatus[1]}
    set -e

    local sub_id=""
    sub_id=$(/usr/bin/awk '/^[[:space:]]*id:[[:space:]]+/ { print $2; exit }' "${submit_log}")
    local status=""
    status=$(/usr/bin/awk '/^[[:space:]]*status:[[:space:]]+/ { val=$2 } END { print val }' "${submit_log}")
    /bin/rm -f "${submit_log}"

    if [[ ${rc} -ne 0 || "${status}" != "Accepted" ]]; then
        print -u2 "Notarization failed (status=${status:-unknown})."
        if [[ -n "${sub_id}" ]]; then
            xcrun notarytool log "${sub_id}" "${notary_auth_args[@]}" || true
        fi
        exit 1
    fi

    xcrun stapler staple "${dmg_path}"
    xcrun stapler validate "${dmg_path}"
    xcrun stapler staple "${app_path}" >/dev/null 2>&1 || true
}

cd "${project_dir}"
if [[ -n "${version_override}" ]]; then
    version=${version_override}
elif [[ -s "${version_file}" ]]; then
    version=$(<"${version_file}")
else
    version=$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)
fi
version=${version#v}
if [[ ! "${version}" =~ '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z][0-9A-Za-z.-]*)?$' ]]; then
    print -u2 "Invalid version: ${version_override:-${version}}"
    exit 1
fi

build_icon
if [[ "${icon_only}" == true ]]; then
    print "Generated ${icon_output}"
    exit 0
fi

load_settings
resolve_identity

source "${script_dir}/macos_toolchain.sh"
flowfile_configure_macos_toolchain

if [[ "${skip_notarize}" == false ]]; then
    ensure_notary_ready
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
    cp "${project_dir}/resources/FlowFile.icns" "${built_app_path}/Contents/Resources/FlowFile.icns"
    cp "${project_dir}/resources/Info.plist" "${built_app_path}/Contents/Info.plist"
    chmod 755 "${built_app_path}/Contents/MacOS/flowfile"
fi

mkdir -p "${dist_dir}"
app_path=${dist_dir}/FlowFile.app
if [[ -d "${app_path}" ]]; then
    /bin/rm -R "${app_path}"
fi
cp -R "${built_app_path}" "${app_path}"
cp "${resources_dir}/PrivacyInfo.xcprivacy" "${app_path}/Contents/Resources/PrivacyInfo.xcprivacy"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${app_path}/Contents/Info.plist"

sign_app "${app_path}"

dmg_stage=$(mktemp -d "${TMPDIR:-/tmp}/flowfile-dmg.XXXXXX")
cp -R "${app_path}" "${dmg_stage}/FlowFile.app"
ln -s /Applications "${dmg_stage}/Applications"
dmg_path=${dist_dir}/FlowFile-${version}.dmg
if [[ -e "${dmg_path}" ]]; then
    /bin/rm -f "${dmg_path}"
fi
if /usr/sbin/diskutil image create from --help >/dev/null 2>&1; then
    /usr/sbin/diskutil image create from --format UDZO --volumeName "FlowFile" "${dmg_stage}" "${dmg_path}"
else
    /usr/bin/hdiutil create -volname "FlowFile" -srcfolder "${dmg_stage}" -format UDZO "${dmg_path}"
fi
/bin/rm -R "${dmg_stage}"

if [[ "${skip_notarize}" == false ]]; then
    notarize_dmg "${dmg_path}" "${app_path}"
else
    if [[ -n "${signing_identity}" ]]; then
        /usr/bin/codesign --force --timestamp --sign "${signing_identity}" "${dmg_path}"
    fi
    print "Skipped notarization (--skip-notarize)."
fi

print -r -- "${version}" > "${version_file}"
print "Application: ${app_path}"
print "Disk image:  ${dmg_path}"
