# Seatbelt

Seatbelt runs a command inside a macOS `sandbox-exec` profile. It is meant for
wrapping coding agents and other developer tools with an OS-level filesystem
policy that applies to the whole process tree.

## The problem Seatbelt solves

Agents usually come with a sandboxing mechanism. However, the process you start
is usually not sandboxed, and both the agent itself and its extensions have to
adhere to a calling convention in order to have tool calls sandboxed. Thus,
escaping a sandbox is quite easy for an agent (in theory).

Example with pi:

```raw
pi Node.js process       unsandboxed
└─ outer shell           effectively just runs the wrapper
   └─ sandbox-exec
      └─ inner shell     sandboxed
         └─ node         sandboxed
            └─ curl      sandboxed
```

A typical Codex runtime looks like:

```raw
node codex.js                  unsandboxed launcher
└─ codex native binary         unsandboxed agent/core
   └─ /usr/bin/sandbox-exec    sandbox launcher
      └─ shell/tool command    sandboxed
         └─ child processes    sandboxed
```

A better approach is to have the sandboxing process start the agent, so the
agent and its child processes cannot escape.

This can even work as an outer layer around any agent's own sandbox:

```raw
seatbelt                         OS-level
└─ agent's built-in sandbox      if any
    └─ your agent's process tree
```

In addition, the outer sandbox must allow whatever the agent needs: API and
network access, configuration, credentials, logs, temporary directories,
sockets, etc. Any inner sandbox can only further restrict access; it cannot
grant permissions denied by the outer sandbox. This makes the outer sandbox the
baseline permission boundary.

## Usage

```bash
cd ~/my-project
seatbelt run pi
seatbelt --config acme run pi
seatbelt --allow-read ~/src/shared --allow-write ~/build-dir run pi
```

Use `seatbelt print-profile` to inspect the composed sandbox profile, or run
`seatbelt run --dry-run ...` to print the final `sandbox-exec` command.

## Configuration

By default, Seatbelt loads `~/.config/seatbelt/default.yaml`. Named
configuration files such as `acme.yaml` or `acme.yml` live in
`~/.config/seatbelt/` and can be selected with `--config acme`. Profiles live in
`~/.config/seatbelt/profiles/` and are written in
[SBPL](https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf).

A configuration composes one or more profiles and can allow extra environment
variables or paths:

```yaml
profiles:
  - base.sb
  - project.sb
  - tools/git.sb
  - agents/pi.sb

allow:
  env:
    - ATLASSIAN_API_TOKEN
  read:
    - ~/src/shared
  write:
    - ~/project-output
```

`--allow-read`, `--allow-write`, and `--allow-env` add permissions for a single
run. Path entries must already exist, and broad directories such as `/`,
`/Users`, `$HOME`, and `$HOME/src` are rejected.

## Notes

- Seatbelt is an outer sandbox. It can be used alongside an agent's built-in
  sandbox; the effective access is the intersection of both policies.
- Run it from a project directory. Seatbelt refuses to run from `$HOME` because
  that would make the home directory the project boundary.
- Network access is not a domain-aware firewall. Profiles can deny or broadly
  allow networking, but host-level policy needs a proxy or another tool.
- macOS only. `sandbox-exec` is deprecated by Apple, but it is still shipped and
  receives security fixes in current macOS releases.
- Keychain access is allowed so tools such as Git and AWS credential helpers can
  work. Be aware that sandboxed commands can still perform authenticated
  actions.

## Install

```bash
cargo build --release
make install
```

`make install` copies the binary to `~/bin` if that directory exists and
installs the bundled configuration files and profiles under
`~/.config/seatbelt/`.

## References

- `man sandbox-exec`, `man sandbox`, `man sandbox_init`, and `man sandboxd`
- Chromium's macOS sandbox documentation:
  <https://chromium.googlesource.com/chromium/src/+/HEAD/sandbox/mac/README.md>
- Prior art: <https://github.com/CJHwong/agent-seatbelt>
