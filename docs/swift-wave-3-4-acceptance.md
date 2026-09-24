# Swift Wave 3.4 acceptance

Status: Swift implementation and code review on 2026-09-24. This record covers the
macOS app in `apps/jet/` and its iOS compile check. The Linux client was not
changed or assessed in this slice, so the cross-client exit criterion in
`docs/desktop-implementation-plan.md` remains open.

The table maps user-visible behavior to the relevant sections of
`docs/design-language.md`. "Code covered" means the Swift implementation and
its existing tests cover the behavior. It does not claim a visual or accessibility
pass on a running window.

| User-visible capability | Design sections | Swift evidence | Status |
| --- | --- | --- | --- |
| Restore one task workspace; resize it without squeezing the conversation behind the work panel | App and window model; Main workspace; Platform adaptation | `jetApp.swift`, `DesktopShellView.swift`: the main window is set to present at launch, keeps a 900 by 600 minimum, closes the inspector when resized below 1100 points, and offers work details in a focused sheet | Code covered; running-window check pending |
| Read task identity, Project, Plane, status, and compose at narrow widths | Conversation; States and scale | `DesktopShellView.swift`: headers and composer context use `ViewThatFits`; task status remains labeled | Code covered; running-window check pending |
| Inspect changes, files, terminals, queue, and Run details without losing task selection | Work panel; Changes and checkpoints; Files, terminals, and recovery | `DesktopSession.swift`, `DesktopShellView.swift`, `DesktopSessionTests.swift` | Code covered; compact sheet interaction pending |
| Review approvals and distinguish Interrupt Turn from Stop Run | Active run; Security and privacy behavior | `DesktopShellView.swift`, `JetCommands.swift`, `DesktopSessionTests.swift`; approval facts stack at narrow widths | Code covered; generic Harness approval decisions still depend on the protocol command |
| Supervise task continuity, Search, and multi-Plane provenance | Sidebar; Selection, continuity, and activity | `DesktopSession.swift`, `DesktopSessionTests.swift`, `Wave31PlaneTests.swift` | Code covered; shared pins and an atomic first create-and-start operation remain backend gaps |
| Review delivery destination and durable outcome | Completion and delivery | `DesktopShellView.swift`, `JetClient.swift`, `DesktopSessionTests.swift` | Code covered |
| Connect and manage Planes and Projects | First launch; Setup and Projects | `PlaneManagementView.swift`, `ProjectSetupView.swift`, `Wave31PlaneTests.swift` | Code covered; Plane transfer GUI remains a separate dependency |
| Open five Settings groups and recover from errors | Settings; Return and recovery | `JetSettingsView.swift`, `JetRecoveryView.swift`, `Wave32SettingsTests.swift`, `Wave33RecoveryTests.swift` | Code covered |
| Preserve legibility in dark, increased-contrast, reduced-transparency, and reduced-motion modes | Visual language; Motion and feedback | `DesktopShellView.swift` uses system colors and opaque bar backgrounds when requested, gives status labels a high-contrast border, and adds no custom motion; `ContentView.swift` contains compact and dark previews | Code covered; visual and assistive-technology checks pending |
| Reach work-panel actions through Mac menus and keep iOS source buildable | Menus, commands, and input; Platform adaptation | `JetCommands.swift`; macOS and iOS Simulator builds | Code covered |

## Configuration checks

| Configuration | Current evidence | Remaining check |
| --- | --- | --- |
| Default 1280 by 800 | macOS build and Swift unit suite passed | Running-window visual and keyboard check |
| Narrow 900 by 600 | Compact preview and width-based sheet path compile | Open, close, and revisit each work tab in a running window |
| Wide 1600 by 900 | Wide dark preview compiles | Running-window layout check |
| Full screen and multiple displays | Native resizable `Window` scene | Move and resize a running window on each display |
| Dark and increased contrast | Dark preview compiles; status labels respond to increased contrast | Visual contrast and VoiceOver check |
| Reduced motion and transparency | Bar backgrounds become opaque when transparency is reduced; no custom motion was added | Verify system settings in a running window |
| iOS Simulator | Generic Simulator build passed | Companion product work is deferred by the desktop plan |

The macOS UI test runner launched the app process but exposed no app window in
this environment. A signed retry gave the same result. The running-window
checks above therefore remain unverified; the unit suite and both platform
builds are the completed automated gates.

## Native guidance used

- [Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos/): resizable, restorable windows and menu access.
- [Layout](https://developer.apple.com/design/human-interface-guidelines/layout): hide tertiary content before the main task becomes cramped.
- [ViewThatFits](https://developer.apple.com/documentation/swiftui/viewthatfits): adapt labeled content to available width.
- [Reduce Transparency](https://developer.apple.com/documentation/swiftui/environmentvalues/accessibilityreducetransparency): use an opaque background when the setting is enabled.
- [Color](https://developer.apple.com/design/human-interface-guidelines/color): use semantic colors and labels that communicate state without color alone.

These Apple pages were checked on 2026-09-24. The changes use APIs available
at the repository's existing macOS and iOS deployment targets.
