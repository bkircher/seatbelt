# AGENTS.md

- This repository provides a shell script that wraps `sandbox-exec` and a
  default Seatbelt profile.
- Verify changes to shell scripts by running `shellcheck`.
- Verify changes to the SBPL profile by running the following syntax/parse/smoke
  test:

<shell>
sandbox-exec -f default-profile.sb \
    -D "_USERS_DIR=$(cd "$HOME/.." && pwd -P)" \
    -D "_HOME=$HOME" \
    -D "_PROJECT_DIR=$(pwd -P)" \
    -D "_TMPDIR=${TMPDIR:-/tmp}" \
    /usr/bin/true
</shell>

- Verify default profile behavior by executing `./test.sh`.
- If running the behavioral tests through the `sb` wrapper, pass the repository
  profile explicitly:

<shell>
./sb --profile ./default-profile.sb ./test.sh
</shell>
