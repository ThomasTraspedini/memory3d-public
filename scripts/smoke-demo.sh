#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: scripts/smoke-demo.sh [all|rust|python|mcp]\n' >&2
}

mode="${1:-all}"
case "$mode" in
  all|rust|python|mcp) ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage
    exit 2
    ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="${MEMORY3D_SMOKE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/memory3d-smoke.XXXXXX")}"

cleanup() {
  if [[ -z "${MEMORY3D_SMOKE_DIR:-}" && -z "${MEMORY3D_SMOKE_KEEP:-}" ]]; then
    rm -rf "$work"
  else
    printf 'smoke workspace: %s\n' "$work"
  fi
}
trap cleanup EXIT

mkdir -p "$work"
cd "$root"

require_file() {
  if [[ ! -s "$1" ]]; then
    printf 'expected non-empty file: %s\n' "$1" >&2
    exit 1
  fi
}

validate_cli_demo() {
  python3 - "$work/cli-ingest.json" "$work/cli-recall.json" <<'PY'
import json
import math
import pathlib
import sys

ingest = json.loads(pathlib.Path(sys.argv[1]).read_text())
recall = json.loads(pathlib.Path(sys.argv[2]).read_text())

assert ingest["node_count"] >= 25, ingest
assert ingest["relation_count"] >= 20, ingest
assert recall["stats"]["seeds"] <= recall["options"]["seed_limit"], recall
assert recall["stats"]["visited_nodes"] <= recall["options"]["max_visited_nodes"], recall
assert recall["stats"]["visited_edges"] <= recall["options"]["max_visited_edges"], recall

results = recall["results"]
assert results, recall
assert any("Mobile session loss incident" in item["node"]["text"] for item in results), results
assert any(len(item["path"]["steps"]) >= 2 for item in results), results

for item in results:
    assert math.isfinite(item["score"]), item
    assert item["path"]["seed"] > 0, item
    assert len(item["path"]["steps"]) == item["hops"], item
    for step in item["path"]["steps"]:
        assert step["source"] > 0 and step["target"] > 0, step
        assert step["relation"], step
        assert math.isfinite(step["weight"]), step
PY
}

validate_python_demo() {
  "$work/venv/bin/python" - "$work/python-ingest.json" "$work/python-recall.json" <<'PY'
import json
import math
import pathlib
import sys

ingest = json.loads(pathlib.Path(sys.argv[1]).read_text())
recall = json.loads(pathlib.Path(sys.argv[2]).read_text())

assert ingest["node_count"] >= 25, ingest
assert ingest["relation_count"] >= 20, ingest
assert recall["stats"]["visited_nodes"] <= 64, recall
assert recall["stats"]["visited_edges"] <= 128, recall
assert any("Mobile session loss incident" in item["text"] for item in recall["results"]), recall
assert any(len(item["path"]["steps"]) >= 2 for item in recall["results"]), recall

for item in recall["results"]:
    assert math.isfinite(item["score"]), item
    assert item["path"]["seed"] > 0, item
PY
}

run_rust_demo() {
  local database="$work/codebase.memory3d"
  cargo run -q -p memory3d-cli -- --db "$database" --json demo ingest > "$work/cli-ingest.json"
  require_file "$database"
  cargo run -q -p memory3d-cli -- --db "$database" --json demo recall > "$work/cli-recall.json"
  validate_cli_demo
  cargo run -q -p memory3d-cli -- --db "$database" check > "$work/cli-check.txt"
}

run_python_demo() {
  local database="$work/python-codebase.memory3d"
  python3 -m venv "$work/venv"
  "$work/venv/bin/python" -m pip install --upgrade pip
  "$work/venv/bin/python" -m pip install 'maturin>=1.7,<2' pytest
  VIRTUAL_ENV="$work/venv" PATH="$work/venv/bin:$PATH" "$work/venv/bin/python" \
    -m maturin develop --manifest-path crates/memory3d-python/Cargo.toml
  "$work/venv/bin/python" -m pytest crates/memory3d-python/tests
  PYTHONPATH="$root/examples/python-demo" "$work/venv/bin/python" \
    examples/python-demo/ingest.py "$database" > "$work/python-ingest.json"
  require_file "$database"
  PYTHONPATH="$root/examples/python-demo" "$work/venv/bin/python" \
    examples/python-demo/recall.py "$database" > "$work/python-recall.json"
  validate_python_demo
}

run_mcp_verification() {
  cargo test -q -p memory3d-mcp --test protocol
}

case "$mode" in
  all)
    run_rust_demo
    run_python_demo
    run_mcp_verification
    ;;
  rust)
    run_rust_demo
    ;;
  python)
    run_python_demo
    ;;
  mcp)
    run_mcp_verification
    ;;
esac

printf 'Memory3D smoke %s passed in %s\n' "$mode" "$work"
