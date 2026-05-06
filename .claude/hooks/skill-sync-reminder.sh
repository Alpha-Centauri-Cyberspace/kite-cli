#!/usr/bin/env bash
# Fires after Edit/Write/MultiEdit. If the touched file is part of the public
# CLI surface, emit a reminder so the agent updates the kite-plugin skill that
# documents the CLI for downstream Claude Code users.
#
# Surface = anything a user can observe via `kite --help` or the published docs.
# Adjust the surface_match regex below as the codebase evolves.
set -euo pipefail

input="$(cat)"

# Pull tool_input.file_path out of the hook payload. Falls back to empty if
# python3 / jq aren't available so the hook never blocks the tool.
file_path=""
if command -v python3 >/dev/null 2>&1; then
  file_path="$(printf '%s' "$input" \
    | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(d.get("tool_input",{}).get("file_path",""))' 2>/dev/null || true)"
fi

surface_match='(/src/commands/|/src/main\.rs$|/docs/COMMANDS\.md$|/README\.md$)'

if [[ -n "$file_path" && "$file_path" =~ $surface_match ]]; then
  cat <<'EOF' >&2
[skill-sync] Heads up: this is a CLI-surface file.

If you've added/changed a command, flag, default, output format, auth flow,
or env var, also update:

  ../kite-plugin/skills/kite/SKILL.md
  ../kite-plugin/.claude-plugin/plugin.json   (bump version)

See CLAUDE.md > "Cross-repo dependency: kite-plugin SKILL.md".
EOF
fi

exit 0
