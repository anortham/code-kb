#!/usr/bin/env bash
#
# restore-julie-extract.sh — restore the pinned julie-extract binary into .tools/.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PINS="${SCRIPT_DIR}/julie-pins.json"
TOOLS_DIR="${REPO_ROOT}/.tools"

if [[ ! -f "${PINS}" ]]; then
  echo "error: pins file not found at ${PINS}" >&2
  exit 1
fi

read_pin() {
  local expr="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r "${expr}" "${PINS}"
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c "import json, sys, re; data=json.load(open(sys.argv[1])); val=data; [val:=val[k] for k in re.findall(r'[\w-]+', sys.argv[2])]; print(val)" "${PINS}" "$expr"
  else
    echo "error: need either jq or python3 to read ${PINS}" >&2
    exit 1
  fi
}

VERSION="$(read_pin '.version')"

FROM_SOURCE=""
SOURCE_REQUESTED=0
if [[ "${1:-}" == "--from-source" ]]; then
  SOURCE_REQUESTED=1
  shift
  FROM_SOURCE="${1:-${JULIE_EXTRACTORS_SOURCE:-}}"
  if [[ $# -gt 0 ]]; then
    shift
  fi
elif [[ -n "${JULIE_EXTRACTORS_SOURCE:-}" ]]; then
  FROM_SOURCE="${JULIE_EXTRACTORS_SOURCE}"
fi

if [[ "${SOURCE_REQUESTED}" == "1" && -z "${FROM_SOURCE}" ]]; then
  echo "error: --from-source requires a path argument or JULIE_EXTRACTORS_SOURCE" >&2
  exit 1
fi

if [[ -n "${FROM_SOURCE}" ]]; then
  SOURCE_ROOT="$(cd "${FROM_SOURCE}" && pwd)"
  SOURCE_MANIFEST="${SOURCE_ROOT}/Cargo.toml"
  if [[ ! -f "${SOURCE_MANIFEST}" ]]; then
    echo "error: from-source path is not a julie-extractors checkout: ${SOURCE_ROOT}" >&2
    exit 1
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo is required for --from-source restore" >&2
    exit 1
  fi

  mkdir -p "${TOOLS_DIR}"
  BINARY="${TOOLS_DIR}/julie-extract"
  SOURCE_BINARY="${SOURCE_ROOT}/target/release/julie-extract"

  echo "Building julie-extract v${VERSION} from source: ${SOURCE_ROOT}"
  cargo build --manifest-path "${SOURCE_MANIFEST}" --release -p julie-extract-cli --bin julie-extract
  if [[ ! -f "${SOURCE_BINARY}" ]]; then
    echo "error: expected build output not found: ${SOURCE_BINARY}" >&2
    exit 1
  fi

  cp "${SOURCE_BINARY}" "${BINARY}"
  chmod +x "${BINARY}"
  echo "Installed: ${BINARY}"
  "${BINARY}" --version || true
  exit 0
fi

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"
TRIPLE=""
case "${OS}" in
  Darwin)
    case "${ARCH}" in
      arm64|aarch64) TRIPLE="aarch64-apple-darwin" ;;
      x86_64)        TRIPLE="x86_64-apple-darwin" ;;
    esac
    ;;
  Linux)
    case "${ARCH}" in
      x86_64) TRIPLE="x86_64-unknown-linux-gnu" ;;
    esac
    ;;
esac

if [[ -z "${TRIPLE}" ]]; then
  echo "error: unsupported platform '${OS}/${ARCH}' for prebuilt julie-extract v${VERSION}" >&2
  echo "Supported: Linux x86_64, macOS Apple Silicon, macOS Intel, Windows x86_64." >&2
  exit 1
fi

ASSET="$(read_pin ".assets[\"${TRIPLE}\"].name")"
ASSET="${ASSET/\{VER\}/${VERSION}}"
SHA256="$(read_pin ".assets[\"${TRIPLE}\"].sha256")"
URL_TEMPLATE="$(read_pin '.urlTemplate')"

URL="${URL_TEMPLATE/\{VER\}/${VERSION}}"
URL="${URL/\{asset\}/${ASSET}}"

mkdir -p "${TOOLS_DIR}"
ARCHIVE="${TOOLS_DIR}/${ASSET}"
BINARY="${TOOLS_DIR}/julie-extract"

echo "Restoring julie-extract v${VERSION} for ${TRIPLE}..."
echo "  URL:    ${URL}"
echo "  SHA256: ${SHA256}"

curl -fsSL "${URL}" -o "${ARCHIVE}"

verify_sha() {
  local file="$1" expected="$2" actual=""
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${file}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${file}" | awk '{print $1}')"
  else
    echo "error: need sha256sum or shasum to verify download" >&2
    exit 1
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "error: sha256 mismatch for ${file}" >&2
    echo "  expected: ${expected}" >&2
    echo "  actual:   ${actual}" >&2
    rm -f "${file}"
    exit 1
  fi
}
verify_sha "${ARCHIVE}" "${SHA256}"
echo "  Checksum verified."

STAGING="$(mktemp -d "${TOOLS_DIR}/staging.XXXXXX")"
cleanup() {
  rm -rf "${STAGING}" "${ARCHIVE}"
}
trap cleanup EXIT

if [[ "${ARCHIVE}" == *.tar.gz ]]; then
  tar -xzf "${ARCHIVE}" -C "${STAGING}"
elif [[ "${ARCHIVE}" == *.zip ]]; then
  unzip -q "${ARCHIVE}" -d "${STAGING}"
fi

FOUND="$(find "${STAGING}" -type f \( -name "julie-extract" -o -name "julie-extract.exe" \) -print -quit)"
if [[ -z "${FOUND}" ]]; then
  echo "error: julie-extract binary not found in archive" >&2
  exit 1
fi

mv "${FOUND}" "${BINARY}"
chmod +x "${BINARY}"
if [[ "${OS}" == "Darwin" ]]; then
  xattr -d com.apple.quarantine "${BINARY}" 2>/dev/null || true
fi

echo "Successfully installed: ${BINARY}"
"${BINARY}" --version
