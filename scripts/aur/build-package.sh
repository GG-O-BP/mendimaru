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
runuser -u aurbuild -- env CARGO_BUILD_JOBS=2 makepkg --nodeps --noconfirm
packages=(/build/mendimaru-*.pkg.tar.zst)
[[ ${#packages[@]} == 1 && -f "${packages[0]}" ]]
mkdir /inventory
bsdtar -xf "${packages[0]}" -C /inventory
node /gate/verify-package.mjs "/build/src/mendimaru-$pkgver" /inventory > /output/inventory.json
pacman -Qlp "${packages[0]}" > /output/package-files.txt
cp "${packages[0]}" /output/
