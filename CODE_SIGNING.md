# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io),
certificate by [SignPath Foundation](https://signpath.org).

## Team roles

puppetty is developed and maintained by its author:

- **Authors** (may modify source code): [@Hinaser](https://github.com/Hinaser)
- **Reviewers** (review external contributions before merge):
  [@Hinaser](https://github.com/Hinaser) — all pull requests from outside the
  team are reviewed line by line before they can be merged
- **Approvers** (approve release signing): [@Hinaser](https://github.com/Hinaser)

All accounts with commit or signing access use multi-factor authentication.

## Build integrity

Signed binaries are built exclusively by [GitHub Actions
workflows](.github/workflows/) from the source code in this repository —
never on developer machines. Every release build starts from a version tag,
runs on clean GitHub-hosted runners, and sets product name and version
metadata on the produced binaries (`puppetty-engine`, `puppetty-gui`).
Releases are published from drafts by explicit maintainer action, and
GitHub's immutable releases prevent any modification of published assets.

## Privacy policy

This program will not transfer any information to other networked systems
unless specifically requested by the user or the person installing or
operating it.

In detail:

- puppetty contains **no telemetry, analytics, or auto-update phone-home**.
- All session data (rendered screens, event logs, asciinema recordings)
  stays on the local machine under `~/.puppetty/`.
- Secrets stored via `puppetty cred` live in the operating system keyring
  and are never written to disk, logs, or any network destination.
- The only network-facing behavior is **explicitly user-configured**: if you
  set up a decider command (e.g. an LLM CLI) or the GUI's AI helper command,
  puppetty pipes the rendered terminal screen to *that local command's
  stdin*; whether that command contacts a network service is determined by
  the tool you chose. Credential *values* are never included — a decider
  only ever sees credential names.

## Reporting

To report a security concern or a suspected violation of this policy, open
an issue at <https://github.com/puppetty-org/puppetty/issues> or contact the
maintainer. Reports are investigated with root-cause analysis and findings
are published in the issue tracker.
