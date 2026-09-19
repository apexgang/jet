# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

## Users

Jet is for people who use Codex or Claude Code to complete technical work on one or more computers. This includes developers working in codebases and non-developers asking an agent to configure a device or coordinate a task across devices. It must work out of the box for casual users while retaining the control that experienced users need.

## Product Purpose

Jet gives people one desktop application for starting, supervising, and returning to agent work across computers. It manages durable Conversations, Runs, Projects, isolated Workspaces, terminals, approvals, schedules, and retained history through the same model on every client.

Success means a user can begin useful work without coding knowledge or an understanding of Jet's architecture, choose local or remote execution when needed, and trust that active work and history survive ordinary restarts and upgrades.

## Positioning

Jet provides one consistent workflow for Codex and Claude Code while preserving each Harness's native behavior. A Conversation can run its Harness locally or natively on a paired remote Plane. It can also keep the Harness on its origin Plane while using controlled remote tools. This covers local, remote, and mixed execution without forcing every Harness into a lowest-common-denominator integration.

## Operating Context

Users work from native and cross-platform desktop clients connected to one or more Planes running `jetd`. They select a Project and working tree, start or resume a Conversation, follow Harness output, answer approvals, use Workspace terminals, inspect changes, and decide how work reaches a permanent checkout or branch.

Some Conversations stay active across long Runs, queued turns, schedules, daemon restarts, or Plane transfers. Users may move between computers while one Home Plane remains authoritative for each Conversation.

## Capabilities and Constraints

- Jet supports Codex and Claude Code through versioned Jet Crafts.
- Conversations can run on the local Plane, on a paired remote Plane in Visa mode, or through controlled remote tools in No-Visa mode.
- Each Conversation has one authoritative Home Plane and at most one active Run.
- Managed Workspaces isolate concurrent Conversations that use the same Project. The user's Local checkout remains outside the managed Workspace lifecycle.
- `jetfueld` keeps an active Harness Run or Workspace terminal alive while `jetd` restarts or upgrades.
- Every GUI uses the versioned Jet protocol. The Apple client is native SwiftUI; the cross-platform desktop client uses Tauri and Svelte.
- The v1 core targets macOS and Linux. The native Apple and Tauri clients remain under active development, and the v1 release contract has open gates.
- Jet uses the domain terms in `CONTEXT.md`. Future product and interface work must not replace them with conflicting synonyms.
- Jet is licensed under Apache License 2.0.

## Brand Commitments

The product name is Jet. Product language should be direct and comprehensible to people without software development experience. Technical domain terms remain available where precision matters, but the interface must explain them through context instead of assuming prior knowledge.

## Evidence on Hand

- `README.md` records the product purpose, supported Harnesses, architecture, development status, and repository layout.
- `CONTEXT.md` defines the product's domain language and the distinctions future interface work must preserve.
- `docs/` contains detailed behavior, security, recovery, remote execution, scheduling, Git delivery, and release contracts.
- `apps/jet/` contains the native SwiftUI Apple client.
- `apps/jet-tauri/` contains the Tauri and Svelte cross-platform client. Its visible interface is still the starter template, so it is not evidence of an established Jet visual identity.
- The repository contains no approved testimonials, customer logos, pricing, usage benchmarks, or press claims. Future work must not fabricate them.

## Product Principles

- Make the first useful action clear without requiring users to understand Planes, Crafts, or execution modes in advance.
- Keep local and remote work in one consistent Conversation model.
- Preserve Harness-native capability and behavior where Jet claims parity.
- Make authority, location, approvals, and destructive actions explicit before they can surprise the user.
- Protect active work, history, and Workspace changes through failures, restarts, and transfers.

## Accessibility & Inclusion

Jet must serve non-developers, casual users, and experienced developers in the same product. Core tasks need clear defaults and plain explanations, while advanced controls can appear when the user needs them. Each client should follow the accessibility conventions and interaction patterns of its operating system.
