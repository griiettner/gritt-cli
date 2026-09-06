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
#   FAKE_AGENT_NOISE      when set, print a line every 50 ms forever after the fixture
#   FAKE_AGENT_VERSION_FILE file whose first line is the version `--version` reports
#   FAKE_AGENT_UPDATE_SLEEP seconds a self-update (`update`/`upgrade`) sleeps first
#   FAKE_AGENT_UPDATE_EXIT  exit status of a self-update (default 0)
#   FAKE_AGENT_UPDATE_TO    version a successful self-update writes to FAKE_AGENT_VERSION_FILE
#   FAKE_AGENT_PID_FILE     file that receives this process id during a self-update
case "$1" in
  --version)
    if [ -n "$FAKE_AGENT_VERSION_FILE" ] && [ -f "$FAKE_AGENT_VERSION_FILE" ]; then
      echo "fake-agent $(head -n 1 "$FAKE_AGENT_VERSION_FILE")"
    else
      echo "fake-agent 1.0.0"
    fi
    exit 0
    ;;
  update|upgrade)
    if [ -n "$FAKE_AGENT_PID_FILE" ]; then echo "$$" > "$FAKE_AGENT_PID_FILE"; fi
    echo "checking for updates"
    if [ -n "$FAKE_AGENT_UPDATE_SLEEP" ]; then sleep "$FAKE_AGENT_UPDATE_SLEEP"; fi
    if [ "${FAKE_AGENT_UPDATE_EXIT:-0}" = "0" ] && [ -n "$FAKE_AGENT_UPDATE_TO" ] && [ -n "$FAKE_AGENT_VERSION_FILE" ]; then
      echo "$FAKE_AGENT_UPDATE_TO" > "$FAKE_AGENT_VERSION_FILE"
    fi
    if [ "${FAKE_AGENT_UPDATE_EXIT:-0}" != "0" ]; then echo "update failed: token=sk-fake-secret" >&2; fi
    exit "${FAKE_AGENT_UPDATE_EXIT:-0}"
    ;;
  --list-models)
    if [ -n "$FAKE_AGENT_MODELS_FILE" ]; then cat "$FAKE_AGENT_MODELS_FILE"; else printf '%s\n' "gpt-5.5-medium (default)" "composer-2"; fi
    exit "${FAKE_AGENT_MODELS_EXIT:-0}"
    ;;
  debug)
    if [ "$2" = "models" ]; then
      if [ -n "$FAKE_AGENT_MODELS_FILE" ]; then cat "$FAKE_AGENT_MODELS_FILE"; else printf '%s\n' '{"models":[{"slug":"gpt-5.4","display_name":"GPT-5.4"}]}'; fi
      exit "${FAKE_AGENT_MODELS_EXIT:-0}"
    fi
    ;;
  models)
    if [ -n "$FAKE_AGENT_MODELS_FILE" ]; then cat "$FAKE_AGENT_MODELS_FILE"; else printf '%s\n' "opencode/big-pickle" "openai/gpt-5-nano"; fi
    exit "${FAKE_AGENT_MODELS_EXIT:-0}"
    ;;
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
if [ -n "$FAKE_AGENT_NOISE" ]; then
  while :; do printf '%s\n' '{"type":"noise"}'; sleep 0.05; done
fi
if [ -n "$FAKE_AGENT_SLEEP" ]; then sleep "$FAKE_AGENT_SLEEP"; fi
exit "${FAKE_AGENT_EXIT:-0}"
