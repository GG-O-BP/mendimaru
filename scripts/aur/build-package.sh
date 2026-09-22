#!/usr/bin/env bash
# Runs only inside the disposable Arch builder created by package-gate.sh.
# PKGBUILD supplies depends, makedepends, and pkgver when sourced below.
# shellcheck disable=SC2154
set -euo pipefail
pacman -Syu --noconfirm
useradd --create-home --shell /bin/bash aurbuild
install -d -o aurbuild -g aurbuild /build
cp /gate/PKGBUILD /gate/mendimaru-*.tar /build/
cd /build
# shellcheck disable=SC1091
source ./PKGBUILD
build_dependencies=()
for dependency in "${depends[@]}" "${makedepends[@]}"; do
  # The browser CLI does not start WinBoat. All other dependencies are real
  # repository packages; do not install a fake provider for this AUR-only app.
  [[ "$dependency" == winboat ]] || build_dependencies+=("$dependency")
done
pacman -S --needed --noconfirm "${build_dependencies[@]}"
pacman -T "${build_dependencies[@]}"
chown -R aurbuild:aurbuild /build
# `sccache` replays byte-identical rustc invocations from a mounted cache. It
# is a host build tool, never a package dependency, so the declared depends and
# makedepends above stay exactly what the AUR package ships with, and the
# resulting binary is unchanged. Without a mounted cache the build runs plain.
build_environment=(CARGO_BUILD_JOBS=4)
if [[ -d /sccache ]]; then
  pacman -S --needed --noconfirm sccache
  chown -R aurbuild:aurbuild /sccache
  build_environment+=(
    RUSTC_WRAPPER=/usr/bin/sccache
    SCCACHE_DIR=/sccache
    SCCACHE_CACHE_SIZE=3G
    SCCACHE_IDLE_TIMEOUT=0
  )
fi
runuser -u aurbuild -- env "${build_environment[@]}" makepkg --nodeps --noconfirm
if [[ -d /sccache ]]; then
  runuser -u aurbuild -- env SCCACHE_DIR=/sccache /usr/bin/sccache --show-stats || true
  runuser -u aurbuild -- env SCCACHE_DIR=/sccache /usr/bin/sccache --stop-server >/dev/null || true
fi
packages=(/build/mendimaru-*.pkg.tar.zst)
[[ ${#packages[@]} == 1 && -f "${packages[0]}" ]]
mkdir /inventory
bsdtar -xf "${packages[0]}" -C /inventory
node /gate/verify-package.mjs "/build/src/mendimaru-$pkgver" /inventory > /output/inventory.json
pacman -Qlp "${packages[0]}" > /output/package-files.txt
cp "${packages[0]}" /output/
