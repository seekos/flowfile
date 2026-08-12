#!/bin/zsh

set -euo pipefail

script_dir=${0:A:h}
project_dir=${script_dir:h}
sdk_path=$(xcrun --sdk macosx --show-sdk-path)

case "$(uname -m)" in
    arm64)
        export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="--target=arm64-apple-macos11 -isysroot ${sdk_path}"
        ;;
    x86_64)
        export BINDGEN_EXTRA_CLANG_ARGS_x86_64_apple_darwin="--target=x86_64-apple-macos11 -isysroot ${sdk_path}"
        ;;
    *)
        print -u2 "FlowFile currently supports arm64 and x86_64 macOS hosts."
        exit 1
        ;;
esac

cd "${project_dir}"
cargo build

# Run from one stable bundle path so Launch Services and macOS privacy controls
# do not see a new command-line executable path on every build.
app_path=${project_dir}/target/debug/bundle/osx/FlowFile.app
if [[ -d "${app_path}" ]]; then
    /bin/rm -R "${app_path}"
fi
mkdir -p "${app_path}/Contents/MacOS" "${app_path}/Contents/Resources"
cp "${project_dir}/target/debug/flowfile" "${app_path}/Contents/MacOS/flowfile"
cp "${project_dir}/resources/FlowFile.icns" \
    "${app_path}/Contents/Resources/FlowFile.icns"
cp "${project_dir}/resources/Info.plist" "${app_path}/Contents/Info.plist"
chmod 755 "${app_path}/Contents/MacOS/flowfile"
/usr/bin/codesign --force --deep --sign - "${app_path}" >/dev/null 2>&1

exec /usr/bin/open -W "${app_path}"
