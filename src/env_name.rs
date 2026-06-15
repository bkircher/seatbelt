use std::str::FromStr;

use serde::{Deserialize, Deserializer};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvName(String);

impl EnvName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EnvName {
    type Error = InvalidEnvName;

    fn try_from(name: String) -> std::result::Result<Self, Self::Error> {
        if is_valid_env_name(&name) {
            Ok(Self(name))
        } else {
            Err(InvalidEnvName { name })
        }
    }
}

impl FromStr for EnvName {
    type Err = InvalidEnvName;

    fn from_str(name: &str) -> std::result::Result<Self, Self::Err> {
        Self::try_from(name.to_owned())
    }
}

impl<'de> Deserialize<'de> for EnvName {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Self::try_from(name).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for EnvName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid environment variable name: {name}")]
pub(crate) struct InvalidEnvName {
    name: String,
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();

    let Some(first) = chars.next() else {
        return false;
    };

    if !is_ascii_alpha_or_underscore(first) {
        return false;
    }

    chars.all(is_ascii_alnum_or_underscore)
}

fn is_ascii_alpha_or_underscore(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn is_ascii_alnum_or_underscore(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_environment_variable_names() {
        assert_eq!(
            EnvName::try_from("TOKEN".to_owned()).map(|name| name.to_string()),
            Ok("TOKEN".to_owned())
        );
        assert_eq!(
            EnvName::try_from("_TOKEN_1".to_owned()).map(|name| name.to_string()),
            Ok("_TOKEN_1".to_owned())
        );
    }

    #[test]
    fn rejects_invalid_environment_variable_names() {
        assert_eq!(
            EnvName::try_from(String::new()).err(),
            Some(InvalidEnvName {
                name: String::new()
            })
        );
        assert_eq!(
            EnvName::try_from("1TOKEN".to_owned()).err(),
            Some(InvalidEnvName {
                name: "1TOKEN".to_owned()
            })
        );
        assert_eq!(
            EnvName::try_from("BAD-NAME".to_owned()).err(),
            Some(InvalidEnvName {
                name: "BAD-NAME".to_owned()
            })
        );
        assert_eq!(
            EnvName::try_from("BAD.NAME".to_owned()).err(),
            Some(InvalidEnvName {
                name: "BAD.NAME".to_owned()
            })
        );
    }

    #[test]
    fn deserializes_valid_environment_variable_names() {
        let actual = yaml_serde::from_str::<Vec<EnvName>>("- TOKEN\n- _TOKEN_1\n").map(|names| {
            names
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        });

        assert_eq!(
            actual.map_err(|error| error.to_string()),
            Ok(vec!["TOKEN".to_owned(), "_TOKEN_1".to_owned()])
        );
    }

    #[test]
    fn rejects_invalid_environment_variable_names_during_deserialization() {
        let result: std::result::Result<Vec<EnvName>, _> = yaml_serde::from_str("- 1TOKEN\n");

        assert!(
            result.err().map(|error| error.to_string()).is_some_and(
                |message| message.contains("invalid environment variable name: 1TOKEN")
            )
        );
    }
}
