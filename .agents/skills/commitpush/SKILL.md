---
name: commitpush
description: Commit staged changes, push to remote, and open a GitHub PR. Use when the user wants to commit, push, and create a pull request in one step. Invoked via /commitpush.
disable-model-invocation: true
---

# commitpush – Commit, Push & PR

## Workflow

### 1. Check staged files

Run `git status`. If there are **no staged files**, stop and ask:
> "No staged files found. Please stage your changes with `git add` and try again."

Do NOT run `git add` automatically.

### 2. Determine target branch

Run `git rev-parse --abbrev-ref HEAD` to get the current branch name.
Infer the PR base branch:

- If branch starts with `hotfix/` → base is `develop`
- If branch starts with `feature/` → base is `develop`
- If branch starts with `release/` → base is `main`
- Otherwise → ask the user which base branch to target

### 3. Review changes and write commit message

Run `git --no-pager diff --cached --stat` and `git --no-pager diff --cached` to understand the staged changes.

Write a conventional commit message (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, etc.) with:

- A concise subject line (≤72 chars)
- A body with bullet points describing what changed and why

Do not add a `Co-Authored-By` trailer or other AI attribution in `/commit` or `/commitpush`. An agent committing directly as part of a ticket follows its harness convention instead.

Show the proposed message and ask: "Ready to commit with this message?"

### 4. Check for existing commits

Run `git --no-pager log origin/{base_branch}..HEAD --oneline` to see all commits on this branch not yet in the base.

Note these — they will be summarized in the PR description.

### 5. Commit

Run `git commit -m "<message>"` with the approved message.

### 6. Push

Run `git push origin {current_branch}`.
If the branch is new upstream, use `git push -u origin {current_branch}`.

### 7. Open PR

Use `gh pr create --base {base_branch} --head {current_branch}` with:

- `--title`: the commit subject line (or a summary if multiple commits)
- `--body`: a detailed markdown PR description covering **all commits** on the branch, structured as:

  - **Summary** — one-paragraph overview of all changes
  - **Changes** — grouped by commit with bullet points
  - **Why** — brief rationale for the changes

If `gh` is not installed or not authenticated, print the GitHub PR creation URL instead.

Do not add a `Co-Authored-By` trailer or other AI attribution to the PR body either.

### 8. Report

Output exactly:

---
PR created: <full_pr_url>
---