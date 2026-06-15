use std::{
    env,
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::Path,
    process::Command as ProcessCommand,
};

use eyre::{Context, Result, eyre};

use crate::{env_name::EnvName, paths::CanonicalPathBuf, profile::SandboxProfile};

const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

pub(crate) struct SandboxContext<'a> {
    pub(crate) profile: &'a SandboxProfile,
    pub(crate) resolved_users_dir: CanonicalPathBuf,
    pub(crate) resolved_home: CanonicalPathBuf,
    pub(crate) project_dir: CanonicalPathBuf,
    pub(crate) resolved_tmpdir: CanonicalPathBuf,
}

pub(crate) trait EnvSource {
    fn var_os(&self, name: &str) -> Option<OsString>;
}

pub(crate) struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var_os(&self, name: &str) -> Option<OsString> {
        env::var_os(name)
    }
}

pub(crate) fn build_final_command(
    sandbox_context: &SandboxContext<'_>,
    env_source: &impl EnvSource,
    allow_env: &[EnvName],
    command: &[OsString],
) -> Result<Vec<OsString>> {
    let mut final_command = vec![OsString::from(SANDBOX_EXEC_PATH)];

    match sandbox_context.profile {
        SandboxProfile::File(profile) => {
            final_command.push(OsString::from("-f"));
            final_command.push(profile.as_os_str().to_os_string());
        }
        SandboxProfile::Text(profile) => {
            final_command.push(OsString::from("-p"));
            final_command.push(OsString::from(profile));
        }
    }

    final_command.extend([
        OsString::from("-D"),
        env_pair_path("_USERS_DIR", sandbox_context.resolved_users_dir.as_path()),
        OsString::from("-D"),
        env_pair_path("_HOME", sandbox_context.resolved_home.as_path()),
        OsString::from("-D"),
        env_pair_path("_PROJECT_DIR", sandbox_context.project_dir.as_path()),
        OsString::from("-D"),
        env_pair_path("_TMPDIR", sandbox_context.resolved_tmpdir.as_path()),
        OsString::from("/usr/bin/env"),
        OsString::from("-i"),
        env_pair_path("HOME", sandbox_context.resolved_home.as_path()),
        env_pair("USER", env_source.var_os("USER").unwrap_or_default()),
        env_pair(
            "SHELL",
            env_source
                .var_os("SHELL")
                .unwrap_or_else(|| OsString::from("/bin/zsh")),
        ),
        env_pair(
            "TERM",
            env_source
                .var_os("TERM")
                .unwrap_or_else(|| OsString::from("xterm-256color")),
        ),
        env_pair(
            "LANG",
            env_source
                .var_os("LANG")
                .unwrap_or_else(|| OsString::from("en_US.UTF-8")),
        ),
        env_pair("PATH", env_source.var_os("PATH").unwrap_or_default()),
        env_pair_path("TMPDIR", sandbox_context.resolved_tmpdir.as_path()),
    ]);

    append_if_set(&mut final_command, env_source, "SSH_AUTH_SOCK");
    append_if_set(&mut final_command, env_source, "EDITOR");
    append_if_set(&mut final_command, env_source, "VISUAL");
    append_if_set(&mut final_command, env_source, "XDG_CONFIG_HOME");
    append_if_set(&mut final_command, env_source, "XDG_DATA_HOME");
    append_if_set(&mut final_command, env_source, "XDG_CACHE_HOME");
    append_if_set(&mut final_command, env_source, "XDG_RUNTIME_DIR");

    for env_name in allow_env {
        let value = env_source
            .var_os(env_name.as_str())
            .ok_or_else(|| eyre!("environment variable is not set: {env_name}"))?;
        final_command.push(env_pair(env_name.as_str(), value));
    }

    final_command.extend(command.iter().cloned());

    Ok(final_command)
}

fn env_pair_path(name: &str, value: &Path) -> OsString {
    env_pair(name, value.as_os_str())
}

fn env_pair(name: &str, value: impl AsRef<OsStr>) -> OsString {
    let mut pair = OsString::from(name);
    pair.push("=");
    pair.push(value.as_ref());
    pair
}

fn append_if_set(command: &mut Vec<OsString>, env_source: &impl EnvSource, name: &str) {
    if let Some(value) = env_source.var_os(name) {
        command.push(env_pair(name, value));
    }
}

pub(crate) fn exec_command(final_command: &[OsString]) -> Result<()> {
    let (program, args) = final_command
        .split_first()
        .ok_or_else(|| eyre!("internal error: final command is empty"))?;

    let error = ProcessCommand::new(program).args(args).exec();
    Err(error).wrap_err_with(|| format!("failed to execute {}", program.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

    use super::*;
    use crate::{profile::SandboxProfile, test_support::*};

    struct TestEnv {
        values: BTreeMap<String, OsString>,
    }

    impl TestEnv {
        fn new(values: BTreeMap<String, OsString>) -> Self {
            Self { values }
        }
    }

    impl EnvSource for TestEnv {
        fn var_os(&self, name: &str) -> Option<OsString> {
            self.values.get(name).cloned()
        }
    }

    fn empty_env() -> TestEnv {
        TestEnv::new(BTreeMap::new())
    }

    fn file_profile() -> SandboxProfile {
        SandboxProfile::File(PathBuf::from("/profiles/default.sb"))
    }

    fn sandbox_context(profile: &SandboxProfile) -> SandboxContext<'_> {
        SandboxContext {
            profile,
            resolved_users_dir: canonical_path("/Users"),
            resolved_home: canonical_path("/Users/alice"),
            project_dir: canonical_path("/Users/alice/project"),
            resolved_tmpdir: canonical_path("/tmp/alice"),
        }
    }

    #[test]
    fn build_final_command_assembles_sandbox_env_and_command() {
        let env_source = TestEnv::new(BTreeMap::from([
            ("USER".to_owned(), os("alice")),
            ("SHELL".to_owned(), os("/bin/fish")),
            ("TERM".to_owned(), os("ansi")),
            ("LANG".to_owned(), os("C.UTF-8")),
            ("PATH".to_owned(), os("/usr/bin:/bin")),
            ("SSH_AUTH_SOCK".to_owned(), os("/tmp/ssh.sock")),
            ("EDITOR".to_owned(), os("vim")),
            ("VISUAL".to_owned(), os("nvim")),
            ("XDG_CONFIG_HOME".to_owned(), os("/Users/alice/.config")),
            ("XDG_DATA_HOME".to_owned(), os("/Users/alice/.local/share")),
            ("XDG_CACHE_HOME".to_owned(), os("/Users/alice/.cache")),
            ("XDG_RUNTIME_DIR".to_owned(), os("/tmp/runtime")),
            ("EXTRA_TOKEN".to_owned(), os("secret")),
        ]));

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[env_name("EXTRA_TOKEN")],
            &[os("echo"), os("hello world")],
        ));

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-f"),
                os("/profiles/default.sb"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER=alice"),
                os("SHELL=/bin/fish"),
                os("TERM=ansi"),
                os("LANG=C.UTF-8"),
                os("PATH=/usr/bin:/bin"),
                os("TMPDIR=/tmp/alice"),
                os("SSH_AUTH_SOCK=/tmp/ssh.sock"),
                os("EDITOR=vim"),
                os("VISUAL=nvim"),
                os("XDG_CONFIG_HOME=/Users/alice/.config"),
                os("XDG_DATA_HOME=/Users/alice/.local/share"),
                os("XDG_CACHE_HOME=/Users/alice/.cache"),
                os("XDG_RUNTIME_DIR=/tmp/runtime"),
                os("EXTRA_TOKEN=secret"),
                os("echo"),
                os("hello world"),
            ]
        );
    }

    #[test]
    fn build_final_command_uses_defaults_for_missing_base_env() {
        let env_source = empty_env();
        let profile = file_profile();
        let context = sandbox_context(&profile);

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[],
            &[os("true")],
        ));

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-f"),
                os("/profiles/default.sb"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER="),
                os("SHELL=/bin/zsh"),
                os("TERM=xterm-256color"),
                os("LANG=en_US.UTF-8"),
                os("PATH="),
                os("TMPDIR=/tmp/alice"),
                os("true"),
            ]
        );
    }

    #[test]
    fn build_final_command_uses_generated_profile_text() {
        let env_source = empty_env();
        let profile =
            SandboxProfile::Text("(version 1)\n(import \"/profiles/base.sb\")\n".to_owned());
        let context = sandbox_context(&profile);

        let actual = must(build_final_command(
            &context,
            &env_source,
            &[],
            &[os("true")],
        ));

        assert_eq!(
            actual,
            vec![
                os(SANDBOX_EXEC_PATH),
                os("-p"),
                os("(version 1)\n(import \"/profiles/base.sb\")\n"),
                os("-D"),
                os("_USERS_DIR=/Users"),
                os("-D"),
                os("_HOME=/Users/alice"),
                os("-D"),
                os("_PROJECT_DIR=/Users/alice/project"),
                os("-D"),
                os("_TMPDIR=/tmp/alice"),
                os("/usr/bin/env"),
                os("-i"),
                os("HOME=/Users/alice"),
                os("USER="),
                os("SHELL=/bin/zsh"),
                os("TERM=xterm-256color"),
                os("LANG=en_US.UTF-8"),
                os("PATH="),
                os("TMPDIR=/tmp/alice"),
                os("true"),
            ]
        );
    }

    #[test]
    fn build_final_command_rejects_unset_allow_env_name() {
        let env_source = empty_env();

        let profile = file_profile();
        let context = sandbox_context(&profile);

        let result =
            build_final_command(&context, &env_source, &[env_name("TOKEN")], &[os("true")]);

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("environment variable is not set: TOKEN".to_owned())
        );
    }
}
