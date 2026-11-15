#!/usr/bin/env bash
# Bottom-up crates.io publish for the Lemma workspace.
#
# Usage: publish-crates.sh <semver>
# Requires: CARGO_REGISTRY_TOKEN, cargo on PATH.
#
# Guarantees:
# - Idempotent: a crate version that already exists on crates.io is treated as success
#   so "Re-run failed jobs" can finish a partial release.
# - After each publish, waits until cargo's crates.io index can resolve that version
#   (the failure mode of a fixed sleep). Polls the sparse index cargo uses, busts the
#   local registry cache, then confirms with `cargo package` of the next crate.

set -euo pipefail

VERSION="${1:?usage: publish-crates.sh <semver>}"
: "${CARGO_REGISTRY_TOKEN:?CARGO_REGISTRY_TOKEN must be set}"

CRATES=(lemma-engine lemma-lsp lemma-openapi lemma)
# Max wait per edge: 90 * 10s = 15 minutes.
POLL_ATTEMPTS="${CRATES_IO_POLL_ATTEMPTS:-90}"
POLL_SLEEP_SECS="${CRATES_IO_POLL_SLEEP_SECS:-10}"

cargo_home="${CARGO_HOME:-${HOME}/.cargo}"

sparse_index_path() {
  local name="$1"
  local len=${#name}
  if ((len <= 2)); then
    printf '%s/%s' "${len}" "${name}"
  elif ((len == 3)); then
    printf '3/%s/%s' "${name:0:1}" "${name}"
  else
    printf '%s/%s/%s' "${name:0:2}" "${name:2:2}" "${name}"
  fi
}

bust_registry_cache() {
  # Stale sparse-index snapshots are why a fixed sleep fails: cargo keeps reading
  # the cached index even after crates.io has the new version.
  rm -rf \
    "${cargo_home}/registry/index" \
    "${cargo_home}/registry/cache" \
    "${cargo_home}/registry/src"
}

sparse_index_has_version() {
  local crate="$1"
  local version="$2"
  local url="https://index.crates.io/$(sparse_index_path "${crate}")"
  local body
  if ! body="$(curl -fsSL -A "lemma-release-ci" "${url}")"; then
    return 1
  fi
  # Sparse index: one JSON object per line; match the exact vers field.
  printf '%s\n' "${body}" | grep -F "\"vers\":\"${version}\"" >/dev/null
}

# Faithful probe: same path+version rewrite resolution `cargo publish` runs for the
# next crate. --no-verify skips the compile; we only need index resolution.
package_resolves() {
  local pkg="$1"
  local out
  set +e
  out="$(cargo package -p "${pkg}" --no-verify --allow-dirty 2>&1)"
  local ec=$?
  set -e
  if ((ec == 0)); then
    return 0
  fi
  # Still waiting on the dependency index.
  if printf '%s\n' "${out}" | grep -Eq 'failed to select a version for the requirement|failed to prepare local package'; then
    return 1
  fi
  # Any other package failure is a real bug; surface it.
  printf '%s\n' "${out}" >&2
  return 2
}

wait_for_index() {
  local published_crate="$1"
  local next_pkg="$2"
  local attempt=1
  echo "Waiting until crates.io index serves ${published_crate}@${VERSION} (for ${next_pkg})..."
  while ((attempt <= POLL_ATTEMPTS)); do
    bust_registry_cache
    if sparse_index_has_version "${published_crate}" "${VERSION}"; then
      local resolve_ec=0
      package_resolves "${next_pkg}" || resolve_ec=$?
      if ((resolve_ec == 0)); then
        echo "Index ready: ${published_crate}@${VERSION} resolves for ${next_pkg}"
        return 0
      fi
      if ((resolve_ec == 2)); then
        echo "error: cargo package -p ${next_pkg} failed for a reason other than index lag" >&2
        exit 1
      fi
      echo "sparse index lists ${VERSION}; cargo package still lagging (attempt ${attempt}/${POLL_ATTEMPTS})"
    else
      echo "sparse index missing ${published_crate}@${VERSION} (attempt ${attempt}/${POLL_ATTEMPTS})"
    fi
    sleep "${POLL_SLEEP_SECS}"
    attempt=$((attempt + 1))
  done
  echo "error: timed out after $((POLL_ATTEMPTS * POLL_SLEEP_SECS))s waiting for ${published_crate}@${VERSION}" >&2
  exit 1
}

publish_one() {
  local pkg="$1"
  echo "Publishing ${pkg}@${VERSION}..."
  local out
  set +e
  out="$(cargo publish -p "${pkg}" 2>&1)"
  local ec=$?
  set -e
  printf '%s\n' "${out}"
  if ((ec == 0)); then
    return 0
  fi
  if printf '%s\n' "${out}" | grep -Eq 'already exists on crates\.io'; then
    echo "${pkg}@${VERSION} already on crates.io; continuing"
    return 0
  fi
  echo "error: cargo publish -p ${pkg} failed" >&2
  exit "${ec}"
}

for i in "${!CRATES[@]}"; do
  pkg="${CRATES[$i]}"
  publish_one "${pkg}"
  next_index=$((i + 1))
  if ((next_index < ${#CRATES[@]})); then
    wait_for_index "${pkg}" "${CRATES[$next_index]}"
  fi
done

echo "All crates published for ${VERSION}."
