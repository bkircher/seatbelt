# AGENTS.md

- This repository provides a shell script that wraps `sandbox-exec` and a
  default Seatbelt profile.
- Verify changes to shell scripts by running `shellcheck`.
- Verify changes to the SBPL profile by running the syntax smoke test:
  `./test-syntax.sh`.
- Ask the user to verify profile behavior by executing `./test.sh` outside of a
  sandbox.
