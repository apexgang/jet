# Limit hosted pull requests to GitHub in v1

V1 keeps local Git operations provider-neutral but creates and updates hosted pull requests only for GitHub remotes. Other remotes retain branch, commit, push, diff, and web-opening capabilities without pretending to support their review APIs. GitHub draft pull requests use credentials from the platform credential store, and one Conversation updates its existing draft rather than creating a new pull request after every Run.
