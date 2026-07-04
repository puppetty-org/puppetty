# Security policy

## Supported versions

The latest release of each channel (npm package, GUI installer) receives
security fixes. Older versions are not patched — please update.

## Reporting a vulnerability

Please report vulnerabilities privately via
[GitHub security advisories](https://github.com/puppetty-org/puppetty/security/advisories/new)
rather than public issues. You can expect an initial response within a few
days. Coordinated disclosure is appreciated; credit is given in release
notes unless you prefer otherwise.

## Security model (short version)

puppetty drives terminals programmatically, so its design treats prompt
answering as a privilege hierarchy:

- **Secrets are never automated blindly**: password/passphrase/token prompts
  are classified `forbid` and are answered only by a human or from the OS
  keyring (`credential` rules). Secret values never appear in logs, screen
  tails sent to deciders, or session recordings.
- **Destructive prompts need a human** by default (`onDanger: "human"`);
  routing them to an LLM decider is an explicit opt-in.
- **Event logs record attribution, not content**, for human-typed input
  (byte counts only).
- **Remote debugging of the GUI (CDP) is off by default** and opt-in via
  Settings.
- Session control endpoints are named pipes / Unix domain sockets, reachable
  only from the local machine and the same user context.

## Build integrity

Release builds run from version tags on clean GitHub-hosted runners. The GUI
installer endpoint publishes per-platform app packages with SHA-256 files,
and the install scripts verify the selected package before installing the
downloaded files.

## Privacy

puppetty contains no telemetry, analytics, or auto-update phone-home. Session
data stays on the local machine under `~/.puppetty/`, and secrets stored with
`puppetty cred` live in the operating system keyring. Network access happens
only through tools you explicitly configure, such as a decider command.
