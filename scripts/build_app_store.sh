#!/bin/zsh

set -euo pipefail

script_dir=${0:A:h}
project_dir=${script_dir:h}
resources_dir=${project_dir}/resources
output_dir=${project_dir}/dist/app-store
app_path=${output_dir}/FlowFile.app
pkg_path=${output_dir}/FlowFile.pkg
entitlements_path=${resources_dir}/FlowFile-AppStore.entitlements
privacy_manifest_path=${resources_dir}/PrivacyInfo.xcprivacy
bundle_id=${FLOWFILE_BUNDLE_ID:-cc.bso.flowfile}
version=""
build_number=""
prepare_only=false
upload=false

usage() {
    print "Usage: ${0:t} [-v <version>] [-b <build>] [--prepare-only] [--upload]"
    print "  -v, --version       App Store version (default: Cargo.toml package version)"
    print "  -b, --build         Monotonically increasing CFBundleVersion (default: 1)"
    print "      --prepare-only  Ad-hoc sign a sandboxed app for local verification"
    print "      --upload        Validate and upload the signed package with altool"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -v|--version)
            version=${2:-}
            shift 2
            ;;
        -b|--build)
            build_number=${2:-}
            shift 2
            ;;
        --prepare-only)
            prepare_only=true
            shift
            ;;
        --upload)
            upload=true
            shift
            ;;
        -h|--help)
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

cd "${project_dir}"
version=${version:-$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)}
version=${version#v}
build_number=${build_number:-1}
if [[ ! "${version}" =~ '^[0-9]+\.[0-9]+\.[0-9]+$' ]]; then
    print -u2 "App Store version must contain three numeric components, for example 1.0.0."
    exit 1
fi
if [[ ! "${build_number}" =~ '^[0-9]+([.][0-9]+)*$' ]]; then
    print -u2 "Build number must contain only digits and periods, for example 1 or 2026.9.17.1."
    exit 1
fi
if [[ ! "${bundle_id}" =~ '^[A-Za-z0-9-]+([.][A-Za-z0-9-]+)+$' ]]; then
    print -u2 "Invalid bundle identifier: ${bundle_id}"
    exit 1
fi

source "${script_dir}/macos_toolchain.sh"
flowfile_configure_macos_toolchain
"${script_dir}/build.sh" --icon-only

print "Building sandboxed FlowFile ${version} (${build_number}) for ${bundle_id}"
cargo build --release --features app-store

if [[ -d "${app_path}" ]]; then
    /bin/rm -R "${app_path}"
fi
mkdir -p "${app_path}/Contents/MacOS" "${app_path}/Contents/Resources"
cp "${project_dir}/target/release/flowfile" "${app_path}/Contents/MacOS/flowfile"
cp "${resources_dir}/FlowFile.icns" "${app_path}/Contents/Resources/FlowFile.icns"
cp "${privacy_manifest_path}" "${app_path}/Contents/Resources/PrivacyInfo.xcprivacy"
cp "${resources_dir}/Info.plist" "${app_path}/Contents/Info.plist"
chmod 755 "${app_path}/Contents/MacOS/flowfile"

/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier ${bundle_id}" "${app_path}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${app_path}/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${build_number}" "${app_path}/Contents/Info.plist"

if [[ -n "${FLOWFILE_PROVISIONING_PROFILE:-}" ]]; then
    if [[ ! -f "${FLOWFILE_PROVISIONING_PROFILE}" ]]; then
        print -u2 "Provisioning profile not found: ${FLOWFILE_PROVISIONING_PROFILE}"
        exit 1
    fi
    cp "${FLOWFILE_PROVISIONING_PROFILE}" \
        "${app_path}/Contents/embedded.provisionprofile"
fi

if [[ "${prepare_only}" == true ]]; then
    /usr/bin/codesign --force --sign - --entitlements "${entitlements_path}" "${app_path}"
    /usr/bin/productbuild --component "${app_path}" /Applications "${pkg_path}"
    print "Prepared a local sandbox test build (not uploadable): ${app_path}"
else
    app_identity=${FLOWFILE_APP_STORE_APP_IDENTITY:-}
    installer_identity=${FLOWFILE_APP_STORE_INSTALLER_IDENTITY:-}
    if [[ -z "${app_identity}" ]]; then
        app_identity=$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null |
            /usr/bin/sed -nE 's/.*"((Apple Distribution|3rd Party Mac Developer Application|Mac App Distribution):[^"]+)".*/\1/p' |
            /usr/bin/head -n 1)
    fi
    if [[ -z "${installer_identity}" ]]; then
        installer_identity=$(/usr/bin/security find-certificate -a -c "Mac Installer Distribution" 2>/dev/null |
            /usr/bin/sed -nE 's/.*"alis"<blob>="([^"]+)".*/\1/p' |
            /usr/bin/head -n 1)
    fi
    if [[ -z "${app_identity}" || -z "${installer_identity}" ]]; then
        print -u2 "Mac App Store signing identities with private keys are not available locally."
        print -u2 "Install Apple Distribution and Mac Installer Distribution identities, or set:"
        print -u2 "  FLOWFILE_APP_STORE_APP_IDENTITY"
        print -u2 "  FLOWFILE_APP_STORE_INSTALLER_IDENTITY"
        exit 1
    fi

    /usr/bin/codesign --force --timestamp --sign "${app_identity}" \
        --entitlements "${entitlements_path}" "${app_path}"
    /usr/bin/codesign --verify --deep --strict --verbose=2 "${app_path}"
    /usr/bin/productbuild --component "${app_path}" /Applications \
        --sign "${installer_identity}" "${pkg_path}"
    /usr/sbin/pkgutil --check-signature "${pkg_path}"
    print "Signed App Store package: ${pkg_path}"
fi

if [[ "${upload}" == true ]]; then
    if [[ "${prepare_only}" == true ]]; then
        print -u2 "--upload cannot be combined with --prepare-only."
        exit 1
    fi
    if [[ -n "${FLOWFILE_ASC_KEY_ID:-}" && -n "${FLOWFILE_ASC_ISSUER_ID:-}" ]]; then
        auth_args=(--apiKey "${FLOWFILE_ASC_KEY_ID}" --apiIssuer "${FLOWFILE_ASC_ISSUER_ID}")
    elif [[ -n "${FLOWFILE_ASC_APPLE_ID:-}" && -n "${FLOWFILE_ASC_APP_PASSWORD:-}" ]]; then
        auth_args=(-u "${FLOWFILE_ASC_APPLE_ID}" -p "${FLOWFILE_ASC_APP_PASSWORD}")
    else
        print -u2 "Upload credentials are missing. Configure an App Store Connect API key or:"
        print -u2 "  FLOWFILE_ASC_APPLE_ID and FLOWFILE_ASC_APP_PASSWORD"
        exit 1
    fi
    xcrun altool --validate-app -f "${pkg_path}" -t macos "${auth_args[@]}"
    xcrun altool --upload-app -f "${pkg_path}" -t macos "${auth_args[@]}"
fi

print "Application: ${app_path}"
print "Installer:   ${pkg_path}"
