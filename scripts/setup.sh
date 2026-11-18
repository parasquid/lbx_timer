#!/usr/bin/env bash
# Prepares a Rust toolchain for developing lbx-timer:
#   1. finds a Rust toolchain that satisfies rust-version in Cargo.toml, or
#      installs one from the distribution's packages (Ubuntu, Debian) or rustup;
#   2. makes sure rustfmt and clippy are available for it;
#   3. adds the toolchain to PATH in your shell profile (asks first).
#
# Usage:
#   scripts/setup.sh                 interactive setup
#   scripts/setup.sh --yes           no questions: install Rust and add PATH
#   scripts/setup.sh --no-profile    never edit shell profiles
#   scripts/setup.sh --find          print the bin directory of a usable toolchain
#                                    (nothing and exit 1 when there is none)
#   scripts/setup.sh --path          print an `export PATH=...` line for `eval`
#   scripts/setup.sh --check         exit 0 when cargo on PATH is new enough
#
# Environment:
#   RUST_MIN_VERSION minimum Rust version before Cargo.toml exists (optional)
#   RUST_PREFIXES   directories searched for versioned toolchains
#                   (default: /usr/lib /usr/local/lib)
#   SUDO            command used to gain root (default: sudo when not root)
#
# Only Ubuntu and Debian package installs are implemented so far; other
# systems fall back to rustup for Rust and get instructions for the rest.
#
# `source scripts/setup.sh` does everything above AND puts the toolchain on
# PATH in the current shell, so cargo works as soon as the script finishes.

# When sourced, run the real script as a child (it uses `set -e`, which must
# not leak into an interactive shell) and then export PATH in this shell.
_lbx_setup_sourced=false
if [ -n "${ZSH_EVAL_CONTEXT-}" ]; then
    case "$ZSH_EVAL_CONTEXT" in *:file*) _lbx_setup_sourced=true ;; esac
    _lbx_setup_script=${(%):-%x}
elif [ -n "${BASH_SOURCE[0]-}" ] && [ "${BASH_SOURCE[0]}" != "$0" ]; then
    _lbx_setup_sourced=true
    _lbx_setup_script=${BASH_SOURCE[0]}
fi
if [ "$_lbx_setup_sourced" = true ]; then
    unset _lbx_setup_sourced
    bash "$_lbx_setup_script" "$@" || { unset _lbx_setup_script; return 1; }
    if _lbx_setup_path=$(bash "$_lbx_setup_script" --path 2>/dev/null) && [ -n "$_lbx_setup_path" ]; then
        eval "$_lbx_setup_path"
    fi
    if command -v cargo >/dev/null 2>&1; then
        echo "$(cargo --version) is on PATH in this shell." >&2
    fi
    unset _lbx_setup_script _lbx_setup_path
    return 0
fi
unset _lbx_setup_sourced

set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
project=$(cd "$here/.." && pwd)

mode=install
assume_yes=false
edit_profile=true
while [ $# -gt 0 ]; do
    case "$1" in
        --find | --path | --check) mode=${1#--} ;;
        --yes | -y) assume_yes=true ;;
        --no-profile) edit_profile=false ;;
        --help | -h) sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "setup: unknown option $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done
# Until a manifest exists, bootstrap an installed toolchain without inventing
# the application's minimum Rust version. Cargo.toml becomes authoritative.
msrv=${RUST_MIN_VERSION:-0}
if [ -f "$project/Cargo.toml" ]; then
    manifest_msrv=$(sed -n 's/^rust-version *= *"\([0-9.]*\)".*/\1/p' "$project/Cargo.toml" | head -n 1)
    if [ -n "$manifest_msrv" ]; then
        msrv=$manifest_msrv
    fi
fi
if ! [[ "$msrv" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
    echo "setup: invalid minimum Rust version: $msrv" >&2
    exit 2
fi

say() { [ "$mode" = "install" ] && echo "$*" >&2 || true; }
interactive() { [ "$assume_yes" = false ] && [ -t 0 ] && [ -t 2 ]; }

# ask "Question?" default(y|n) -> exit status 0 for yes
ask() {
    local question=$1 default=${2:-n} answer
    if ! interactive; then
        [ "$default" = y ]
        return
    fi
    if [ "$default" = y ]; then
        printf '%s [Y/n] ' "$question" >&2
    else
        printf '%s [y/N] ' "$question" >&2
    fi
    read -r answer || answer=""
    answer=${answer:-$default}
    [[ "$answer" =~ ^[Yy] ]]
}

# version_ok 1.91.1 1.88 -> true when the first is at least the second.
version_ok() {
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]
}

tool_version() { # tool_version <dir-or-empty> <tool> -> "1.91.1" or ""
    local tool=$2
    [ -n "$1" ] && tool="$1/$2"
    "$tool" --version 2>/dev/null | sed -n 's/^[a-z]* \([0-9][0-9.]*\).*/\1/p' | head -n 1
}

# usable_dir <dir-or-empty> -> prints the cargo version when both cargo and
# rustc there satisfy the MSRV.
usable_dir() {
    local cargo rustc
    cargo=$(tool_version "$1" cargo) || true
    rustc=$(tool_version "$1" rustc) || true
    [ -n "$cargo" ] && [ -n "$rustc" ] || return 1
    version_ok "$cargo" "$msrv" && version_ok "$rustc" "$msrv" || return 1
    echo "$cargo"
}

find_toolchain() { # prints "<dir>" ("" means PATH) of the best toolchain, or fails
    if usable_dir "" >/dev/null; then
        echo ""
        return 0
    fi
    local best="" best_version="0" dir version candidate
    # Discover managed installations without changing global defaults or trust.
    if command -v mise >/dev/null 2>&1; then
        candidate=$(mise which cargo 2>/dev/null) || candidate=""
        if [ -n "$candidate" ]; then
            dir=$(dirname "$candidate")
            if usable_dir "$dir" >/dev/null; then
                echo "$dir"
                return 0
            fi
        fi
    fi
    if command -v rustup >/dev/null 2>&1; then
        candidate=$(rustup which --toolchain stable cargo 2>/dev/null) || candidate=""
        if [ -n "$candidate" ]; then
            dir=$(dirname "$candidate")
            if usable_dir "$dir" >/dev/null; then
                echo "$dir"
                return 0
            fi
        fi
    fi
    for prefix in ${RUST_PREFIXES:-/usr/lib /usr/local/lib} "$HOME/.cargo"; do
        for dir in "$prefix"/rust-*/bin "$prefix"/bin; do
            [ -x "$dir/cargo" ] || continue
            version=$(usable_dir "$dir") || continue
            if version_ok "$version" "$best_version" && [ "$version" != "$best_version" ]; then
                best=$dir
                best_version=$version
            fi
        done
    done
    [ -n "$best" ] || return 1
    echo "$best"
}

missing_components() { # missing_components <dir-or-empty> -> "rustfmt clippy" not found
    local dir=$1 missing=""
    if [ -n "$dir" ]; then
        [ -x "$dir/cargo-fmt" ] || missing="$missing rustfmt"
        [ -x "$dir/cargo-clippy" ] || missing="$missing clippy"
    else
        cargo fmt --version >/dev/null 2>&1 || missing="$missing rustfmt"
        cargo clippy --version >/dev/null 2>&1 || missing="$missing clippy"
    fi
    echo "$missing"
}

as_root() {
    if [ "$(id -u)" = "0" ]; then
        "$@"
    elif [ -n "${SUDO-sudo}" ] && command -v "${SUDO-sudo}" >/dev/null 2>&1; then
        "${SUDO-sudo}" "$@"
    else
        echo "setup: root is needed to run: $*" >&2
        echo "setup: rerun as root, install sudo, or run that command yourself." >&2
        return 1
    fi
}

distro_id() { sed -n 's/^ID=//p' /etc/os-release 2>/dev/null | tr -d '"'; }
has_apt() { command -v apt-get >/dev/null 2>&1 && [ "$(distro_id)" = ubuntu -o "$(distro_id)" = debian ]; }

apt_updated=false
apt_update() {
    $apt_updated && return 0
    say "Refreshing the package lists…"
    as_root apt-get update -qq >/dev/null 2>&1 || as_root apt-get update >/dev/null || return 1
    apt_updated=true
}

apt_install() {
    say "Installing: $*"
    as_root apt-get install -y "$@"
}

# Ubuntu keeps the newer toolchains in "universe".
ensure_universe() {
    [ "$(distro_id)" = ubuntu ] || return 0
    if apt-cache policy 2>/dev/null | grep -q '/universe'; then
        return 0
    fi
    say "The 'universe' component is not enabled; it holds the newer Rust toolchains."
    if command -v add-apt-repository >/dev/null 2>&1; then
        as_root add-apt-repository -y universe >/dev/null && apt_updated=false
    else
        say "Enable it with: sudo add-apt-repository universe    (package software-properties-common)"
        return 1
    fi
}

# Ubuntu ships side-by-side toolchains as rustc-1.NN in noble-updates/universe.
apt_packages_for_ubuntu() {
    local candidates best=""
    candidates=$(apt-cache search --names-only '^cargo-1\.[0-9]+$' 2>/dev/null | sed -n 's/^cargo-\([0-9.]*\) .*/\1/p' | sort -V)
    for version in $candidates; do
        version_ok "$version" "$msrv" && best=$version
    done
    [ -n "$best" ] || return 1
    echo "rustc-$best cargo-$best rustfmt-$best rust-$best-clippy"
}

# Debian 13 ships a newer "web" toolchain that replaces /usr/bin/rustc.
apt_packages_for_debian() {
    local version
    version=$(apt-cache policy cargo-web 2>/dev/null | sed -n 's/^ *Candidate: *\([0-9.]*\).*/\1/p')
    [ -n "$version" ] && version_ok "$version" "$msrv" || return 1
    echo "rustc-web cargo-web rustfmt-web rust-web-clippy"
}

find_rust_packages() {
    case "$(distro_id)" in
        ubuntu) apt_packages_for_ubuntu ;;
        debian) apt_packages_for_debian ;;
        *) return 1 ;;
    esac
}

install_rust_with_apt() {
    has_apt || return 1
    ensure_universe || true
    local packages
    packages=$(find_rust_packages) || true
    if [ -z "$packages" ]; then
        apt_update || true
        packages=$(find_rust_packages) || true
    fi
    if [ -z "$packages" ]; then
        say "No package with Rust $msrv or newer is visible to apt."
        case "$(distro_id)" in
            ubuntu) say "On Ubuntu LTS they come from the -updates suite; check /etc/apt/sources.list includes it." ;;
            debian) say "Debian 13 (trixie) has rustc-web; Debian 12 has nothing new enough. Use rustup instead." ;;
        esac
        return 1
    fi
    apt_update || true
    apt_install $packages
}

install_rust_with_rustup() {
    command -v rustup >/dev/null 2>&1 || return 1
    say "Installing the stable toolchain with rustup…"
    rustup toolchain install stable --profile minimal -c rustfmt -c clippy
    rustup default stable >/dev/null
}

install_components() { # install_components <dir-or-empty>
    local dir=$1 missing version
    missing=$(missing_components "$dir")
    [ -n "$missing" ] || return 0
    if [ -n "$dir" ] && has_apt; then
        version=$(basename "$(dirname "$dir")" | sed -n 's/^rust-//p')
        case "$(distro_id)" in
            ubuntu) [ -n "$version" ] && apt_install rustfmt-"$version" rust-"$version"-clippy && return 0 ;;
            debian) apt_install rustfmt-web rust-web-clippy && return 0 ;;
        esac
    elif [ -z "$dir" ] && command -v rustup >/dev/null 2>&1; then
        rustup component add rustfmt clippy && return 0
    fi
    say "Missing components:$missing. 'cargo fmt' / 'cargo clippy' will not work until they are installed."
}

# ---------------------------------------------------------------- PATH profile

profile_file() {
    case "$(basename "${SHELL:-bash}")" in
        zsh) echo "$HOME/.zshrc" ;;
        fish) echo "$HOME/.config/fish/config.fish" ;;
        *) echo "$HOME/.bashrc" ;;
    esac
}

shell_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

profile_line() { # profile_line <dir>
    if [ "$(basename "${SHELL:-bash}")" = fish ]; then
        printf 'fish_add_path --prepend %s\n' "$(shell_quote "$1")"
    else
        printf 'export PATH=%s:$PATH\n' "$(shell_quote "$1")"
    fi
}

add_to_profile() { # add_to_profile <dir>
    local dir=$1 file line marker="# lbx-timer: Rust toolchain"
    file=$(profile_file)
    line=$(profile_line "$dir")
    if [ -f "$file" ] && grep -qF "$line" "$file"; then
        say "$file already puts $dir on PATH."
        return 0
    fi
    mkdir -p "$(dirname "$file")"
    printf '\n%s\n%s\n' "$marker" "$line" >>"$file"
    say "Added to $file:"
    say "    $line"
}

# ---------------------------------------------------------------- main

case "$mode" in
    find)
        dir=$(find_toolchain) || exit 1
        echo "$dir"
        ;;
    path)
        dir=$(find_toolchain) || { echo "setup: no Rust $msrv+ toolchain found; run scripts/setup.sh" >&2; exit 1; }
        if [ -n "$dir" ]; then
            printf 'export PATH=%q:$PATH\n' "$dir"
        fi
        ;;
    check)
        usable_dir "" >/dev/null || { echo "setup: cargo/rustc on PATH is unavailable or below Rust $msrv; run: eval \"\$(scripts/setup.sh --path)\"" >&2; exit 1; }
        ;;
    install)
        if [ "$msrv" = 0 ]; then
            say "No minimum Rust version declared yet; checking the available toolchain."
        fi
        if dir=$(find_toolchain); then
            say "Found Rust $(tool_version "$dir" cargo) (project needs $msrv or newer)."
        else
            say "No Rust $msrv+ toolchain found (project rust-version = $msrv)."
            install_rust_with_apt || install_rust_with_rustup || {
                say "Could not install Rust automatically. Install Rust $msrv or newer (https://rustup.rs), then rerun."
                exit 1
            }
            dir=$(find_toolchain) || { say "Installation finished but no usable toolchain was found."; exit 1; }
        fi
        install_components "$dir"

        if [ -n "$dir" ]; then
            say ""
            say "Rust is installed under $dir but is not first on your PATH."
            if [ "$edit_profile" = true ] && ask "Add it to $(profile_file) so every new shell finds it?" y; then
                add_to_profile "$dir"
            fi
            say "For the current shell run one of:"
            say "    source scripts/setup.sh        (does the setup and the export together)"
            say "    $(profile_line "$dir")"
        else
            say "cargo on PATH is ready."
        fi

        say ""
        if [ -f "$project/Cargo.toml" ]; then
            say "Done. Next: cargo check"
        else
            say "Rust bootstrap is ready. No Cargo.toml yet; app and GUI dependencies will follow."
        fi
        ;;
esac
