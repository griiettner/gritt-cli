---
name: diagnose
description: Builds a tight red-capable debugging loop before fixing a failure. Use when diagnosing a bug, regression, crash, incorrect result, or performance problem.
disable-model-invocation: true
---

# Diagnose

Do not begin with a theory. Build a safe signal that goes red on the user's
actual symptom, and redact secrets from commands, output, and artifacts.

## Workflow

1. Read the relevant architecture, ADRs, ticket, and validation paths.
2. Build and run one red-capable reproduction at the correct public seam.
3. Minimize the reproduction while rerunning it after each removal.
4. State 3 to 5 ranked, falsifiable hypotheses before instrumentation.
5. Probe one hypothesis at a time with tagged temporary diagnostics. Prefer a
   debugger or targeted logs over logging everything.
6. Write the regression test before the fix when a correct seam exists.
7. Fix the cause, rerun the minimized and original reproductions, then run the
   normal verification set.
8. Remove tagged diagnostics and throwaway artifacts. Record an architecture
   follow-up if no correct regression-test seam exists.

## Completion criteria

- The original symptom is reproduced and then no longer reproduces.
- A regression test or explicit missing-seam finding exists.
- Secrets are redacted and temporary instrumentation is gone.

## Output

Return the reproduction command, minimized case, hypotheses, evidence, root
cause, fix, regression coverage, and validation results.
