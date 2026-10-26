#!/usr/bin/env bash
set -euo pipefail

retry_delay=${BUILDX_RETRY_DELAY_SECONDS:-15}
if [[ ! $retry_delay =~ ^[0-9]+$ ]]; then
  echo "BUILDX_RETRY_DELAY_SECONDS must be a non-negative integer" >&2
  exit 2
fi

args=(
  buildx build
  --push
  --progress plain
  --platform linux/amd64,linux/arm64
  --build-arg CARGO_BUILD_JOBS=1
)

if [[ -n ${BUILD_FILE:-} ]]; then
  args+=(--file "$BUILD_FILE")
fi

append_lines() {
  local option=$1
  local values=$2
  local value

  while IFS= read -r value || [[ -n $value ]]; do
    [[ -z $value ]] || args+=("$option" "$value")
  done <<< "$values"
}

append_lines --tag "${BUILD_TAGS:?BUILD_TAGS is required}"
append_lines --label "${BUILD_LABELS:-}"

if [[ -n ${BASE_IMAGE:-} ]]; then
  args+=(--build-arg "BASE_IMAGE=$BASE_IMAGE")
fi

if [[ -n ${BUILD_CACHE_SCOPE:-} ]]; then
  args+=(
    --cache-from "type=gha,scope=$BUILD_CACHE_SCOPE"
    --cache-to "type=gha,scope=$BUILD_CACHE_SCOPE,mode=max"
  )
fi

args+=("${BUILD_CONTEXT:-.}")

attempt=1
max_retries=20
while true; do
  if docker "${args[@]}"; then
    exit 0
  else
    status=$?
  fi

  if (( attempt > max_retries )); then
    echo "Build and push failed after $attempt attempts" >&2
    exit "$status"
  fi

  echo "Build and push failed (attempt $attempt); retrying in ${retry_delay}s ($((max_retries - attempt + 1)) retries left)" >&2
  sleep "$retry_delay"
  if (( retry_delay < 120 )); then
    retry_delay=$((retry_delay * 2))
    (( retry_delay > 120 )) && retry_delay=120
  fi
  attempt=$((attempt + 1))
done
