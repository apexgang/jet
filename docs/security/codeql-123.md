# CodeQL remediation for issue 123

Scope: all 79 open CodeQL alerts in `apexgang/jet`, fetched on 2026-09-10, including alerts not linked to [issue 123](https://github.com/apexgang/jet/issues/123). The baseline was GitHub analysis `1748887811` of commit `1b2e8b0c56829c982c97aa2e077c9bdf615eef7e`, using CodeQL 2.26.4 and `codeql/rust-queries` 0.1.41.

## Changes

- Removed account, identity, pairing, Git, and file payloads from failing test diagnostics. Panic messages identify the expected variant or invariant. Assertions still check the same behavior. Git failures retain their exit status.
- Replaced two indexed removals in test helpers with checked iteration. Empty results still fail with a static message.
- Kept the daemon's rejected-connection diagnostic and omitted the peer UID.
- Made pairing entropy generation return bytes initialized by `getrandom::fill_uninit`. The previous zero-filled salt was overwritten by the OS RNG before use, but CodeQL did not recognize that mutation. The new helper makes the initialization explicit for salts, QR tokens, challenges, and rejection-sampled decimal digits. Entropy failures still return `pairing.entropy_unavailable`; no pairing format changed.

These changes apply ASVS 16.2.5 to diagnostics and 11.5.1 to random material. They follow the CodeQL guidance for [cleartext logging](https://codeql.github.com/codeql-query-help/rust/rust-cleartext-logging/) and [cryptographic values](https://codeql.github.com/codeql-query-help/rust/rust-hard-coded-cryptographic-value/). No alert was dismissed and no query or source file was excluded.

## Alert coverage

Locations below are from the baseline report. All alerts except 79 used `rust/cleartext-logging`; alert 79 used `rust/hard-coded-cryptographic-value`.

| Alert | Baseline location | Change |
| --- | --- | --- |
| [#1](https://github.com/apexgang/jet/security/code-scanning/1) | `packages/jet-core/src/setting_tests.rs:78` | Checked iteration with a static failure message |
| [#2](https://github.com/apexgang/jet/security/code-scanning/2) | `packages/jet-core/src/capability_tests.rs:99` | Checked iteration with a static failure message |
| [#3](https://github.com/apexgang/jet/security/code-scanning/3) | `packages/jet-core/src/account_tests.rs:68` | Static expected-variant or invariant diagnostic |
| [#4](https://github.com/apexgang/jet/security/code-scanning/4) | `packages/jet-core/src/account_tests.rs:94` | Static expected-variant or invariant diagnostic |
| [#5](https://github.com/apexgang/jet/security/code-scanning/5) | `packages/jet-core/src/account_tests.rs:126` | Static expected-variant or invariant diagnostic |
| [#6](https://github.com/apexgang/jet/security/code-scanning/6) | `packages/jet-core/src/account_tests.rs:457` | Static expected-variant or invariant diagnostic |
| [#7](https://github.com/apexgang/jet/security/code-scanning/7) | `packages/jet-core/src/audit_tests.rs:42` | Static expected-variant or invariant diagnostic |
| [#8](https://github.com/apexgang/jet/security/code-scanning/8) | `packages/jet-core/src/audit_tests.rs:70` | Static expected-variant or invariant diagnostic |
| [#9](https://github.com/apexgang/jet/security/code-scanning/9) | `packages/jet-core/src/audit_tests.rs:81` | Static expected-variant or invariant diagnostic |
| [#10](https://github.com/apexgang/jet/security/code-scanning/10) | `packages/jet-core/src/audit_tests.rs:88` | Static expected-variant or invariant diagnostic |
| [#11](https://github.com/apexgang/jet/security/code-scanning/11) | `packages/jet-core/src/audit_tests.rs:135` | Static expected-variant or invariant diagnostic |
| [#12](https://github.com/apexgang/jet/security/code-scanning/12) | `packages/jet-core/src/audit_tests.rs:466` | Static expected-variant or invariant diagnostic |
| [#13](https://github.com/apexgang/jet/security/code-scanning/13) | `packages/jet-core/src/capability_tests.rs:80` | Static expected-variant or invariant diagnostic |
| [#14](https://github.com/apexgang/jet/security/code-scanning/14) | `packages/jet-core/src/capability_tests.rs:97` | Static expected-variant or invariant diagnostic |
| [#15](https://github.com/apexgang/jet/security/code-scanning/15) | `packages/jet-core/src/command_tests.rs:34` | Static expected-variant or invariant diagnostic |
| [#16](https://github.com/apexgang/jet/security/code-scanning/16) | `packages/jet-core/src/command_tests.rs:45` | Static expected-variant or invariant diagnostic |
| [#17](https://github.com/apexgang/jet/security/code-scanning/17) | `packages/jet-core/src/command_tests.rs:63` | Static expected-variant or invariant diagnostic |
| [#18](https://github.com/apexgang/jet/security/code-scanning/18) | `packages/jet-core/src/command_tests.rs:77` | Static expected-variant or invariant diagnostic |
| [#19](https://github.com/apexgang/jet/security/code-scanning/19) | `packages/jet-core/src/command_tests.rs:85` | Static expected-variant or invariant diagnostic |
| [#20](https://github.com/apexgang/jet/security/code-scanning/20) | `packages/jet-core/src/craft_installation_tests.rs:50` | Static expected-variant or invariant diagnostic |
| [#21](https://github.com/apexgang/jet/security/code-scanning/21) | `packages/jet-core/src/craft_installation_tests.rs:87` | Static expected-variant or invariant diagnostic |
| [#22](https://github.com/apexgang/jet/security/code-scanning/22) | `packages/jet-core/src/craft_installation_tests.rs:187` | Static expected-variant or invariant diagnostic |
| [#23](https://github.com/apexgang/jet/security/code-scanning/23) | `packages/jet-core/src/craft_installation_tests.rs:204` | Static expected-variant or invariant diagnostic |
| [#24](https://github.com/apexgang/jet/security/code-scanning/24) | `packages/jet-core/src/import_tests.rs:47` | Static expected-variant or invariant diagnostic |
| [#25](https://github.com/apexgang/jet/security/code-scanning/25) | `packages/jet-core/src/import_tests.rs:66` | Static expected-variant or invariant diagnostic |
| [#26](https://github.com/apexgang/jet/security/code-scanning/26) | `packages/jet-core/src/import_tests.rs:87` | Static expected-variant or invariant diagnostic |
| [#27](https://github.com/apexgang/jet/security/code-scanning/27) | `packages/jet-core/src/import_tests.rs:215` | Static expected-variant or invariant diagnostic |
| [#28](https://github.com/apexgang/jet/security/code-scanning/28) | `packages/jet-core/src/paired_client_tests.rs:71` | Static expected-variant or invariant diagnostic |
| [#29](https://github.com/apexgang/jet/security/code-scanning/29) | `packages/jet-core/src/paired_client_tests.rs:84` | Static expected-variant or invariant diagnostic |
| [#30](https://github.com/apexgang/jet/security/code-scanning/30) | `packages/jet-core/src/paired_client_tests.rs:105` | Static expected-variant or invariant diagnostic |
| [#31](https://github.com/apexgang/jet/security/code-scanning/31) | `packages/jet-core/src/paired_client_tests.rs:233` | Static expected-variant or invariant diagnostic |
| [#32](https://github.com/apexgang/jet/security/code-scanning/32) | `packages/jet-core/src/paired_client_tests.rs:249` | Static expected-variant or invariant diagnostic |
| [#33](https://github.com/apexgang/jet/security/code-scanning/33) | `packages/jet-core/src/paired_client_tests.rs:265` | Static expected-variant or invariant diagnostic |
| [#34](https://github.com/apexgang/jet/security/code-scanning/34) | `packages/jet-core/src/pairing_completion_tests.rs:73` | Static expected-variant or invariant diagnostic |
| [#35](https://github.com/apexgang/jet/security/code-scanning/35) | `packages/jet-core/src/pairing_completion_tests.rs:91` | Static expected-variant or invariant diagnostic |
| [#36](https://github.com/apexgang/jet/security/code-scanning/36) | `packages/jet-core/src/pairing_completion_tests.rs:94` | Static expected-variant or invariant diagnostic |
| [#37](https://github.com/apexgang/jet/security/code-scanning/37) | `packages/jet-core/src/pairing_completion_tests.rs:107` | Static expected-variant or invariant diagnostic |
| [#38](https://github.com/apexgang/jet/security/code-scanning/38) | `packages/jet-core/src/pairing_completion_tests.rs:177` | Static expected-variant or invariant diagnostic |
| [#39](https://github.com/apexgang/jet/security/code-scanning/39) | `packages/jet-core/src/pairing_completion_tests.rs:193` | Static expected-variant or invariant diagnostic |
| [#40](https://github.com/apexgang/jet/security/code-scanning/40) | `packages/jet-core/src/pairing_completion_tests.rs:209` | Static expected-variant or invariant diagnostic |
| [#41](https://github.com/apexgang/jet/security/code-scanning/41) | `packages/jet-core/src/pairing_offer_tests.rs:97` | Static expected-variant or invariant diagnostic |
| [#42](https://github.com/apexgang/jet/security/code-scanning/42) | `packages/jet-core/src/pairing_offer_tests.rs:116` | Static expected-variant or invariant diagnostic |
| [#43](https://github.com/apexgang/jet/security/code-scanning/43) | `packages/jet-core/src/pairing_offer_tests.rs:132` | Static expected-variant or invariant diagnostic |
| [#44](https://github.com/apexgang/jet/security/code-scanning/44) | `packages/jet-core/src/pairing_offer_tests.rs:148` | Static expected-variant or invariant diagnostic |
| [#45](https://github.com/apexgang/jet/security/code-scanning/45) | `packages/jet-core/src/pairing_offer_tests.rs:158` | Static expected-variant or invariant diagnostic |
| [#46](https://github.com/apexgang/jet/security/code-scanning/46) | `packages/jet-core/src/pairing_offer_tests.rs:165` | Static expected-variant or invariant diagnostic |
| [#47](https://github.com/apexgang/jet/security/code-scanning/47) | `packages/jet-core/src/pairing_offer_tests.rs:189` | Code shape assertion without the code |
| [#48](https://github.com/apexgang/jet/security/code-scanning/48) | `packages/jet-core/src/pairing_offer_tests.rs:214` | Static expected-variant or invariant diagnostic |
| [#49](https://github.com/apexgang/jet/security/code-scanning/49) | `packages/jet-core/src/pairing_offer_tests.rs:318` | Static expected-variant or invariant diagnostic |
| [#50](https://github.com/apexgang/jet/security/code-scanning/50) | `packages/jet-core/src/pairing_offer_tests.rs:325` | Static expected-variant or invariant diagnostic |
| [#51](https://github.com/apexgang/jet/security/code-scanning/51) | `packages/jet-core/src/pairing_tests.rs:25` | Static expected-variant or invariant diagnostic |
| [#52](https://github.com/apexgang/jet/security/code-scanning/52) | `packages/jet-core/src/pairing_tests.rs:41` | Static expected-variant or invariant diagnostic |
| [#53](https://github.com/apexgang/jet/security/code-scanning/53) | `packages/jet-core/src/pairing_tests.rs:57` | Static expected-variant or invariant diagnostic |
| [#54](https://github.com/apexgang/jet/security/code-scanning/54) | `packages/jet-core/src/pairing_offer_tests.rs:447` | Static expected-variant or invariant diagnostic |
| [#55](https://github.com/apexgang/jet/security/code-scanning/55) | `packages/jet-core/src/project_tests.rs:45` | Static expected-variant or invariant diagnostic |
| [#56](https://github.com/apexgang/jet/security/code-scanning/56) | `packages/jet-core/src/project_tests.rs:58` | Static expected-variant or invariant diagnostic |
| [#57](https://github.com/apexgang/jet/security/code-scanning/57) | `packages/jet-core/src/project_tests.rs:74` | Static expected-variant or invariant diagnostic |
| [#58](https://github.com/apexgang/jet/security/code-scanning/58) | `packages/jet-core/src/project_tests.rs:90` | Static expected-variant or invariant diagnostic |
| [#59](https://github.com/apexgang/jet/security/code-scanning/59) | `packages/jet-core/src/project_tests.rs:392` | Static expected-variant or invariant diagnostic |
| [#60](https://github.com/apexgang/jet/security/code-scanning/60) | `packages/jet-core/src/promotion_command_tests.rs:34` | Static expected-variant or invariant diagnostic |
| [#61](https://github.com/apexgang/jet/security/code-scanning/61) | `packages/jet-core/src/promotion_effect_tests.rs:36` | Static expected-variant or invariant diagnostic |
| [#62](https://github.com/apexgang/jet/security/code-scanning/62) | `packages/jet-core/src/search_tests.rs:30` | Static expected-variant or invariant diagnostic |
| [#63](https://github.com/apexgang/jet/security/code-scanning/63) | `packages/jet-core/src/search_tests.rs:47` | Static expected-variant or invariant diagnostic |
| [#64](https://github.com/apexgang/jet/security/code-scanning/64) | `packages/jet-core/src/security_tests.rs:107` | Static expected-variant or invariant diagnostic |
| [#65](https://github.com/apexgang/jet/security/code-scanning/65) | `packages/jet-core/src/seed_tests.rs:110` | Static expected-variant or invariant diagnostic |
| [#66](https://github.com/apexgang/jet/security/code-scanning/66) | `packages/jet-core/src/setting_tests.rs:28` | Static expected-variant or invariant diagnostic |
| [#67](https://github.com/apexgang/jet/security/code-scanning/67) | `packages/jet-core/src/setting_tests.rs:63` | Static expected-variant or invariant diagnostic |
| [#68](https://github.com/apexgang/jet/security/code-scanning/68) | `packages/jet-core/src/setting_tests.rs:522` | Static expected-variant or invariant diagnostic |
| [#69](https://github.com/apexgang/jet/security/code-scanning/69) | `packages/jet-core/src/test_support.rs:373` | Static expected-variant or invariant diagnostic |
| [#70](https://github.com/apexgang/jet/security/code-scanning/70) | `packages/jet-core/src/test_support.rs:390` | Static expected-variant or invariant diagnostic |
| [#71](https://github.com/apexgang/jet/security/code-scanning/71) | `packages/jet-core/src/test_support.rs:423` | Git failure status without arguments, path, or stderr |
| [#72](https://github.com/apexgang/jet/security/code-scanning/72) | `packages/jet-core/src/test_support.rs:456` | Static expected-variant or invariant diagnostic |
| [#73](https://github.com/apexgang/jet/security/code-scanning/73) | `packages/jet-core/src/test_support.rs:508` | Static expected-variant or invariant diagnostic |
| [#74](https://github.com/apexgang/jet/security/code-scanning/74) | `packages/jet-core/src/test_support.rs:546` | Static expected-variant or invariant diagnostic |
| [#75](https://github.com/apexgang/jet/security/code-scanning/75) | `packages/jet-core/src/workspace_tests.rs:34` | Static expected-variant or invariant diagnostic |
| [#76](https://github.com/apexgang/jet/security/code-scanning/76) | `packages/jet-core/src/workspace_tests.rs:47` | Static expected-variant or invariant diagnostic |
| [#77](https://github.com/apexgang/jet/security/code-scanning/77) | `packages/jet-core/src/checkpoint_tests.rs:68` | Patch assertion without file contents |
| [#78](https://github.com/apexgang/jet/security/code-scanning/78) | `packages/jet-daemon/src/daemon.rs:258` | Rejected-connection message without the UID |
| [#79](https://github.com/apexgang/jet/security/code-scanning/79) | `packages/jet-core/src/pairing_secret.rs:88` | OS-initialized entropy returned by value |

## Validation

- Fresh local CodeQL scans used 2.26.4 and `codeql/rust-queries` 0.1.41 on complete source snapshots. The baseline reproduced all 79 GitHub findings at the same file and line locations, with no extra findings. The fixed snapshot produced **zero findings**.
- Both scans reported the same 161 files with existing macro/extraction diagnostics. The fixed snapshot added one successfully extracted test file, from 295 to 296. All original alert locations were reproduced before remediation.
- `just fix -p jet-core -p jet-daemon` passed before editing. `just clippy -p jet-core -p jet-daemon` passed after editing. `just fmt` passed after testing.
- `just test -p jet-core -p jet-daemon`: 348 passed, 29 skipped. This includes new tests for salt freshness, salt-dependent digests, secret normalization, and mismatched salts/secrets.
- `just test-doc -p jet-core -p jet-daemon` passed; these crates had no runnable doctests.

The full suite initially encountered sandbox IPC restrictions and was rerun with local socket/subprocess permissions. That run passed 522 of 525 tests, with 32 skipped. Three process tests failed during concurrent CodeQL analysis. All three passed when rerun after scanning finished:

- `stopping_a_run_escalates_to_kill_and_keeps_what_it_produced`
- `bundled_reviewers_send_one_tool_free_request_carrying_only_the_transcript_and_action`
- `definite_launch_failures_finish_and_release_admission`

Their assertions and timeouts were not changed. The 525 selected tests passed across the full run and isolated rerun.

The reproducible scanner command, from `packages/`, is:

```sh
just codeql-scan /tmp/jet-codeql-db /tmp/jet-codeql.sarif /path/to/codeql codeql/rust-queries@0.1.41
```

Use a fresh database path for each scan. The recipe checks the SARIF `runs[].results` arrays and fails if any findings remain, since CodeQL itself returns success when analysis finishes even if it finds problems.
