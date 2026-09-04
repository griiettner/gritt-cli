#!/bin/sh
# A scripted stand-in for an installed agent CLI. Tests point a connector's
# executable at a wrapper that sets these variables and execs this script:
#   FAKE_AGENT_FIXTURE    file whose lines are printed to stdout
#   FAKE_AGENT_LINE_DELAY seconds to sleep between lines
#   FAKE_AGENT_SLEEP      seconds to sleep after the fixture
#   FAKE_AGENT_EXIT       exit status (default 0)
#   FAKE_AGENT_STDERR     one line printed to stderr first
#   FAKE_AGENT_ARGS_FILE  file that receives the arguments, one per line
#   FAKE_AGENT_AUTH       text printed by the auth probe
case "$1" in
  --version) echo "fake-agent 1.0.0"; exit 0 ;;
  login|auth|status) echo "${FAKE_AGENT_AUTH:-Logged in using fake}"; exit 0 ;;
esac
if [ -n "$FAKE_AGENT_ARGS_FILE" ]; then
  : > "$FAKE_AGENT_ARGS_FILE"
  for arg in "$@"; do printf '%s\n' "$arg" >> "$FAKE_AGENT_ARGS_FILE"; done
fi
if [ -n "$FAKE_AGENT_STDERR" ]; then echo "$FAKE_AGENT_STDERR" >&2; fi
if [ -n "$FAKE_AGENT_FIXTURE" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    printf '%s\n' "$line"
    if [ -n "$FAKE_AGENT_LINE_DELAY" ]; then sleep "$FAKE_AGENT_LINE_DELAY"; fi
  done < "$FAKE_AGENT_FIXTURE"
fi
if [ -n "$FAKE_AGENT_SLEEP" ]; then sleep "$FAKE_AGENT_SLEEP"; fi
exit "${FAKE_AGENT_EXIT:-0}"
