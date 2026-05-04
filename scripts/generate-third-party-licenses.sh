#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="${BASH_SOURCE[0]%/*}"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
CONFIG_PATH="${ROOT_DIR}/about.toml"
TEMPLATE_PATH="${ROOT_DIR}/about/templates/third-party-licenses.hbs"
OUTPUT_DIR="${ROOT_DIR}/dist"
OUTPUT_PATH="${OUTPUT_DIR}/THIRD_PARTY_LICENSES.txt"
TMP_PATH="${OUTPUT_PATH}.tmp"
ABOUT_BIN=""

if command -v cargo-about >/dev/null 2>&1; then
    ABOUT_BIN="cargo-about"
elif [[ -n "${HOME:-}" && -x "${HOME}/.cargo/bin/cargo-about" ]]; then
    ABOUT_BIN="${HOME}/.cargo/bin/cargo-about"
else
    printf '%s\n' \
        "error: cargo-about is required to generate the third-party license report." \
        "" \
        "Install it with:" \
        "  cargo install --locked cargo-about --features cli" \
        "" \
        "If it is already installed, make sure Cargo's bin directory is on PATH:" \
        "  export PATH=\"\$HOME/.cargo/bin:\$PATH\"" >&2
    exit 127
fi

if [[ ! -f "${ROOT_DIR}/Cargo.lock" ]]; then
    printf '%s\n' \
        "error: Cargo.lock is required for a reproducible license report." \
        "" \
        "Generate or restore ${ROOT_DIR}/Cargo.lock before running this script." >&2
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"
rm -f "${TMP_PATH}"

(
    cd "${ROOT_DIR}"
    LC_ALL=C TZ=UTC "${ABOUT_BIN}" generate \
        --all-features \
        --locked \
        --config "${CONFIG_PATH}" \
        --output-file "${TMP_PATH}" \
        "${TEMPLATE_PATH}"
)

mv "${TMP_PATH}" "${OUTPUT_PATH}"
printf 'Wrote %s\n' "${OUTPUT_PATH}"
