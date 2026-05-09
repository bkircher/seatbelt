# pi-seatbelt

A macOS sandbox wrapper for the pi coding agent. Two files, no dependencies. The
pi Node.js process itself runs in a sandbox.

It wraps `sandbox-exec` (Apple's Seatbelt) around your agent so it can't read
your secrets or write outside your project, even if you run it with
`--dangerously-skip-permissions` or any equivalent YOLO mode.

## Install

Just symlink or copy `sb` into `$PATH` and `default-profile.sb` into your
`~/.config` directory, respectively. Or put them wherever you prefer.

## Usage

```bash
cd ~/my-project
sb pi
```

`sb` starts the command with a sanitized environment by default: common runtime
variables are preserved, but secrets and arbitrary exported variables are not
passed through. To pass through an additional variable explicitly, use
`--allow-env`:

```bash
sb --allow-env=ATLASSIAN_API_TOKEN pi
sb --allow-env ATLASSIAN_API_TOKEN --allow-env JIRA_API_TOKEN pi
```

## The problem pi-seatbelt fixes

There are some sandbox extensions written for pi, but they are hard to
understand because they all run after the main pi Node.js process is already
running:

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

Arbitrary child processes spawned by the main pi Node.js process are not
automatically sandboxed.

This makes those approaches largely ineffective.

Instead, pi itself needs to be spawned in a sandbox.

## Layering with built-in sandboxes

This works as an outer layer around any agent's own sandbox:

```
pi-seatbelt                  OS-level, file policy
└─ agent's built-in sandbox  tool-level, network filtering, whatever
    └─ your agent process
```

The outer layer handles what the agent shouldn't touch. The inner layer handles
tool-specific permissions and optional network controls. They don't conflict.
Seatbelt rules compose by intersection.

## Comparison with agent-safehouse

[agent-safehouse](https://github.com/eugene1g/agent-safehouse) is a larger
project that solves the same problem. ~50 files, a build system, auto-detection
for different agents, per-toolchain profiles, and per-project config files. It's
well done.

Where this project is stricter:

- Granular read-denies on secrets (AWS credentials with a config exception, gh
  hosts vs. settings, SSH private keys vs. public keys, browser profiles, shell
  history, `.netrc`, `.npmrc`, `.cargo/credentials.toml`). agent-safehouse
  doesn't cover most of these.
- Write-denies inside the project (`.git/hooks`, `.git/config`, `.mcp.json`, IDE
  directories). agent-safehouse grants full write access to the working
  directory.
- Write-denies on `$HOME` shell init files and `~/.gitconfig`. agent-safehouse
  relies on not granting HOME writes in the first place, but toolchain profiles
  can open gaps.

Where agent-safehouse does more:

- Per-toolchain cache directory allowlists (Node, Python, Rust, Go, etc.).
- Agent auto-detection (Claude, Codex, and Amp get different profiles).
- Docker socket blocking by default.
- Optional modules for clipboard, SSH, 1Password, headless browsers.

Pick based on what you want. If you want something you can read in 10 minutes
and tune to your exact setup, this is it. If you want broad coverage maintained
by others, use agent-safehouse. You can also layer them together.

## Customization

`default-profile.sb` is standard
[SBPL](https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf).
The wrapper injects three parameters: `_HOME`, `_PROJECT_DIR`, `_TMPDIR`. Edit
the file to match your setup. Add cache directories your tools need, and block
paths specific to your machine.

## Ideas

- Auth proxy: A reverse proxy running outside the sandbox that injects API keys
  into outbound requests. The sandboxed agent only talks to `localhost` and
  never sees the real keys. [mitmproxy](https://mitmproxy.org/) with header
  injection can do this in one line. Most SDKs (OpenAI, Anthropic, etc.) already
  support a custom `base_url`, which can be used for this.

- Network proxy: Seatbelt is not a DNS-aware outbound firewall. So we can
  disallow all networking
  ```scheme
  (deny network*)
  ```
  We can also allow broad outbound networking
  ```scheme
  (allow network-outbound)
  ```
  but it cannot do domain allowlists natively like
  ```scheme
  (allow network-outbound "example.com:443")
  ```
  Theoretically, we can allow localhost/proxy-style access
  ```scheme
  (deny network*)
  (allow network-outbound (remote tcp "localhost:8080"))
  ```

## Caveats

There are many.

- macOS only. For Linux, maybe look at
  [bubblewrap](https://github.com/containers/bubblewrap). No idea.
- `sandbox-exec` is technically deprecated by Apple. It still works on Sequoia
  and Tahoe, and no replacement exists for third-party use. It is actively
  maintained. Apple still ships security fixes for sandbox escapes and sandbox
  restrictions in current macOS security updates. For example, macOS Sequoia
  15.7.4 had a [fix](https://support.apple.com/en-us/126349) for a sandbox
  breakout.
- Network is wide open. If a secret enters the process, it can be exfiltrated
  easily. Seatbelt cannot deny outgoing network on a host-by-host basis. This
  would require an outgoing network proxy (maybe in the future?).
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
