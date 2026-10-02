#!/bin/sh
# Builds the browser app into crates/web/dist for Vercel (the project's
# outputDirectory). The NEXT_PUBLIC_* names are the same ones the Next app
# read, and they are baked into the wasm at compile time.
set -eu

cd "$(dirname "$0")/.."

require() {
  eval "value=\${$1:-}"
  if [ -z "$value" ]; then
    echo "build-web: $1 is required for a release build" >&2
    exit 1
  fi
}

require NEXT_PUBLIC_PRIVY_APP_ID
require NEXT_PUBLIC_WORLD_APP_ID

# crates/web/src/config.rs asserts the same rule at compile time; failing here
# names the cause before a long compile starts. VERCEL_ENV is set by Vercel
# and must stay in the environment trunk and rustc inherit, or the compile-time
# check cannot see it.
world_environment="${NEXT_PUBLIC_WORLD_ENVIRONMENT:-production}"
if [ "${VERCEL_ENV:-}" = "production" ] && [ "$world_environment" != "production" ]; then
  echo "build-web: refusing a Vercel production build with NEXT_PUBLIC_WORLD_ENVIRONMENT=$world_environment" >&2
  exit 1
fi
export VERCEL_ENV="${VERCEL_ENV:-}"

for tool in npm cargo trunk; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "build-web: $tool is not installed or not on PATH" >&2
    exit 1
  fi
done

npm ci --prefix crates/web/js
(cd crates/web && trunk build --release)
