---
name: gh-merge
description: Automatically commit, push and create PR to default branch once work is done.
disable-model-invocation: true
---

# GitHub Merge

When work is complete, follow through the remote update procedure.

## Before you start

Check if the repository uses [release-please](https://github.com/googleapis/release-please) for releases automations.

If repository uses release-please, adjust the title of PR so release-please correctly parses it.

Before making a PR, estimate changes made. Ask the user what type of version bump he wants. Suggest your recommendation based on chages.

Before user gives you an explicit confrimation, you are not allowed to proceed further.

## Procedure

1. Check git status and diff.
2. Run required tests/checks.
3. Create an appropriate commit message.
4. Commit only relevant files.
5. Push the current branch.
6. Create/update a GitHub PR using `gh pr create`.
7. Enable auto-merge:
   `gh pr merge --auto --squash`
8. Report PR URL and CI status.

## Stacked PRs

If merging to main requires merging multiple PRs, use GitHub Stacked Pull Requests: `gh stack`.

If a PR stack is available or created, use `gh stack merge` to merge it.

## PR Body additions

In addition to pr body always add `closes #<issue-number>` at the end as the new paragraph of the body, if the PR is related to an issue. For example, `closes #99`.

## Notes

- Usage of `gh` cli might be blocked due to sandboxing. Always ask for approval to run `gh` commands.
- If there is nothing to commit, check the remote branch. If commit is not present in origin, push the branch and proceed with the procedure.
- If branch is on the remote and already up to date, explicitly tell the user about it and ask if he wants to just do the PR to default branch.
