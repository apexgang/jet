# Git delivery

Issue #44 implements ADR-0029, ADR-0031, ADR-0067 and ADR-0099 through
protocol minor 35. The backend exposes the controls and generated GUI models.

## Policy and manual controls

Projects supply independent `git.auto_branch`, `git.auto_commit`,
`git.auto_push` and `git.auto_draft_pull_request` defaults. Each defaults to
false. A Conversation can override any one value. `git.branch_prefix` defaults
to `jet/` and uses the same scopes. Plane-wide `git.message_instructions` and
Utility routing remain separate settings.

`DeliverGit` accepts a Conversation, an optional retained checkpoint, and one
operation: `branch`, `commit`, `push`, or `draft_pull_request`. Branch names,
remote names and the draft's optional base are explicit editable inputs. Manual
branch creation and pushing work before the first checkpoint. Commit and PR
text require a checkpoint owned by the selected Conversation.

Successful turn checkpoints admit enabled steps in branch, commit, push, draft
order. The first changed turn creates a branch using the configured prefix plus
its Conversation UUID. A successfully created manual or automatic branch is
retained for later turns. An unchanged turn does not create a branch.
Enabling push or draft creation does not enable commits or branch creation.
A draft requires its branch to have already been pushed. Automatic delivery
uses `origin`; manual Commands may select another configured remote.

## Recorded boundaries

Each step has its own durable Effect identity, repository observations, policy,
generated message and outcome. `GitDeliveries` returns the newest 100 operations
for a Conversation. A Utility job link exposes Provider, Account binding, Model,
purpose and inference policy. Its bounded checkpoint input and deterministic
fallback follow the existing Utility implementation. If that queue is full,
delivery retains deterministic text with the `utility.queue_full` explanation.

Before execution, Jet checks the registered root, HEAD, index, branch, remote,
checkpoint content, active work, current policy and required capabilities.
Push preflight exercises the current Git transport and credentials. Commits
resolve the configured Git author and committer rather than inventing identities.

Commit preparation creates an immutable object from the captured tree. Publication
compares the old ref and locks the index; working files are not rewritten.
The replacement index preserves sparse-checkout flags. Pushes honor pre-push
hooks, including Git LFS uploads. Shared Local checkouts have one delivery
barrier across every Conversation using that Project.
Automatic commits refuse dirty starting baselines and omitted checkpoint content.
Later edits, a conflicting Git operation, or a policy change stop the affected
Effect. Failed or interrupted turns do not admit automatic delivery. Delivery
waits for the Harness parser checkpoint even after a Run ends; later steps wait
for pending predecessors.

A failed later step never rolls back a completed earlier step. Interrupted
mutations are reconciled from observable Git or GitHub state. Jet never repeats
an uncertain mutation. An unresolved outcome blocks further delivery and managed
turns until the user inspects it and submits `AcknowledgeGitDelivery` for that
exact identity. A crash during local publication may also require repairing a
leftover Git index lock after inspecting the ref and index. Acknowledgement records the user and leaves the original outcome
unknown; it does not execute or retry Git.

## GitHub and credentials

Hosted drafts support `github.com` HTTPS and SSH remotes. Local Git operations
remain available for other providers. The Conversation keeps a durable draft
binding across Runs and restarts. Jet refuses to update a draft that the user
closed or marked ready for review, or one whose head or base no longer matches.
Stable Conversation and Effect markers allow reconciliation after a lost reply.
The API operations follow GitHub's [pull request REST contract](https://docs.github.com/en/rest/pulls/pulls).

The production GitHub transport reads one platform-store item with service
`me.heeka.jet.github` and account `github.com`: a macOS Keychain generic password,
or a Linux Secret Service item with those attributes. The value is a GitHub token
with repository pull-request write permission. The transport resolves it for
every request. It never reads environment tokens or `gh` plaintext credentials,
stores tokens in SQLite, follows redirects, or retries mutations. The trusted
`GitHubHost` interface lets platform integrations supply the same contract.

## Review and verification

The coherent implementation crosses the store, core, protocol, daemon and client
contracts, exceeding the repository's usual 800-line guidance. Review it in three
dependent parts: policy and wire contracts; durable admission and Git publication;
GitHub transport and recovery. Separating the policy fix is possible, but the
remaining parts must land together to expose usable delivery Commands.

Tests use public core Commands and Queries, the real daemon protocol, disposable
Git repositories, and a controlled GitHub peer. They cover scope inheritance,
manual operations, exact-content commits, pushes, policy revocation, later edits,
draft reuse, lost acknowledgements, sparse index preservation, shared checkout
isolation and delayed parser checkpoints.

Securability notes: parameterized SQL and literal Git arguments apply ASVS 1.2.4
and 1.2.5. Closed operations, bounded text and response reads apply ASVS 2.2.1.
Durable attempts, compare-and-swap refs and explicit uncertainty apply ASVS 2.3.3
and 2.3.4. Platform-only GitHub credentials apply ASVS 13.3.1. These boundaries
keep integrity, observability and recovery explicit without adding dependencies.
