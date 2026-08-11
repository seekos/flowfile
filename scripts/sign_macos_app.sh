#!/bin/zsh

set -euo pipefail

app_path=${1:?"Usage: sign_macos_app.sh <FlowFile.app>"}
if [[ ! -d "${app_path}" ]]; then
    print -u2 "Application bundle not found: ${app_path}"
    exit 1
fi

identity=${FLOWFILE_CODESIGN_IDENTITY:-}
identity_label=${identity}

if [[ -z "${identity}" ]]; then
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
