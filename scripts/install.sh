#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Install or remove Gimbal Files for the current user. It never runs cargo and
# never needs sudo.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

usage() {
    cat <<'EOF'
Usage: install.sh [--prefix DIR] [--data-dir DIR] [--no-prompt] [--uninstall]

Install Gimbal Files under ~/.local (or DIR), or remove the files this script
installed. Run it from a checkout after `cargo build --release`, or from an
unpacked release tarball.

  --prefix DIR   install under DIR instead of ~/.local
  --data-dir DIR put the desktop entry, icon and marker under DIR instead of
                 the default (DIR/share with --prefix, else XDG_DATA_HOME)
  --no-prompt    never ask anything (for updaters and scripts); a missing
                 optional tool is only reported
  --uninstall    remove exactly the installed files
  -h, --help     show this help
EOF
}

error() {
    echo "install.sh: error: $*" >&2
    exit 1
}

prefix=""
prefix_given=false
data_dir_arg=""
uninstall=false
no_prompt=false
while (( $# > 0 )); do
    case "$1" in
        --prefix)
            if (( $# < 2 )) || [[ -z "$2" || "$2" == --* ]]; then
                error "--prefix needs a directory"
            fi
            prefix="$2"
            prefix_given=true
            shift 2
            ;;
        --prefix=*)
            prefix="${1#*=}"
            [[ -n "$prefix" ]] || error "--prefix needs a directory"
            prefix_given=true
            shift
            ;;
        --data-dir)
            if (( $# < 2 )) || [[ -z "$2" || "$2" == --* ]]; then
                error "--data-dir needs a directory"
            fi
            data_dir_arg="$2"
            shift 2
            ;;
        --data-dir=*)
            data_dir_arg="${1#*=}"
            [[ -n "$data_dir_arg" ]] || error "--data-dir needs a directory"
            shift
            ;;
        --no-prompt)
            no_prompt=true
            shift
            ;;
        --uninstall)
            uninstall=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            error "unknown argument: $1"
            ;;
    esac
done

if [[ -z "$prefix" ]]; then
    [[ -n "${HOME:-}" ]] || error "HOME is not set; use --prefix DIR"
    prefix="${HOME%/}/.local"
fi
case "$prefix" in
    /*) ;;
    *) prefix="$(pwd -P)/$prefix" ;;
esac
prefix="${prefix%/}"
[[ -n "$prefix" ]] || prefix="/"

bin_dir="$prefix/bin"
if [[ -n "$data_dir_arg" ]]; then
    # An updater passes back the data dir the marker recorded, so an install
    # that used a custom XDG_DATA_HOME stays where it was.
    case "$data_dir_arg" in
        /*) data_dir="${data_dir_arg%/}" ;;
        *) data_dir="$(pwd -P)/${data_dir_arg%/}" ;;
    esac
elif $prefix_given; then
    # --prefix always means DIR/share for data, regardless of XDG_DATA_HOME.
    data_dir="$prefix/share"
elif [[ -n "${XDG_DATA_HOME:-}" ]]; then
    case "$XDG_DATA_HOME" in
        /*) data_dir="$XDG_DATA_HOME" ;;
        *) data_dir="$prefix/share" ;;
    esac
else
    data_dir="$prefix/share"
fi

desktop_dir="$data_dir/applications"
icon_dir="$data_dir/icons/hicolor/scalable/apps"
marker="$data_dir/gimbal-files/install.toml"

app_id="io.github.kcanady21.GimbalFiles"
desktop_name="$app_id.desktop"
icon_name="$app_id.svg"

refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$desktop_dir" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -d "$data_dir/icons/hicolor" ]]; then
        gtk-update-icon-cache -f -t "$data_dir/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

if $uninstall; then
    removed=0
    for file in \
        "$bin_dir/gimbal-files" \
        "$bin_dir/gimbal-files-cli" \
        "$desktop_dir/$desktop_name" \
        "$icon_dir/$icon_name" \
        "$marker"; do
        if [[ -e "$file" || -L "$file" ]]; then
            rm -f -- "$file"
            removed=$((removed + 1))
        fi
    done
    refresh_caches
    echo "Removed $removed Gimbal Files file(s)."
    exit 0
fi

# A release tarball has install.sh, bin/ and latest.toml at its root. A
# checkout keeps the script in scripts/ and the built binaries in target/.
tarball_mode=false
if [[ -f "$script_dir/latest.toml" && -f "$script_dir/bin/gimbal-files" ]]; then
    tarball_mode=true
    bin_src="$script_dir/bin"
    desktop_src="$script_dir/share/applications/$desktop_name"
    icon_src="$script_dir/share/icons/hicolor/scalable/apps/$icon_name"
    version_file="$script_dir/latest.toml"
else
    repo_root="$(cd -- "$script_dir/.." && pwd -P)"
    if [[ ! -f "$repo_root/Cargo.toml" || ! -d "$repo_root/crates/fm-ui" ]]; then
        error "run this script from a checkout or an unpacked release tarball"
    fi
    bin_src="$repo_root/target/release"
    desktop_src="$repo_root/packaging/$desktop_name"
    icon_src="$repo_root/packaging/icons/$icon_name"
    version_file="$repo_root/Cargo.toml"
fi

for binary in gimbal-files gimbal-files-cli; do
    if [[ ! -f "$bin_src/$binary" ]]; then
        if $tarball_mode; then
            error "the release tarball is missing bin/$binary"
        else
            error "target/release/$binary is missing; run 'cargo build --release' first (this script never runs cargo)"
        fi
    fi
    [[ -x "$bin_src/$binary" ]] || error "$bin_src/$binary is not executable"
done
[[ -f "$desktop_src" ]] || error "desktop entry not found at $desktop_src"
[[ -f "$icon_src" ]] || error "app icon not found at $icon_src"

if $tarball_mode; then
    version="$(sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$version_file" | head -n 1)"
else
    version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$version_file" | head -n 1)"
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || error "could not read a version from $version_file"

mkdir -p -- "$bin_dir" "$desktop_dir" "$icon_dir" "$(dirname -- "$marker")"

# Record real locations so P2v can compare a canonicalized current_exe with
# bin_dir. The prefix stays as the absolute directory the caller chose.
bin_dir="$(cd -- "$bin_dir" && pwd -P)"
data_dir="$(cd -- "$data_dir" && pwd -P)"
desktop_dir="$data_dir/applications"
icon_dir="$data_dir/icons/hicolor/scalable/apps"
marker="$data_dir/gimbal-files/install.toml"

tmp_paths=()
cleanup_temps() {
    if (( ${#tmp_paths[@]} > 0 )); then
        rm -f -- "${tmp_paths[@]}"
    fi
}
trap cleanup_temps EXIT

install_file() {
    local src="$1"
    local dest="$2"
    local mode="$3"
    local tmp
    tmp="$(dirname -- "$dest")/.$(basename -- "$dest").tmp.$$"
    tmp_paths+=("$tmp")
    cp -f -- "$src" "$tmp"
    chmod "$mode" "$tmp"
    mv -f -- "$tmp" "$dest"
}

install_file "$bin_src/gimbal-files" "$bin_dir/gimbal-files" 0755
install_file "$bin_src/gimbal-files-cli" "$bin_dir/gimbal-files-cli" 0755
# The entry runs the installed binary by absolute path, so the launcher works
# even when the session's PATH lacks the bin dir. A path the desktop-entry
# quoting rules can't carry safely keeps the plain name.
desktop_tmp="$(dirname -- "$desktop_dir/$desktop_name")/.$desktop_name.src.$$"
tmp_paths+=("$desktop_tmp")
case "$bin_dir" in
    *[\\\"\`\$]*|*$'\n'*) cp -f -- "$desktop_src" "$desktop_tmp" ;;
    *)
        exec_path="$bin_dir/gimbal-files"
        exec_path="${exec_path//|/\\|}"
        exec_path="${exec_path//&/\\&}"
        sed "s|^Exec=gimbal-files |Exec=\"$exec_path\" |" "$desktop_src" >"$desktop_tmp"
        ;;
esac
install_file "$desktop_tmp" "$desktop_dir/$desktop_name" 0644
install_file "$icon_src" "$icon_dir/$icon_name" 0644

toml_escape() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '%s' "$value"
}

marker_tmp="$marker.tmp.$$"
tmp_paths+=("$marker_tmp")
{
    printf '# SPDX-License-Identifier: GPL-3.0-or-later\n'
    printf 'bin_dir = "%s"\n' "$(toml_escape "$bin_dir")"
    printf 'prefix = "%s"\n' "$(toml_escape "$prefix")"
    printf 'data_dir = "%s"\n' "$(toml_escape "$data_dir")"
    printf 'version = "%s"\n' "$version"
} >"$marker_tmp"
chmod 0644 "$marker_tmp"
mv -f -- "$marker_tmp" "$marker"

refresh_caches

case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) echo "warning: $bin_dir is not on PATH; add it to use gimbal-files from a terminal" >&2 ;;
esac

echo "Installed Gimbal Files $version to $prefix"

# Disk Usage in the folder menu opens Baobab, which is a system package, so
# Gimbal can't ship it. Offer the distro's package manager, which asks for a
# password; Gimbal's own install never does. Without a person at the keyboard
# (--no-prompt, or no terminal), only say how. GIMBAL_INSTALL_PRETEND_MISSING
# lets scripts/test-install.sh exercise this path on a machine that has it.
baobab_install_command() {
    if command -v apt-get >/dev/null 2>&1; then
        echo "sudo apt install baobab"
    elif command -v dnf >/dev/null 2>&1; then
        echo "sudo dnf install baobab"
    elif command -v pacman >/dev/null 2>&1; then
        echo "sudo pacman -S --needed baobab"
    elif command -v zypper >/dev/null 2>&1; then
        echo "sudo zypper install baobab"
    fi
}

baobab_missing=false
if [[ "${GIMBAL_INSTALL_PRETEND_MISSING:-}" == "baobab" ]] || ! command -v baobab >/dev/null 2>&1; then
    baobab_missing=true
fi
if $baobab_missing; then
    baobab_command="$(baobab_install_command)"
    echo "Disk Usage in Gimbal's folder menu needs Baobab (GNOME Disk Usage Analyzer), which is not installed."
    if [[ -z "$baobab_command" ]]; then
        echo "Install the 'baobab' package with your distribution's package manager to enable it."
    elif $no_prompt || [[ ! -t 0 || ! -t 1 ]] || ! command -v sudo >/dev/null 2>&1; then
        echo "To enable it, run: $baobab_command"
    else
        printf 'Install it now with "%s"? This asks for your password. [y/N] ' "$baobab_command"
        read -r answer || answer=""
        case "$answer" in
            y|Y)
                # The command is one of the fixed strings above.
                read -r -a baobab_argv <<<"$baobab_command"
                if "${baobab_argv[@]}"; then
                    echo "Baobab installed. Restart Gimbal Files if it is running to enable Disk Usage."
                else
                    echo "warning: installing Baobab failed; Gimbal Files is installed without Disk Usage" >&2
                fi
                ;;
            *)
                echo "Skipped. To enable Disk Usage later, run: $baobab_command"
                ;;
        esac
    fi
fi
