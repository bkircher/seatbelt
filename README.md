# Seatbelt

A macOS sandbox wrapper for running agents or other tools with SBPL profiles.

## Usage

```bash
cd ~/acme-project
seatbelt --config=acme run pi
```

## The problem `seatbelt` fixes

Some sandbox extensions written for `pi` are hard to reason about because they
all run after the main `pi` Node.js process is already running:

```text
pi Node.js process       unsandboxed
└─ outer shell           effectively just runs the wrapper
   └─ sandbox-exec
      └─ inner shell     sandboxed
         └─ node         sandboxed
            └─ curl      sandboxed
```

The sandbox applies to the inner shell and its process tree. `exec`, `fork`,
`spawn`, etc. do not escape it. However, any extension can spawn its own child
processes, so they can bypass the inner sandbox. This means:

Arbitrary child processes spawned by the main `pi` Node.js process are not
automatically sandboxed.

This makes those approaches largely ineffective. Instead, `pi` itself needs to
be spawned in a sandbox. For this use case, `sandbox-exec` provides a very
straightforward SBPL profile syntax.

However, this still does not handle:

- policy composition
- environment variables
- outbound network firewalling

## Layering with built-in sandboxes

This works as an outer layer around any agent's own sandbox:

```
seatbelt                         OS-level, file policy
└─ agent's built-in sandbox      if any
    └─ your agent process tree
```

The outer layer handles what the agent shouldn't touch. The inner layer handles
tool-specific permissions and optional network controls. They don't conflict.
Seatbelt rules compose by taking the intersection.

## Customization

Profiles are written in
[SBPL](https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf).
The wrapper injects four parameters: `_USERS_DIR`, `_HOME`, `_PROJECT_DIR`, and
`_TMPDIR`. You can add or change your own profiles. Profiles are meant for
defining sandbox policies. They should be small and targeted to the command,
tools, or current project or task.

For user-facing configuration, use `--config`. It allows SBPL profile
composition and environment variable handling. Network policies may come later.

## Debugging SBPL profiles

See logs with something like:

```sh
log stream --style syslog --predicate 'process == "kernel" AND sender == "Sandbox"'
```

or just denials:

```sh
log stream --style syslog --predicate 'process == "kernel" AND sender == "Sandbox" AND eventMessage CONTAINS "deny"'
```

## Ideas / TODO

- Additional profiles for clipboard, SSH, 1Password, headless browsers?
- Auth proxy: A reverse proxy running outside the sandbox that injects API keys
  into outbound requests. The sandboxed agent only talks to `localhost` and
  never sees the real keys. [mitmproxy](https://mitmproxy.org/) with header
  injection can do this in one line. Most SDKs (OpenAI, Anthropic, etc.) already
  support a custom `base_url`, which can be used for this.
- Network proxy: Seatbelt is not a DNS-aware outbound firewall. It can disallow
  all networking:
  ```scheme
  (deny network*)
  ```
  It can also allow broad outbound networking:
  ```scheme
  (allow network-outbound)
  ```
  However, it cannot do domain allowlists natively like:
  ```scheme
  (allow network-outbound "example.com:443")
  ```
  In theory, it can allow localhost/proxy-style access:
  ```scheme
  (deny network*)
  (allow network-outbound (remote tcp "localhost:8080"))
  ```

## Caveats

There are many.

- macOS only. For Linux, maybe look at
  [bubblewrap](https://github.com/containers/bubblewrap).
- `sandbox-exec` is technically deprecated by Apple. It still works on Sequoia
  and Tahoe, and no replacement exists for third-party use. It is actively
  maintained. Apple still ships security fixes for sandbox escapes and sandbox
  restrictions in current macOS security updates. For example, macOS Sequoia
  15.7.4 had a [fix](https://support.apple.com/en-us/126349) for a sandbox
  breakout.
- Network access is wide open. If a secret enters the process, it can be
  exfiltrated easily. Seatbelt cannot deny outbound network access on a
  host-by-host basis. This would require an outbound network proxy (maybe in the
  future?).
- Keychain access is allowed by design, so credential helpers (git, AWS) work
  without the agent seeing raw tokens. But the agent can still perform
  authenticated actions like `git push`.

## Best documentation / references

- `man sandbox-exec`, `man sandbox`, `man sandbox_init`, `man sandboxd` for the
  small official interface.
- Chromium's macOS sandbox docs and production `.sb` policies; they are
  practical and current:
  <https://chromium.googlesource.com/chromium/src/%2B/HEAD/sandbox/mac/README.md>
  and
  <https://www.chromium.org/developers/design-documents/sandbox/osx-sandboxing-design/>
- Existing system profiles under `/System/Library/Sandbox/Profiles`, or
  `/usr/share/sandbox`, depending on macOS version.
- Reverse-engineered SBPL docs are useful because Apple does not publish
  thorough SBPL documentation; Chromium explicitly notes that there is no
  official OS-provided SBPL documentation:
  <https://www.chromium.org/developers/design-documents/sandbox/osx-sandboxing-design/>

## Prior art

- [CJHwong/agent-seatbelt](https://github.com/CJHwong/agent-seatbelt) for Claude
  Code. In my opinion, it uses the right approach.
