# Automatic review

Issue #39 implements the first half of ADR-0012: one Plane-wide Automatic
review mode that answers Harness approval requests which would otherwise wait
for a person. Turning it on changes who answers, and nothing else. The
Harness keeps the sandbox, filesystem, network, tool, and permission boundary
it already had, the reviewer never reaches the execution it judges, and the
only thing that crosses back is one decision for one request. There is no
blanket approval to grant: the wire carries `allow_once` and `deny`, and
nothing else.

The trusted-core deny rules, denial budgets, and the one-retry user override
described by ADR-0012 are issue #40 and are not implemented here. Until then,
a review that cannot happen makes no decision, and the request stays held for
a person.

## What one review is

A Craft that negotiated Craft 1.8 reports the request it is holding before it
reports that the Harness is waiting:

```json
{"kind":"approval_requested","request":{"request_id":"perm-1","tool":"Write","action":"{\"file_path\":\"note.txt\"}"}}
```

The host records that as an `approval.requested` Event, reviews it, records
an `approval.reviewed` Event and a Security-audit decision, and only then
answers through the existing action Command. The request commits on its own
source boundary first, so the review always describes something durable, and
the decision is durable before it can authorize anything.

The review runs beside the supervising loop rather than inside it, so a slow
reviewer never stops the Craft from being read while the Harness waits.

## Policy

Configure these through the existing authenticated Settings Commands (Jet
protocol 1.30). All three are Plane-wide; ADR-0012 makes Automatic review one
mode for a whole Plane, so no narrower scope can enable it for part of one.

| Setting | Value | Default |
| --- | --- | --- |
| `review.automatic` | Plane flag | Disabled |
| `review.account_binding` | Account binding UUID, or empty text for each Run's own | Empty |
| `review.cross_provider_consent` | The exact binding UUID authorized to receive Conversation content from another Provider | Empty |

The default reviewer is the Run's own: the Provider and Account binding its
native execution already authenticates through, which both Visa and No-Visa
Runs record. Naming a binding for another Provider requires persistent
consent recording that exact binding; without it the review is unavailable
and the request waits for a person. Setting or clearing any of these produces
a Security-audit decision.

A Plane that never turned `review.automatic` on does nothing at all: it
observes no Plane capabilities, selects no reviewer, and records nothing
about a request it was never going to answer.

Once it is on, review is still unavailable when the Run carries no Account
binding of its own and the Plane names none, when the selected binding's
Credential cannot be resolved, and while the Plane is in Security-degraded
mode — a Plane that cannot vouch for its own audit does not decide on a
person's behalf (ADR-0105). Each of those is recorded as a review that could
not happen.

## What a reviewer is shown, and what it may answer

| Field | Bound, in UTF-8 bytes |
| --- | --- |
| `transcript` | ≤12288, the newest visible `turn.input` and `run.output` content of the Conversation, oldest first, one line per entry |
| `tool` | ≤128, the native tool or permission name |
| `action` | ≤4096, the exact requested action as the Harness asked for it |

Only the portable Presentation views a GUI renders count as visible: the
lossless native event beside them is Harness output — tool results, file
content, terminal bytes — and no reviewer is shown any of it, so an output
with no view of its own contributes nothing. Nothing else reaches the
reviewer either: no credentials, Settings, working-tree paths, or tool
definitions. Entries are flattened to one line each so no content can forge
the structure of the transcript around it, an entry too long for what is left
of the budget is cut rather than dropped, and the reviewer is told that
everything it is shown is untrusted data.

The answer is data, validated again by the core:

```json
{"risk":"low|medium|high|critical","authorization":"absent|partial|sufficient","decision":"allow|deny","rationale":"…"}
```

Unknown fields, unknown vocabulary, malformed JSON, an empty or oversized
rationale, an answer over 4096 bytes, and a reported Model that differs from
the selection are all refused. The core never decodes an answer as a Command.

The decision the core makes is not the reviewer's:

| Reviewer answer | Decision |
| --- | --- |
| `deny` | Deny |
| `allow`, risk `low` or `medium` | Allow once |
| `allow`, risk `high`, authorization `sufficient` | Allow once |
| `allow`, risk `high`, authorization `absent` or `partial` | Deny |
| `allow`, risk `critical` | Deny |

## Bundled Crafts

An accepted `claude-code` installation reviews for `anthropic`; an accepted
`codex` installation reviews for `openai`. The reviewer for a Run is the Run's
own Craft whenever that Craft speaks for the selected binding's Provider, so a
Harness carrying its own separate reviewer is the one that answers. Its
accepted declaration must disclose the optional `review` feature, the `curl`
executable, and the Provider endpoint.

Neither bundled Harness exposes a separate native reviewer that satisfies the
Jet contract today, so both answer through Jet's versioned equivalent and
report `equivalent`. The contract keeps `native` for a Craft whose Harness
does, and the reviewer that answered is recorded on every review.

Review uses an isolated, versioned stdin/stdout exchange, the same shape as
Utility work:

- `--review-model` returns `CraftReviewModel` without receiving user content.
- `--review` accepts one `CraftReviewRequest` (≤128 KiB including escaping)
  and returns one `CraftReviewReply` (≤64 KiB including escaping).

These modes never launch a Harness, connect to a native session, or receive a
broker, workspace root, or tool definitions, and they reuse the Utility
transport: one fixed HTTPS request through curl with no redirects, retries,
proxy, or curlrc, credentials on stdin only, and the same per-reference
credential resolution described in [Utility work](utility-work.md).

Each review has a 30-second deadline covering selection and the request
together. An attempt that times out, fails, or is interrupted makes no
decision and is never repeated on its own.

## What is recorded

`approval.requested` and `approval.reviewed` Events carry the exact request,
the reviewer, Provider, binding, Model, resolved policy, the reviewer's
verdict, and the decision the core made. They are Conversation history and
are not indexed for Search.

The Security audit separately records one `approval.reviewed` decision per
review against the execution, at elevated risk, with the outcome the review
came to: succeeded for an allowance, denied for a denial, and failed for a
review the Plane opted into but could not carry out. The audit holds
attribution and outcome only — the requested action is Conversation content
and stays in the journal (ADR-0105).
