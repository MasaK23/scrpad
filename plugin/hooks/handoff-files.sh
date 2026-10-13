#!/bin/sh
# Claude Code PreToolUse hook: stops a tool call that names a file scrpad hands to a project
# (handoff-files.jq), so the agent does not read the secret in it. Exit 2 blocks the call and
# tells the agent why. Best effort: quoting tricks, globs (cat .e*) or other names slip past.
if ! command -v jq >/dev/null; then
  echo "scrpad hook: jq is not installed, so .env files are not guarded" >&2
  exit 1
fi
if jq -e -f "$(dirname "$0")/handoff-files.jq" >/dev/null; then
  echo "scrpad handoff file: not for the agent. Use scrpad --run or --write; never read the file." >&2
  exit 2
fi
exit 0
