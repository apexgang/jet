# Security policy

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include sensitive
details in a pull request.

Use
[GitHub private vulnerability reporting](https://github.com/apexgang/jet/security/advisories/new)
to send the maintainers:

- the affected component and version or commit;
- the conditions needed to reproduce the problem;
- a minimal proof of concept, logs, or screenshots with secrets removed;
- the likely impact and any known workaround.

If private vulnerability reporting is unavailable, open a public issue that asks
for a private contact channel. Do not describe the vulnerability in that issue.

The maintainers will acknowledge a report as soon as practical, investigate it,
and coordinate disclosure and a fix with the reporter. Please allow time for a
patch to reach supported releases before publishing details.

## Supported versions

Jet is under active development. Security fixes target the latest release and the
current `main` branch. Older releases may require upgrading rather than a separate
patch.

## Scope

Reports about Jet's source, release artifacts, bundled Crafts, protocol handling,
credential boundaries, remote access, or update process belong here. Vulnerabilities
in an upstream Harness or Provider should be reported to that project's security
team unless Jet's integration creates or worsens the issue.

For ordinary bugs and hardening suggestions without a confidentiality need, use
the [public issue tracker](https://github.com/apexgang/jet/issues).
