#!/usr/bin/env bash
#
# Prepare, validate, commit, or publish the mendimaru AUR package for one
# tagged GitHub release. The default mode prepares a local checkout only.

# Keep `sh scripts/update-mendimaru-aur.sh` working when /bin/sh is not Bash.
if [ -z "${BASH_VERSION:-}" ]; then
    exec bash "$0" "$@"
fi

set -Eeuo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
PACKAGE_NAME="${AUR_PACKAGE_NAME:-mendimaru}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-GG-O-BP/mendimaru}"
RELEASE_TAG="${MENDIMARU_RELEASE_TAG:-${GITHUB_REF_NAME:-}}"
AUR_TEMPLATE_DIR="${AUR_TEMPLATE_DIR:-$REPOSITORY_DIR/aur}"
AUR_WORK_DIR="${AUR_WORK_DIR:-$REPOSITORY_DIR/.aur-work}"
AUR_FETCH_BASE="${AUR_FETCH_BASE:-https://aur.archlinux.org}"
AUR_PUSH_BASE="${AUR_PUSH_BASE:-ssh://aur@aur.archlinux.org}"
GITHUB_TOKEN_VALUE="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

MODE="prepare"
VERIFY_SOURCE=0
INITIALIZE_EMPTY=0
OFFLINE=0
TMP_DIR=""
CHECKOUT_DIR=""
UPSTREAM_REF=""
UPSTREAM_VERSION=""
UPSTREAM_PKGREL=""
PACKAGE_RELEASE=1
ARCHIVE_SHA256=""

usage() {
    cat <<'EOF'
Usage:
  bash scripts/update-mendimaru-aur.sh [options]

Options:
  --tag TAG             GitHub release tag to package (for example v0.1.0)
  --work-dir PATH       AUR checkout directory
  --verify-source       Run makepkg --verifysource after rendering the package
  --initialize-empty    Allow the first commit to an empty AUR repository
  --offline             Prepare files without contacting AUR
  --commit              Create a local AUR commit, but do not push
  --push                Commit and push the package to AUR
  -h, --help            Show this help

Environment:
  GITHUB_TOKEN or GH_TOKEN
                        Optional GitHub API token
  GITHUB_REPOSITORY     Upstream owner/repository
                        (default: GG-O-BP/mendimaru)
  GITHUB_API_BASE       Override the GitHub Releases API base
  MENDIMARU_RELEASE_TAG Same as --tag
  AUR_PACKAGE_NAME      AUR package name (default: mendimaru)
  AUR_TEMPLATE_DIR      PKGBUILD template directory
  AUR_WORK_DIR          Same as --work-dir
  AUR_FETCH_BASE        AUR read URL base
                        (default: https://aur.archlinux.org)
  AUR_PUSH_BASE         AUR push URL base
                        (default: ssh://aur@aur.archlinux.org)

Examples:
  # Prepare and inspect an existing release
  bash scripts/update-mendimaru-aur.sh --tag v0.1.0

  # First publication
  bash scripts/update-mendimaru-aur.sh \
    --tag v0.1.0 \
    --initialize-empty \
    --verify-source \
    --push

  # Prepare a release while AUR is unavailable
  bash scripts/update-mendimaru-aur.sh \
    --tag v0.1.0 \
    --verify-source \
    --offline
EOF
}

log() {
    printf '%s\n' "$*"
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then
        rm -rf -- "$TMP_DIR"
    fi
}

trap cleanup EXIT

require_command() {
    command -v "$1" >/dev/null 2>&1 ||
        die "required command not found: $1"
}

while (($# > 0)); do
    case "$1" in
        --tag)
            (($# >= 2)) || die "--tag requires a value"
            RELEASE_TAG="$2"
            shift 2
            ;;
        --tag=*)
            RELEASE_TAG="${1#*=}"
            shift
            ;;
        --work-dir)
            (($# >= 2)) || die "--work-dir requires a path"
            AUR_WORK_DIR="$2"
            shift 2
            ;;
        --work-dir=*)
            AUR_WORK_DIR="${1#*=}"
            shift
            ;;
        --verify-source)
            VERIFY_SOURCE=1
            shift
            ;;
        --initialize-empty)
            INITIALIZE_EMPTY=1
            shift
            ;;
        --offline)
            OFFLINE=1
            shift
            ;;
        --commit)
            MODE="commit"
            shift
            ;;
        --push)
            MODE="push"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

if ((OFFLINE == 1)) && [[ "$MODE" != "prepare" ]]; then
    die "--offline cannot be combined with --commit or --push"
fi

[[ "$GITHUB_REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] ||
    die "invalid GITHUB_REPOSITORY: $GITHUB_REPOSITORY"
[[ "$PACKAGE_NAME" =~ ^[a-z0-9@._+-]+$ ]] ||
    die "invalid AUR_PACKAGE_NAME: $PACKAGE_NAME"
[[ "$RELEASE_TAG" =~ ^v([0-9]+(\.[0-9]+){2})$ ]] ||
    die "release tag must be v-prefixed semantic version: ${RELEASE_TAG:-<empty>}"

VERSION="${BASH_REMATCH[1]}"

for command_name in bash cmp cp curl git jq makepkg sha256sum sed grep awk mktemp tar; do
    require_command "$command_name"
done

[[ -f "$AUR_TEMPLATE_DIR/PKGBUILD" ]] ||
    die "AUR PKGBUILD template not found: $AUR_TEMPLATE_DIR/PKGBUILD"

mkdir -p -- "$AUR_WORK_DIR"
AUR_WORK_DIR="$(cd -- "$AUR_WORK_DIR" && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mendimaru-aur-update.XXXXXXXX")"

API_BASE="${GITHUB_API_BASE:-https://api.github.com/repos/$GITHUB_REPOSITORY}"
WEB_BASE="https://github.com/$GITHUB_REPOSITORY"
RELEASE_JSON="$TMP_DIR/release.json"
SOURCE_ARCHIVE="$TMP_DIR/$PACKAGE_NAME-$VERSION.tar.gz"

api_download() {
    local url="$1"
    local destination="$2"
    local -a headers=(
        -H "Accept: application/vnd.github+json"
        -H "X-GitHub-Api-Version: 2022-11-28"
        -H "User-Agent: mendimaru-aur-updater"
    )

    if [[ -n "$GITHUB_TOKEN_VALUE" ]]; then
        headers+=(-H "Authorization: Bearer $GITHUB_TOKEN_VALUE")
    fi

    curl \
        --fail \
        --silent \
        --show-error \
        --location \
        --retry 3 \
        --retry-all-errors \
        "${headers[@]}" \
        --output "$destination" \
        "$url"
}

download_file() {
    local url="$1"
    local destination="$2"

    curl \
        --fail \
        --silent \
        --show-error \
        --location \
        --retry 3 \
        --retry-all-errors \
        --output "$destination" \
        "$url"
}

read_assignment() {
    local file="$1"
    local name="$2"
    local line
    local count

    count="$(grep -Ec "^${name}=" "$file" || true)"
    [[ "$count" == "1" ]] ||
        die "expected one ${name}= assignment in $file, found $count"

    line="$(grep -E "^${name}=" "$file")"
    line="${line#*=}"
    line="${line#\"}"
    line="${line%\"}"
    line="${line#\'}"
    line="${line%\'}"
    printf '%s\n' "$line"
}

replace_assignment() {
    local file="$1"
    local name="$2"
    local value="$3"
    local count

    count="$(grep -Ec "^${name}=" "$file" || true)"
    [[ "$count" == "1" ]] ||
        die "expected one ${name}= assignment in $file, found $count"

    sed -E -i "s|^${name}=.*$|${name}=${value}|" "$file"
}

validate_srcinfo_contract() {
    local srcinfo="$1"
    local package_count

    package_count="$(grep -Ec '^[[:space:]]*pkgname = ' "$srcinfo" || true)"
    [[ "$package_count" == "1" ]] ||
        die "$srcinfo must describe exactly one package"
    grep -Eq '^[[:space:]]*pkgname = mendimaru$' "$srcinfo" ||
        die "$srcinfo does not describe the mendimaru package"
    [[ "$(grep -Ec '^[[:space:]]*depends = winboat$' "$srcinfo" || true)" == "1" ]] ||
        die "$srcinfo does not require winboat"
}

archive_member_to_file() {
    local member="$1"
    local destination="$2"

    tar -xOf "$SOURCE_ARCHIVE" "$member" >"$destination" ||
        die "release archive is missing $member"
}

validate_archive_versions() {
    local listing="$TMP_DIR/archive.list"
    local archive_root
    local package_json="$TMP_DIR/package.json"
    local cargo_toml="$TMP_DIR/Cargo.toml"
    local tauri_config="$TMP_DIR/tauri.conf.json"
    local package_version
    local cargo_version
    local tauri_version

    tar -tzf "$SOURCE_ARCHIVE" >"$listing"
    archive_root="$(awk -F/ 'NF { print $1; exit }' "$listing")"
    [[ -n "$archive_root" ]] || die "release archive is empty"

    archive_member_to_file "$archive_root/package.json" "$package_json"
    archive_member_to_file "$archive_root/src-tauri/Cargo.toml" "$cargo_toml"
    archive_member_to_file \
        "$archive_root/src-tauri/tauri.conf.json" \
        "$tauri_config"

    package_version="$(jq -er '.version' "$package_json")"
    cargo_version="$(
        sed -n -E 's/^version = "([^"]+)"$/\1/p' "$cargo_toml" |
            head -n 1
    )"
    tauri_version="$(jq -er '.version' "$tauri_config")"

    [[ "$package_version" == "$VERSION" ]] ||
        die "package.json version $package_version does not match $RELEASE_TAG"
    [[ "$cargo_version" == "$VERSION" ]] ||
        die "Cargo.toml version $cargo_version does not match $RELEASE_TAG"
    [[ "$tauri_version" == "$VERSION" ]] ||
        die "tauri.conf.json version $tauri_version does not match $RELEASE_TAG"

    grep -Fxq "$archive_root/LICENSE" "$listing" ||
        die "release archive is missing LICENSE"
}

resolve_release() {
    local actual_tag
    local archive_url

    log "==> Resolving GitHub release: $RELEASE_TAG"
    api_download \
        "$API_BASE/releases/tags/$RELEASE_TAG" \
        "$RELEASE_JSON"

    actual_tag="$(jq -er '.tag_name' "$RELEASE_JSON")"
    [[ "$actual_tag" == "$RELEASE_TAG" ]] ||
        die "GitHub returned release tag $actual_tag, expected $RELEASE_TAG"
    [[ "$(jq -er '.draft' "$RELEASE_JSON")" == "false" ]] ||
        die "GitHub release is still a draft: $RELEASE_TAG"
    [[ "$(jq -er '.prerelease' "$RELEASE_JSON")" == "false" ]] ||
        die "GitHub release is a prerelease: $RELEASE_TAG"

    archive_url="$WEB_BASE/archive/refs/tags/$RELEASE_TAG.tar.gz"
    download_file "$archive_url" "$SOURCE_ARCHIVE"
    ARCHIVE_SHA256="$(sha256sum "$SOURCE_ARCHIVE" | awk '{print $1}')"
    [[ "$ARCHIVE_SHA256" =~ ^[0-9a-f]{64}$ ]] ||
        die "release archive returned an invalid SHA-256"

    validate_archive_versions
}

has_head() {
    git -C "$1" rev-parse --verify HEAD >/dev/null 2>&1
}

load_package_metadata() {
    local directory="$1"
    local ref="$2"
    local pkgbuild="$TMP_DIR/upstream-PKGBUILD"
    local srcinfo="$TMP_DIR/upstream-SRCINFO"
    local validation_dir="$TMP_DIR/upstream-validation"
    local generated_srcinfo="$validation_dir/.SRCINFO.generated"

    git -C "$directory" show "$ref:PKGBUILD" >"$pkgbuild" ||
        die "$ref does not contain PKGBUILD"
    git -C "$directory" show "$ref:.SRCINFO" >"$srcinfo" ||
        die "$ref does not contain .SRCINFO"

    mkdir -p -- "$validation_dir"
    cp -- "$pkgbuild" "$validation_dir/PKGBUILD"
    (
        cd -- "$validation_dir"
        makepkg --printsrcinfo >"$generated_srcinfo"
        makepkg --packagelist >/dev/null
    )
    cmp -s "$srcinfo" "$generated_srcinfo" ||
        die "$ref contains a stale .SRCINFO"
    validate_srcinfo_contract "$srcinfo"

    [[ "$(read_assignment "$pkgbuild" pkgname)" == "$PACKAGE_NAME" ]] ||
        die "$ref contains an unexpected package name"
    UPSTREAM_VERSION="$(read_assignment "$pkgbuild" pkgver)"
    UPSTREAM_PKGREL="$(read_assignment "$pkgbuild" pkgrel)"
    [[ "$UPSTREAM_PKGREL" =~ ^[0-9]+$ ]] ||
        die "pkgrel is not numeric in $ref:PKGBUILD"
}

prepare_checkout() {
    local checkout="$AUR_WORK_DIR/$PACKAGE_NAME"
    local status
    local ahead
    local behind

    if [[ -e "$checkout" && ! -d "$checkout/.git" ]]; then
        die "$checkout exists but is not an AUR Git checkout"
    fi

    if ((OFFLINE == 1)); then
        log "==> Preparing offline AUR checkout: $PACKAGE_NAME"
        if [[ ! -d "$checkout/.git" ]]; then
            mkdir -p -- "$checkout"
            git -C "$checkout" init -b master
        fi
        status="$(git -C "$checkout" status --porcelain)"
        [[ -z "$status" ]] ||
            die "offline AUR checkout has uncommitted changes: $checkout"$'\n'"$status"
        CHECKOUT_DIR="$checkout"
        return
    fi

    if [[ ! -d "$checkout/.git" ]]; then
        log "==> Cloning AUR package: $PACKAGE_NAME"
        git -c init.defaultBranch=master clone \
            "${AUR_FETCH_BASE%/}/${PACKAGE_NAME}.git" \
            "$checkout"
    else
        log "==> Updating AUR checkout: $PACKAGE_NAME"
    fi

    status="$(git -C "$checkout" status --porcelain)"
    [[ -z "$status" ]] ||
        die "AUR checkout has uncommitted changes: $checkout"$'\n'"$status"

    if ! git -C "$checkout" remote get-url source >/dev/null 2>&1; then
        git -C "$checkout" remote add \
            source \
            "${AUR_FETCH_BASE%/}/${PACKAGE_NAME}.git"
    fi
    git -C "$checkout" remote set-url \
        source \
        "${AUR_FETCH_BASE%/}/${PACKAGE_NAME}.git"

    if git -C "$checkout" remote get-url origin >/dev/null 2>&1; then
        git -C "$checkout" remote set-url \
            origin \
            "${AUR_PUSH_BASE%/}/${PACKAGE_NAME}.git"
    else
        git -C "$checkout" remote add \
            origin \
            "${AUR_PUSH_BASE%/}/${PACKAGE_NAME}.git"
    fi

    git -C "$checkout" fetch --prune source

    if git -C "$checkout" show-ref \
        --verify \
        --quiet \
        refs/remotes/source/master; then
        UPSTREAM_REF="source/master"
        if has_head "$checkout"; then
            ahead="$(git -C "$checkout" rev-list --count "$UPSTREAM_REF..HEAD")"
            behind="$(git -C "$checkout" rev-list --count "HEAD..$UPSTREAM_REF")"
            if ((ahead > 0 && behind > 0)); then
                die "$checkout has diverged from $UPSTREAM_REF"
            fi
            if ((ahead > 0)) && [[ "$MODE" != "push" ]]; then
                die "$checkout has $ahead unpushed commit(s); rerun with --push"
            fi
            if ((behind > 0)); then
                git -C "$checkout" merge --ff-only "$UPSTREAM_REF"
            fi
        else
            git -C "$checkout" checkout -B master "$UPSTREAM_REF"
        fi
        load_package_metadata "$checkout" "$UPSTREAM_REF"
    else
        ((INITIALIZE_EMPTY == 1)) ||
            die "AUR repository is empty; rerun with --initialize-empty"
        if has_head "$checkout"; then
            load_package_metadata "$checkout" HEAD
        fi
    fi

    CHECKOUT_DIR="$checkout"
}

generate_srcinfo() {
    local generated="$TMP_DIR/.SRCINFO"

    (
        cd -- "$CHECKOUT_DIR"
        makepkg --printsrcinfo >"$generated"
        makepkg --packagelist >/dev/null
    )
    [[ -s "$generated" ]] || die "makepkg generated an empty .SRCINFO"
    validate_srcinfo_contract "$generated"
    cp -- "$generated" "$CHECKOUT_DIR/.SRCINFO"
}

package_files_changed() {
    if has_head "$CHECKOUT_DIR"; then
        if git -C "$CHECKOUT_DIR" diff --quiet HEAD -- PKGBUILD .SRCINFO; then
            return 1
        fi
        return 0
    fi

    [[ -f "$CHECKOUT_DIR/PKGBUILD" || -f "$CHECKOUT_DIR/.SRCINFO" ]]
}

render_package() {
    local pkgbuild="$CHECKOUT_DIR/PKGBUILD"

    log "==> Rendering $PACKAGE_NAME $VERSION"
    cp -- "$AUR_TEMPLATE_DIR/PKGBUILD" "$pkgbuild"
    [[ "$(read_assignment "$pkgbuild" pkgname)" == "$PACKAGE_NAME" ]] ||
        die "AUR template contains an unexpected package name"

    replace_assignment "$pkgbuild" pkgver "$VERSION"
    PACKAGE_RELEASE=1
    if [[ "$UPSTREAM_VERSION" == "$VERSION" ]]; then
        PACKAGE_RELEASE="$UPSTREAM_PKGREL"
    fi
    replace_assignment "$pkgbuild" pkgrel "$PACKAGE_RELEASE"
    replace_assignment "$pkgbuild" sha256sums "('$ARCHIVE_SHA256')"
    generate_srcinfo

    if [[ "$UPSTREAM_VERSION" == "$VERSION" ]] && package_files_changed; then
        PACKAGE_RELEASE="$((UPSTREAM_PKGREL + 1))"
        replace_assignment "$pkgbuild" pkgrel "$PACKAGE_RELEASE"
        generate_srcinfo
    fi
}

validate_rendered_package() {
    local generated="$TMP_DIR/.SRCINFO.validated"
    local status
    local changed_files

    (
        cd -- "$CHECKOUT_DIR"
        makepkg --printsrcinfo >"$generated"
        makepkg --packagelist >/dev/null
    )
    cmp -s "$CHECKOUT_DIR/.SRCINFO" "$generated" ||
        die "generated .SRCINFO is stale"
    git -C "$CHECKOUT_DIR" diff --check

    status="$(git -C "$CHECKOUT_DIR" status --porcelain)"
    changed_files="$(
        git -C "$CHECKOUT_DIR" status --porcelain |
            sed -E 's/^...//' |
            LC_ALL=C sort -u
    )"
    if [[ -n "$status" && "$changed_files" != $'.SRCINFO\nPKGBUILD' ]]; then
        die "AUR checkout contains unexpected changes"$'\n'"$status"
    fi

    if ((VERIFY_SOURCE == 1)); then
        log "==> Verifying package sources"
        mkdir -p -- "$TMP_DIR/sources"
        (
            cd -- "$CHECKOUT_DIR"
            SRCDEST="$TMP_DIR/sources" makepkg --verifysource
        )
    fi
}

show_changes() {
    if has_head "$CHECKOUT_DIR"; then
        git -C "$CHECKOUT_DIR" --no-pager diff -- PKGBUILD .SRCINFO
        return
    fi

    for file in PKGBUILD .SRCINFO; do
        git --no-pager diff --no-index -- /dev/null "$CHECKOUT_DIR/$file" || true
    done
}

resolve_release
prepare_checkout
render_package
validate_rendered_package

if package_files_changed; then
    log
    log "--- $PACKAGE_NAME changes ---"
    show_changes
else
    log
    log "$PACKAGE_NAME ${VERSION}-${PACKAGE_RELEASE} is already prepared."
fi

if [[ "$MODE" == "commit" || "$MODE" == "push" ]] && package_files_changed; then
    log
    log "==> Creating AUR commit"
    git -C "$CHECKOUT_DIR" add PKGBUILD .SRCINFO
    git -C "$CHECKOUT_DIR" commit \
        -m "Update $PACKAGE_NAME to ${VERSION}-${PACKAGE_RELEASE}"
fi

if [[ "$MODE" == "push" ]]; then
    has_head "$CHECKOUT_DIR" || die "there is no AUR commit to push"
    log
    log "==> Pushing AUR repository"
    git -C "$CHECKOUT_DIR" push origin HEAD:master
    log
    log "AUR publication complete."
elif [[ "$MODE" == "commit" ]]; then
    log
    log "Local AUR commit created under: $CHECKOUT_DIR"
    log "No repository was pushed."
else
    log
    log "AUR files prepared under: $CHECKOUT_DIR"
    log "No commit or push was performed."
fi
