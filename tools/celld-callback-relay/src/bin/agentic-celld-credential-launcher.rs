//! Fixed-purpose credential-file launcher for the disposable Celld fleet.
//!
//! Celld v0.2.1 accepts S3 credentials only through its final process
//! environment. The qualification fixture keeps those values out of Docker
//! arguments and Docker's persisted container configuration by mounting a
//! protected AWS profile and resolving it immediately before `exec`.

use anyhow::{anyhow, bail, Context, Result};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::Command,
};

const CREDENTIAL_FILE: &str = "/run/identity/credentials";
const CELLD_BINARY: &str = "/usr/local/bin/celld";
const MAX_CREDENTIAL_BYTES: u64 = 16 * 1024;

struct Credentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

fn bounded_value(value: &str, field: &str, minimum: usize, maximum: usize) -> Result<String> {
    if value.len() < minimum
        || value.len() > maximum
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        bail!("credential field {field} is invalid");
    }
    Ok(value.to_string())
}

fn parse_credentials(source: &str) -> Result<Credentials> {
    let mut in_default = false;
    let mut saw_default = false;
    let mut access_key_id = None;
    let mut secret_access_key = None;
    let mut session_token = None;

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') || line.ends_with(']') {
            if line != "[default]" || saw_default {
                bail!("credential file must contain exactly one default profile");
            }
            in_default = true;
            saw_default = true;
            continue;
        }
        if !in_default {
            bail!("credential assignment appears outside the default profile");
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("credential assignment is malformed"))?;
        let key = key.trim();
        let value = value.trim();
        let slot = match key {
            "aws_access_key_id" => &mut access_key_id,
            "aws_secret_access_key" => &mut secret_access_key,
            "aws_session_token" => &mut session_token,
            _ => bail!("credential file contains an unsupported field"),
        };
        if slot.is_some() {
            bail!("credential file contains a duplicate field");
        }
        *slot = Some(value.to_string());
    }

    if !saw_default {
        bail!("credential file has no default profile");
    }
    Ok(Credentials {
        access_key_id: bounded_value(
            access_key_id
                .as_deref()
                .ok_or_else(|| anyhow!("credential file has no access key id"))?,
            "aws_access_key_id",
            8,
            128,
        )?,
        secret_access_key: bounded_value(
            secret_access_key
                .as_deref()
                .ok_or_else(|| anyhow!("credential file has no secret access key"))?,
            "aws_secret_access_key",
            16,
            256,
        )?,
        session_token: session_token
            .as_deref()
            .map(|value| bounded_value(value, "aws_session_token", 1, 4096))
            .transpose()?,
    })
}

fn load_credentials(path: &Path) -> Result<Credentials> {
    let metadata = fs::symlink_metadata(path).context("inspect protected credential file")?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() == 0
        || metadata.len() > MAX_CREDENTIAL_BYTES
    {
        bail!("credential file must be a protected bounded regular file");
    }
    let source = fs::read_to_string(path).context("read protected credential file")?;
    parse_credentials(&source)
}

fn main() -> Result<()> {
    let credentials = load_credentials(Path::new(CREDENTIAL_FILE))?;
    let mut command = Command::new(CELLD_BINARY);
    command.args(std::env::args_os().skip(1));
    command.env_remove("AWS_SHARED_CREDENTIALS_FILE");
    command.env("AWS_ACCESS_KEY_ID", credentials.access_key_id);
    command.env("AWS_SECRET_ACCESS_KEY", credentials.secret_access_key);
    match credentials.session_token {
        Some(token) => {
            command.env("AWS_SESSION_TOKEN", token);
        }
        None => {
            command.env_remove("AWS_SESSION_TOKEN");
        }
    }
    Err(anyhow!("exec Celld failed: {}", command.exec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn parses_only_one_complete_default_profile() {
        let parsed = parse_credentials(
            "[default]\naws_access_key_id = CELLDACCESS01\naws_secret_access_key = abcdefghijklmnopqrstuvwxyz012345\n",
        )
        .unwrap();
        assert_eq!(parsed.access_key_id, "CELLDACCESS01");
        assert_eq!(parsed.secret_access_key, "abcdefghijklmnopqrstuvwxyz012345");
        assert!(parsed.session_token.is_none());

        for invalid in [
            "aws_access_key_id=x\n",
            "[other]\naws_access_key_id=CELLDACCESS01\naws_secret_access_key=abcdefghijklmnopqrstuvwxyz012345\n",
            "[default]\naws_access_key_id=CELLDACCESS01\naws_access_key_id=CELLDACCESS02\naws_secret_access_key=abcdefghijklmnopqrstuvwxyz012345\n",
            "[default]\naws_access_key_id=CELLDACCESS01\nunknown=value\naws_secret_access_key=abcdefghijklmnopqrstuvwxyz012345\n",
            "[default]\naws_access_key_id=CELLDACCESS01\n",
        ] {
            assert!(parse_credentials(invalid).is_err());
        }
    }

    #[test]
    fn protected_file_validation_rejects_group_readable_credentials() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "[default]").unwrap();
        writeln!(file, "aws_access_key_id = CELLDACCESS01").unwrap();
        writeln!(
            file,
            "aws_secret_access_key = abcdefghijklmnopqrstuvwxyz012345"
        )
        .unwrap();
        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load_credentials(file.path()).is_err());
        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_credentials(file.path()).is_ok());
    }
}
