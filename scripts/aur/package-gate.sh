#!/usr/bin/env bash
# Build the committed source, then exercise only the installed Arch package.
set -euo pipefail
repository=$(git rev-parse --show-toplevel)
output=${1:-"$repository/artifacts/aur"}
mkdir -p "$output"
output=$(realpath "$output")
scratch=$(mktemp -d)
runtime=""
cleanup() {
  if [[ -n "$runtime" ]]; then docker rm -f "$runtime" >/dev/null; fi
  rm -rf "$scratch"
}
trap cleanup EXIT
# An optional compiler cache only replays byte-identical rustc invocations, so
# the packaged binary is the same with or without it. It stays opt-in because
# the gate must keep working on hosts that have no cache directory to offer.
builder_mounts=()
if [[ -n "${MENDIMARU_AUR_CACHE_DIR:-}" ]]; then
  compiler_cache="$MENDIMARU_AUR_CACHE_DIR/sccache"
  mkdir -p "$compiler_cache"
  compiler_cache=$(realpath "$compiler_cache")
  chmod 0777 "$compiler_cache"
  builder_mounts+=(-v "$compiler_cache:/sccache")
fi
version=$(git show HEAD:package.json | node -p 'JSON.parse(require("fs").readFileSync(0, "utf8")).version')
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
git archive HEAD --prefix="mendimaru-$version/" -o "$scratch/mendimaru-$version.tar"
checksum=$(sha256sum "$scratch/mendimaru-$version.tar" | cut -d ' ' -f 1)
git show HEAD:aur/PKGBUILD | sed \
  -e "s/^pkgver=.*/pkgver=$version/" \
  -e "s|^source=.*|source=(\"mendimaru-$version.tar\")|" \
  -e "s/^sha256sums=.*/sha256sums=('$checksum')/" > "$scratch/PKGBUILD"
cp "$scratch/PKGBUILD" "$output/PKGBUILD"
git rev-parse HEAD > "$output/commit.txt"
for file in build-package.sh verify-package.mjs installed-package.mjs; do
  git show "HEAD:scripts/aur/$file" > "$scratch/$file"
done
docker run --rm --init --cpus=4 --memory=12g \
  -v "$scratch:/gate:ro" -v "$output:/output" \
  ${builder_mounts[@]+"${builder_mounts[@]}"} \
  archlinux:base-devel bash /gate/build-package.sh

# Only the package and standalone Node-built-in smoke fixtures enter this
# second container. It has no checkout, npm, Cargo, or development node_modules.
runtime=$(docker run --detach --init archlinux:base tail -f /dev/null)
packages=("$output"/mendimaru-*.pkg.tar.zst)
[[ ${#packages[@]} == 1 && -f "${packages[0]}" ]]
docker cp "${packages[0]}" "$runtime:/tmp/mendimaru.pkg.tar.zst"
mkdir "$scratch/smoke"
cp "$scratch/installed-package.mjs" "$scratch/smoke/"
for file in fixture-server.mjs smoke.browser.json; do
  git show "HEAD:tests/browser/$file" > "$scratch/smoke/$file"
done
cp "$output/inventory.json" "$scratch/smoke/inventory.json"
docker cp "$scratch/smoke" "$runtime:/smoke"
docker exec "$runtime" bash -c '
  set -euo pipefail
  # Arch container images omit documentation by default. Install every file
  # so pacman -Qkk also verifies the package documentation directories.
  sed -i "/^NoExtract[[:space:]]*=/d" /etc/pacman.conf
  pacman -Syu --noconfirm
  mapfile -t dependencies < <(bsdtar -xOf /tmp/mendimaru.pkg.tar.zst .PKGINFO | sed -n "s/^depend = //p" | sed "/^winboat$/d")
  pacman -S --needed --noconfirm "${dependencies[@]}" ttf-liberation
  pacman -U --noconfirm --assume-installed winboat=0.9.2 /tmp/mendimaru.pkg.tar.zst
  pacman -Qkk mendimaru
  ! command -v npm
  ! command -v cargo
  useradd --create-home --shell /bin/bash smoke
  rm /tmp/mendimaru.pkg.tar.zst
'
docker network disconnect bridge "$runtime"
docker exec --workdir /tmp "$runtime" runuser -u smoke -- \
  node /smoke/installed-package.mjs missing > "$output/doctor-missing.json"
docker network connect bridge "$runtime"
docker exec --workdir /tmp "$runtime" runuser -u smoke -- \
  mendimaru browser install chromium --timeout-seconds 600 --json > "$output/install.json"
docker network disconnect bridge "$runtime"
docker exec --workdir /tmp "$runtime" runuser -u smoke -- \
  node /smoke/installed-package.mjs ready > "$output/smoke.json"
printf 'Installed AUR package gate passed. Evidence: %s\n' "$output"
