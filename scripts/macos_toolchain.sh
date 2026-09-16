#!/bin/zsh

# Select a usable macOS SDK without requiring the active full Xcode install to
# have accepted its license. A separately installed Command Line Tools package
# is sufficient for FlowFile's normal build and development workflows.
flowfile_configure_macos_toolchain() {
    local sdk_path=""
    local command_line_tools=/Library/Developer/CommandLineTools

    if [[ -n "${FLOWFILE_DEVELOPER_DIR:-}" ]]; then
        export DEVELOPER_DIR=${FLOWFILE_DEVELOPER_DIR}
    fi

    if sdk_path=$(xcrun --sdk macosx --show-sdk-path 2>/dev/null) &&
        [[ -n "${sdk_path}" ]]; then
        :
    elif [[ -d "${command_line_tools}" ]] &&
        sdk_path=$(DEVELOPER_DIR=${command_line_tools} \
            xcrun --sdk macosx --show-sdk-path 2>/dev/null) &&
        [[ -n "${sdk_path}" ]]; then
        export DEVELOPER_DIR=${command_line_tools}
        print "Active Xcode tools are unavailable; using Command Line Tools."
    else
        print -u2 "No usable macOS developer tools were found."
        print -u2 "Install Command Line Tools with: xcode-select --install"
        print -u2 "Or accept the selected Xcode license with: sudo xcodebuild -license"
        return 1
    fi

    case "$(uname -m)" in
        arm64)
            export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="--target=arm64-apple-macos11 -isysroot ${sdk_path}"
            ;;
        x86_64)
            export BINDGEN_EXTRA_CLANG_ARGS_x86_64_apple_darwin="--target=x86_64-apple-macos11 -isysroot ${sdk_path}"
            ;;
        *)
            print -u2 "FlowFile supports arm64 and x86_64 macOS hosts."
            return 1
            ;;
    esac
}
