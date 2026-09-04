---
name: control-loop
description: Designs a bounded recurring agent workflow around a measurable repository target. Use when turning repeatable maintenance into a local or scheduled sensor-controller-actuator loop.
disable-model-invocation: true
---

# Control loop

Use this skill to turn recurring maintenance into a small, reviewable loop. A
loop has a set point, sensor, controller, actuator, feedback, and optional
dampener. Read the references before designing one.

## Workflow

1. Read the repository router, relevant memory, existing skills, CI files, and
   validation commands before asking setup questions.
2. Define the set point and the exact read/write scope. Prefer an invariant or
   measurable threshold over a vague quality goal.
3. Choose a sensor that can run locally and produces stable output. Use an
   existing linter, test, typecheck, structural search, or a small Rust command
   before inventing an agent-based sensor.
4. Define the controller's next increment. Bound it to one reviewable unit,
   make selection deterministic where practical, and record exclusions.
5. Define the actuator skill, validation commands, response template, and
   feedback file. Keep each rule in one source of truth.
6. Run sensor, controller, and actuator independently by hand before wiring
   CI. The workflow should orchestrate working local pieces, not hide them.
7. If scheduled work is approved, limit each loop to one open PR by label.
   Manual dispatch may bypass that bound. Never add credentials or a cadence
   without an explicit target and repository policy.
8. Record the agreed design and the first result. Put lasting reviewer
   corrections in the feedback file, not in a run log.

## References

- [design](references/design.md) defines the loop vocabulary and design record.
- [feedback](references/feedback.md) is the standing feedback template.
- [workflow](references/workflow-template.yml) is a deliberately thin CI shape.
- [response](references/response-template.md) defines a reviewable result.

## Output

Report the set point, scope, sensor command, controller rule, actuator skill,
validation commands, WIP bound, feedback location, and any unresolved risk.
