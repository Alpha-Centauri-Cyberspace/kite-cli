#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

OUT_COMMANDS="$ROOT_DIR/docs/COMMANDS.md"
OUT_WEB_REF="${KITE_WEB_CLI_REFERENCE:-}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
TARGET_TRIPLE="${CARGO_BUILD_TARGET:-}"
if [ -n "$TARGET_TRIPLE" ]; then
  KITE_BIN="$TARGET_DIR/$TARGET_TRIPLE/debug/kite"
else
  KITE_BIN="$TARGET_DIR/debug/kite"
fi

mkdir -p "$(dirname "$OUT_COMMANDS")"
if [ -n "$OUT_WEB_REF" ]; then
  mkdir -p "$(dirname "$OUT_WEB_REF")"
fi

cargo build -p kite-cli >/dev/null

if [ ! -x "$KITE_BIN" ]; then
  echo "kite binary not found at $KITE_BIN" >&2
  exit 1
fi

tmp_file="$(mktemp)"
trap 'rm -f "$tmp_file"' EXIT

emit_help() {
  local title="$1"
  shift
  local output
  output="$("$KITE_BIN" "$@")"

  {
    printf '## `%s`\n\n' "$title"
    printf '```text\n%s\n```\n\n' "$output"
  } >>"$tmp_file"
}

cat <<'EOF' >"$tmp_file"
# Kite CLI Command Reference

This file is auto-generated from the live `clap` help output.
Do not edit manually. Run:

```bash
./scripts/generate-cli-docs.sh
```

## Payload Output Modes

- `kite stream` (default) prints concise summary lines.
- `kite stream --compact` prints summary-only one-liners.
- `kite stream --json` prints full CloudEvent payload JSON.

EOF

emit_help "kite" --help
emit_help "kite stream" stream --help
emit_help "kite proxy" proxy --help
emit_help "kite listen" listen --help
emit_help "kite run" run --help
emit_help "kite retry" retry --help
emit_help "kite login" login --help
emit_help "kite github" github --help
emit_help "kite github install" github install --help
emit_help "kite endpoints" endpoints --help
emit_help "kite endpoints list" endpoints list --help
emit_help "kite endpoints create" endpoints create --help
emit_help "kite endpoints deactivate" endpoints deactivate --help
emit_help "kite keys" keys --help
emit_help "kite keys list" keys list --help
emit_help "kite keys create" keys create --help
emit_help "kite keys revoke" keys revoke --help
emit_help "kite skill" skill --help
emit_help "kite skill install" skill install --help
emit_help "kite skill list" skill list --help
emit_help "kite skill publish" skill publish --help
emit_help "kite skill search" skill search --help
emit_help "kite skill registry-install" skill registry-install --help
emit_help "kite skill export" skill export --help
emit_help "kite queue" queue --help
emit_help "kite queue list" queue list --help
emit_help "kite queue show" queue show --help
emit_help "kite queue replay" queue replay --help
emit_help "kite queue flush" queue flush --help
emit_help "kite queue stats" queue stats --help
emit_help "kite agent" agent --help
emit_help "kite agent register" agent register --help
emit_help "kite agent send" agent send --help
emit_help "kite agent listen" agent listen --help
emit_help "kite logs" logs --help
emit_help "kite status" status --help
emit_help "kite update" update --help

cp "$tmp_file" "$OUT_COMMANDS"
if [ -n "$OUT_WEB_REF" ]; then
  cp "$tmp_file" "$OUT_WEB_REF"
fi

echo "Generated:"
echo "  $OUT_COMMANDS"
if [ -n "$OUT_WEB_REF" ]; then
  echo "  $OUT_WEB_REF"
fi
