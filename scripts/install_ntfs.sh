#!/bin/zsh

set -euo pipefail

if (( $# != 0 )); then
    print -u2 "用法: ./scripts/install_ntfs.sh"
    exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
    print -u2 "错误: 此脚本仅支持 macOS。"
    exit 1
fi

has_brew_package() {
    brew list "$@" >/dev/null 2>&1
}

paragon_installed() {
    /usr/sbin/pkgutil --pkg-info com.paragon-software.pkg.ntfs >/dev/null 2>&1 ||
        has_brew_package --cask paragon-ntfs
}

ntfs3g_installed() {
    command -v ntfs-3g >/dev/null 2>&1 || has_brew_package --formula ntfs-3g-mac
}

supports_fskit() {
    local version major minor remainder
    version=$(/usr/bin/sw_vers -productVersion)
    major="${version%%.*}"
    remainder="${version#*.}"
    minor="${remainder%%.*}"
    [[ "${major}" == <-> && "${minor}" == <-> ]] || return 1
    (( major > 15 || (major == 15 && minor >= 4) ))
}

print_ntfs3g_next_step() {
    if supports_fskit; then
        print "当前 macOS 将使用 FSKit 后端，不需要在“隐私与安全性”中允许 macFUSE。"
        print "请运行本脚本并选择“用 NTFS-3G 重新挂载为可写”。"
    else
        print "首次重新挂载时 macOS 可能要求允许 macFUSE 系统扩展。"
        print "出现提示后，请在“系统设置 → 隐私与安全性”允许扩展并重启。"
    fi
}

require_brew() {
    if ! command -v brew >/dev/null 2>&1; then
        print -u2 "错误: 未安装 Homebrew，请先访问 https://brew.sh/ 安装。"
        return 1
    fi
}

confirm() {
    local answer
    read "answer?$1 [y/N] "
    [[ "${answer:l}" == "y" || "${answer:l}" == "yes" ]]
}

install_paragon() {
    require_brew || return 0
    if ntfs3g_installed; then
        print -u2 "已检测到 NTFS-3G。请先运行: brew uninstall ntfs-3g-mac"
        return
    fi
    if paragon_installed; then
        print "Paragon NTFS 已安装。"
        return
    fi

    print "Paragon NTFS 是商业软件，Homebrew 将下载官方试用版安装器。"
    confirm "继续安装 Paragon NTFS？" || return 0
    brew install --cask paragon-ntfs
    print "安装器完成后，请在“系统设置 → 隐私与安全性”允许系统扩展，重启 Mac，再重新插入 U 盘。"
}

install_ntfs3g() {
    require_brew || return 0
    if paragon_installed; then
        print -u2 "已检测到 Paragon NTFS。请先使用其卸载程序移除，避免驱动冲突。"
        return
    fi
    if ntfs3g_installed && has_brew_package --cask macfuse; then
        print "NTFS-3G 和 macFUSE 已安装。"
        print_ntfs3g_next_step
        return
    fi

    print "将安装 macFUSE 和 macOS 专用 NTFS-3G。"
    confirm "继续安装 NTFS-3G？" || return 0
    brew install --cask macfuse
    brew tap gromgit/fuse
    brew trust --formula gromgit/fuse/ntfs-3g-mac
    brew install gromgit/fuse/ntfs-3g-mac
    print_ntfs3g_next_step
}

mount_ntfs_rw() {
    require_brew || return 0
    if ! ntfs3g_installed; then
        print -u2 "错误: 请先安装 NTFS-3G。"
        return
    fi

    local entries
    entries=$(/sbin/mount | /usr/bin/awk '
        /^\/dev\/disk[0-9]+s[0-9]+ on \/Volumes\// && /\(ntfs,/ && /read-only/ {
            device = $0
            sub(/^\/dev\//, "", device)
            sub(/ on .*/, "", device)
            mount_point = $0
            sub(/^.* on /, "", mount_point)
            sub(/ \(ntfs,.*/, "", mount_point)
            print device "\t" mount_point
        }
    ')

    local -a devices mount_points
    local device mount_point
    while IFS=$'\t' read -r device mount_point; do
        [[ -n "${device}" && "${mount_point}" == /Volumes/* ]] || continue
        if /usr/sbin/diskutil info "/dev/${device}" |
            /usr/bin/grep -q 'Device Location:.*External'; then
            devices+=("${device}")
            mount_points+=("${mount_point}")
        fi
    done <<< "${entries}"

    if (( ${#devices} == 0 )); then
        print "未发现由 macOS 只读挂载的外置 NTFS 卷。"
        return
    fi

    print ""
    print "选择要重新挂载的 NTFS 卷:"
    local index=1
    for mount_point in "${mount_points[@]}"; do
        print "${index}) ${mount_point} (/dev/${devices[index]})"
        (( index++ ))
    done
    read "index?请选择: "
    if [[ "${index}" != <-> ]] || (( index < 1 || index > ${#devices} )); then
        print -u2 "无效选项。"
        return
    fi

    device=${devices[index]}
    mount_point=${mount_points[index]}
    print "请先关闭正在使用 ${mount_point} 中文件的应用。"
    confirm "卸载 /dev/${device} 并用 NTFS-3G 重新挂载为可写？" || return 0

    local ntfs_prefix
    if ! ntfs_prefix=$(brew --prefix ntfs-3g-mac); then
        print -u2 "错误: 无法定位 NTFS-3G。"
        return
    fi
    if ! /usr/sbin/diskutil unmount "/dev/${device}"; then
        print -u2 "卸载失败；请关闭占用此 U 盘的应用后重试。"
        return
    fi
    if ! sudo /bin/mkdir -p "${mount_point}"; then
        /usr/sbin/diskutil mount "/dev/${device}" >/dev/null || true
        print -u2 "创建挂载点失败，已恢复系统只读挂载。"
        return
    fi
    local -a backend_options
    if supports_fskit; then
        backend_options=(-o backend=fskit)
        print "正在通过 macFUSE FSKit 后端挂载（无需系统扩展授权）..."
    else
        backend_options=()
        print "正在通过 macFUSE 内核后端挂载..."
    fi
    if ! sudo "${ntfs_prefix}/sbin/mount_ntfs" "${backend_options[@]}" "/dev/${device}" "${mount_point}"; then
        sudo /bin/rmdir "${mount_point}" 2>/dev/null || true
        /usr/sbin/diskutil mount "/dev/${device}" >/dev/null || true
        print -u2 "NTFS-3G 挂载失败，已恢复系统只读挂载。"
        [[ -f /var/log/mount-ntfs-3g.log ]] && tail -20 /var/log/mount-ntfs-3g.log
        return
    fi

    local mounted_rw=false
    local attempt
    for attempt in {1..40}; do
        if /sbin/mount | /usr/bin/grep -F " on ${mount_point} (" |
            /usr/bin/grep -qv 'read-only'; then
            mounted_rw=true
            break
        fi
        /bin/sleep 0.25
    done
    if ${mounted_rw}; then
        print "✓ ${mount_point} 已通过 NTFS-3G 挂载为可写。"
        return
    fi

    sudo /bin/rmdir "${mount_point}" 2>/dev/null || true
    /usr/sbin/diskutil mount "/dev/${device}" >/dev/null || true
    print -u2 "未能确认 NTFS 可写挂载，已恢复系统只读挂载。"
}

show_status() {
    print ""
    print "驱动状态:"
    paragon_installed && print "  ✓ Paragon NTFS" || print "  - Paragon NTFS 未安装"
    ntfs3g_installed && print "  ✓ NTFS-3G" || print "  - NTFS-3G 未安装"
    has_brew_package --cask macfuse && print "  ✓ macFUSE" || print "  - macFUSE 未安装"

    print ""
    print "已挂载的 NTFS/FUSE 卷:"
    local mounts
    mounts=$(/sbin/mount | /usr/bin/awk '
        / on \/Volumes\// && ($0 ~ /\(ntfs,/ || $0 ~ /fuse/ || $0 ~ /fskit/ || $0 ~ /ufsd/) {
            mode = index($0, "read-only") ? "只读" : "可写"
            print "  " mode " · " $0
        }
    ')
    [[ -n "${mounts}" ]] && print -r -- "${mounts}" || print "  未发现"
}

while true; do
    print ""
    print "FlowFile NTFS 驱动安装"
    print "1) 安装 Paragon NTFS（商业试用版，推荐）"
    print "2) 安装 NTFS-3G + macFUSE（开源）"
    print "3) 用 NTFS-3G 重新挂载为可写"
    print "4) 检查驱动和 NTFS 卷状态"
    print "0) 退出"
    read "choice?请选择: "

    case "${choice}" in
        1) install_paragon ;;
        2) install_ntfs3g ;;
        3) mount_ntfs_rw ;;
        4) show_status ;;
        0) print "已退出。"; exit 0 ;;
        *) print -u2 "无效选项，请重新选择。" ;;
    esac
done
