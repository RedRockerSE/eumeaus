#!/usr/bin/env sh
# Installs the latest (or $EUMEAUS_VERSION-pinned) eumeaus release from
# GitHub Releases. See CLI.md for what gets installed and where.
#
#   curl -fsSL https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.sh | sh
#
# Env vars:
#   EUMEAUS_VERSION      pin to a specific release tag (e.g. v0.1.0) instead
#                        of resolving "latest"
#   EUMEAUS_INSTALL_DIR  where to put the eumeaus binary (default: ~/.local/bin)
set -eu

repo="RedRockerSE/eumeaus"
install_dir="${EUMEAUS_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)

case "$os-$arch" in
    Linux-x86_64)
        target="x86_64-unknown-linux-musl"
        archive_ext="tar.gz"
        ;;
    *)
        echo "error: no prebuilt eumeaus binary for $os/$arch yet." >&2
        echo "Build from source instead — see CLI.md's \"Building and running\" section:" >&2
        echo "  https://github.com/$repo/blob/main/CLI.md#building-and-running" >&2
        exit 1
        ;;
esac

version="${EUMEAUS_VERSION:-}"
if [ -z "$version" ]; then
    echo "Resolving the latest eumeaus release..."
    version=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
        | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
fi
if [ -z "$version" ]; then
    echo "error: could not determine the latest eumeaus release (rate-limited by GitHub's API? pass EUMEAUS_VERSION=vX.Y.Z to skip this lookup)" >&2
    exit 1
fi

stage="eumeaus-${version}-${target}"
archive="${stage}.${archive_ext}"
base_url="https://github.com/$repo/releases/download/${version}"

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

echo "Downloading eumeaus ${version} for ${target}..."
curl -fsSL "$base_url/$archive" -o "$tmp_dir/$archive"
curl -fsSL "$base_url/checksums.txt" -o "$tmp_dir/checksums.txt"

expected=$(grep "  ${archive}\$" "$tmp_dir/checksums.txt" | cut -d' ' -f1 || true)
if [ -z "$expected" ]; then
    echo "error: no checksum entry for $archive in checksums.txt — refusing to install an unverifiable download" >&2
    exit 1
fi
actual=$(sha256sum "$tmp_dir/$archive" | cut -d' ' -f1)
if [ "$expected" != "$actual" ]; then
    echo "error: checksum mismatch for $archive" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    exit 1
fi
echo "Checksum verified."

tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"
extracted="$tmp_dir/$stage"

mkdir -p "$install_dir"
cp "$extracted/eumeaus" "$install_dir/eumeaus"
chmod +x "$install_dir/eumeaus"

plugin_dir="$install_dir/eumeaus-plugins/username-search"
mkdir -p "$plugin_dir"
cp "$extracted/plugins/username-search/eumeaus-username-search-plugin" "$plugin_dir/"
cp "$extracted/plugins/username-search/plugin.toml" "$plugin_dir/"
chmod +x "$plugin_dir/eumeaus-username-search-plugin"

email_plugin_dir="$install_dir/eumeaus-plugins/email-lookup"
mkdir -p "$email_plugin_dir"
cp "$extracted/plugins/email-lookup/eumeaus-email-lookup-plugin" "$email_plugin_dir/"
cp "$extracted/plugins/email-lookup/plugin.toml" "$email_plugin_dir/"
chmod +x "$email_plugin_dir/eumeaus-email-lookup-plugin"

ip_plugin_dir="$install_dir/eumeaus-plugins/ip-lookup"
mkdir -p "$ip_plugin_dir"
cp "$extracted/plugins/ip-lookup/eumeaus-ip-lookup-plugin" "$ip_plugin_dir/"
cp "$extracted/plugins/ip-lookup/plugin.toml" "$ip_plugin_dir/"
chmod +x "$ip_plugin_dir/eumeaus-ip-lookup-plugin"

echo
echo "Installed eumeaus ${version} to $install_dir/eumeaus"
echo "Installed the bundled username-search plugin to $plugin_dir"
echo "Installed the bundled email-lookup plugin to $email_plugin_dir"
echo "Installed the bundled ip-lookup plugin to $ip_plugin_dir"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *)
        echo
        echo "warning: $install_dir is not on your PATH. Add this to your shell profile:"
        echo "  export PATH=\"$install_dir:\$PATH\""
        ;;
esac

echo
echo "Try it: eumeaus --help"
echo "To run a scan with the bundled plugin: eumeaus scan run --plugins-dir $install_dir/eumeaus-plugins ..."
