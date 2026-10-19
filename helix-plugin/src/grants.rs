//! Explicit user grants for operations outside the component sandbox.
use serde::{de::Error as _, Deserialize, Deserializer};

/// The user's `allow-process` grant in `plugins.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProcessGrant {
    /// Process execution has not been granted.
    #[default]
    Denied,
    /// Any program may run with the user's rights.
    All,
    /// Only these exact `argv[0]` values may run. Paths are not reduced to basenames.
    Programs(Vec<String>),
}

impl<'de> Deserialize<'de> for ProcessGrant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Grant {
            Enabled(bool),
            Programs(Vec<String>),
        }
        match Grant::deserialize(deserializer)? {
            Grant::Enabled(true) => Ok(Self::All),
            Grant::Enabled(false) => Ok(Self::Denied),
            Grant::Programs(mut programs) => {
                if programs
                    .iter()
                    .any(|program| program.is_empty() || program.contains('\0'))
                {
                    return Err(D::Error::custom(
                        "allow-process programs must be nonempty and contain no NUL",
                    ));
                }
                let mut seen = std::collections::HashSet::new();
                programs.retain(|program| seen.insert(program.clone()));
                if programs.is_empty() {
                    Ok(Self::Denied)
                } else {
                    Ok(Self::Programs(programs))
                }
            }
        }
    }
}

impl ProcessGrant {
    /// Short status for `:plugin-list` and `--health`.
    pub fn describe(&self) -> String {
        match self {
            Self::Denied => "not granted".into(),
            Self::All => "granted".into(),
            Self::Programs(programs) if programs.is_empty() => "not granted".into(),
            Self::Programs(programs) => format!("only {}", programs.join(", ")),
        }
    }

    pub(crate) fn authorize(&self, plugin: &str, program: &str) -> Result<(), String> {
        let allowed = match self {
            Self::Denied => false,
            Self::All => true,
            Self::Programs(programs) => programs.iter().any(|allowed| allowed == program),
        };
        if !allowed {
            return Err(match self {
                Self::Programs(programs) if !programs.is_empty() => format!(
                    "process program {program:?} not granted: add it to allow-process under [{plugin}] in plugins.toml"
                ),
                _ => format!("process not granted: add allow-process = true under [{plugin}] in plugins.toml"),
            });
        }
        // A future optional first-use prompt belongs here, after the explicit
        // grant succeeds. For now the user's configuration is the only grant.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<ProcessGrant, toml::de::Error> {
        let table: toml::Table = toml::from_str(source)?;
        table["allow-process"].clone().try_into()
    }

    #[test]
    fn grants_are_explicit_and_validate_program_names() {
        assert_eq!(ProcessGrant::default(), ProcessGrant::Denied);
        assert_eq!(parse("allow-process = true").unwrap(), ProcessGrant::All);
        assert_eq!(
            parse("allow-process = false").unwrap(),
            ProcessGrant::Denied
        );
        assert_eq!(parse("allow-process = []").unwrap(), ProcessGrant::Denied);
        assert_eq!(
            parse("allow-process = ['rg', 'fd', 'rg']").unwrap(),
            ProcessGrant::Programs(vec!["rg".into(), "fd".into()])
        );
        for invalid in [
            "allow-process = 1",
            "allow-process = 'true'",
            "allow-process = ['']",
            "allow-process = [1]",
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn allow_lists_match_exact_argv_zero_not_a_basename() {
        let grant = ProcessGrant::Programs(vec!["rg".into(), "fd".into()]);
        assert!(grant.authorize("files", "rg").is_ok());
        assert!(grant.authorize("files", "/tmp/rg").is_err());
        assert!(grant.authorize("files", "sh").is_err());
        assert_eq!(grant.describe(), "only rg, fd");
        assert_eq!(ProcessGrant::All.describe(), "granted");
        assert_eq!(ProcessGrant::Denied.describe(), "not granted");
        assert_eq!(
            ProcessGrant::Denied.authorize("files", "rg").unwrap_err(),
            "process not granted: add allow-process = true under [files] in plugins.toml"
        );
    }
}
