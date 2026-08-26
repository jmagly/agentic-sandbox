//! Fixed-purpose privileged QEMU storage cleanup helper.
//!
//! This binary accepts no arguments. A bounded flat JSON request arrives on
//! stdin through one exact sudoers rule. It can move only a journal-named
//! Celld quarantine below the fixed Titan VM root into a root-only sibling on
//! the same filesystem, then remove that exact captured inode without
//! following links. Workspace code and caller-selected executable paths are
//! never evaluated.

use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::BTreeMap,
    env,
    ffi::{CStr, CString},
    fs,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

const REQUEST_SCHEMA: &str = "agentic-sandbox.celld-qemu-cleanup-helper/v1";
const RESPONSE_SCHEMA: &str = "agentic-sandbox.celld-qemu-cleanup-helper-result/v1";
const OPERATION: &str = "capture-delete";
const VM_ROOT: &str = "/build/agentic-sandbox/vms";
const CAPTURE_ROOT: &str = "/build/agentic-sandbox/.celld-qemu-cleanup";
const MAX_INPUT_BYTES: u64 = 8 * 1024;
const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_DEPTH: usize = 128;

#[derive(Debug, PartialEq, Eq)]
struct Request {
    run_id: String,
    source_path: PathBuf,
    capture_path: PathBuf,
    expected_uid: u32,
    expected_gid: u32,
    expected_device: u64,
    expected_inode: u64,
}

fn tombstone_path(capture_path: &Path) -> PathBuf {
    let name = capture_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("invalid");
    capture_path.with_file_name(format!("{name}.deleted"))
}

struct FlatJson<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> FlatJson<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            offset: 0,
        }
    }

    fn whitespace(&mut self) {
        while matches!(
            self.bytes.get(self.offset),
            Some(b' ' | b'\n' | b'\r' | b'\t')
        ) {
            self.offset += 1;
        }
    }

    fn byte(&mut self, expected: u8) -> Result<()> {
        self.whitespace();
        if self.bytes.get(self.offset) != Some(&expected) {
            bail!("request JSON is malformed");
        }
        self.offset += 1;
        Ok(())
    }

    fn string(&mut self) -> Result<String> {
        self.byte(b'"')?;
        let start = self.offset;
        while let Some(byte) = self.bytes.get(self.offset).copied() {
            if byte == b'"' {
                let value = std::str::from_utf8(&self.bytes[start..self.offset])
                    .context("request string is not UTF-8")?;
                self.offset += 1;
                if value
                    .bytes()
                    .any(|candidate| candidate == b'\\' || candidate < 0x20)
                {
                    bail!("request strings may not contain escapes or control bytes");
                }
                return Ok(value.to_string());
            }
            self.offset += 1;
        }
        bail!("request JSON string is unterminated")
    }

    fn object(mut self) -> Result<BTreeMap<String, String>> {
        let mut values = BTreeMap::new();
        self.byte(b'{')?;
        self.whitespace();
        if self.bytes.get(self.offset) == Some(&b'}') {
            self.offset += 1;
        } else {
            loop {
                let key = self.string()?;
                self.byte(b':')?;
                let value = self.string()?;
                if values.insert(key, value).is_some() {
                    bail!("request JSON contains a duplicate field");
                }
                self.whitespace();
                match self.bytes.get(self.offset) {
                    Some(b',') => self.offset += 1,
                    Some(b'}') => {
                        self.offset += 1;
                        break;
                    }
                    _ => bail!("request JSON object is malformed"),
                }
            }
        }
        self.whitespace();
        if self.offset != self.bytes.len() {
            bail!("request JSON contains trailing data");
        }
        Ok(values)
    }
}

fn take(values: &mut BTreeMap<String, String>, name: &str) -> Result<String> {
    values
        .remove(name)
        .ok_or_else(|| anyhow!("request is missing {name}"))
}

fn safe_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

fn uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|offset| bytes[*offset] == b'-')
        && bytes.iter().enumerate().all(|(offset, byte)| {
            [8, 13, 18, 23].contains(&offset)
                || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        })
        && matches!(bytes[14], b'1'..=b'5')
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

fn safe_quarantine_name(name: &str) -> bool {
    let Some((resource, mutation)) = name
        .strip_prefix('.')
        .and_then(|value| value.rsplit_once(".cleanup-"))
    else {
        return false;
    };
    resource.starts_with("celld-")
        && (7..=68).contains(&resource.len())
        && resource
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && uuid(mutation)
}

fn safe_tombstone_name(name: &str) -> bool {
    name.strip_suffix(".final.deleted")
        .map_or(false, safe_quarantine_name)
}

fn parse_request(source: &str) -> Result<Request> {
    let mut values = FlatJson::new(source).object()?;
    if take(&mut values, "schema_version")? != REQUEST_SCHEMA {
        bail!("request schema is unsupported");
    }
    if take(&mut values, "operation")? != OPERATION {
        bail!("request operation is unsupported");
    }
    let run_id = take(&mut values, "run_id")?;
    let source_path = PathBuf::from(take(&mut values, "source_path")?);
    let capture_path = PathBuf::from(take(&mut values, "capture_path")?);
    let expected_uid = take(&mut values, "expected_uid")?
        .parse::<u32>()
        .context("expected_uid is invalid")?;
    let expected_gid = take(&mut values, "expected_gid")?
        .parse::<u32>()
        .context("expected_gid is invalid")?;
    let expected_device = take(&mut values, "expected_device")?
        .parse::<u64>()
        .context("expected_device is invalid")?;
    let expected_inode = take(&mut values, "expected_inode")?
        .parse::<u64>()
        .context("expected_inode is invalid")?;
    if !values.is_empty() {
        bail!("request contains unsupported fields");
    }
    if !safe_run_id(&run_id) {
        bail!("run_id is invalid");
    }
    let source_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("source_path has no safe final component"))?;
    if !safe_quarantine_name(source_name) || source_path != Path::new(VM_ROOT).join(source_name) {
        bail!("source_path is outside the exact fixed quarantine allowlist");
    }
    let expected_capture = Path::new(CAPTURE_ROOT)
        .join(&run_id)
        .join(format!("{source_name}.final"));
    if capture_path != expected_capture {
        bail!("capture_path is not the deterministic exact-run capture");
    }
    Ok(Request {
        run_id,
        source_path,
        capture_path,
        expected_uid,
        expected_gid,
        expected_device,
        expected_inode,
    })
}

fn metadata_if_present(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn require_root_directory(path: &Path, owner_only: bool) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect fixed directory {}", path.display()))?;
    let forbidden = if owner_only { 0o077 } else { 0o022 };
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.mode() & forbidden != 0
    {
        bail!(
            "fixed directory {} has unsafe ownership or mode",
            path.display()
        );
    }
    Ok(metadata)
}

fn ensure_root_only_directory(path: &Path) -> Result<fs::Metadata> {
    match metadata_if_present(path)? {
        Some(_) => {}
        None => {
            fs::create_dir(path).with_context(|| format!("create {}", path.display()))?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("protect {}", path.display()))?;
        }
    }
    require_root_directory(path, true)
}

fn exact_identity(metadata: &fs::Metadata, request: &Request, description: &str) -> Result<()> {
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != request.expected_uid
        || metadata.gid() != request.expected_gid
        || metadata.dev() != request.expected_device
        || metadata.ino() != request.expected_inode
    {
        bail!("{description} does not match the exact expected directory identity");
    }
    Ok(())
}

fn expected_group_is_authorized(
    primary_gid: u32,
    supplementary_gids: &[u32],
    expected_gid: u32,
) -> bool {
    expected_gid == primary_gid || supplementary_gids.contains(&expected_gid)
}

fn sudo_caller_supplementary_groups(sudo_uid: u32, sudo_gid: u32) -> Result<Vec<u32>> {
    let sudo_user = env::var("SUDO_USER").context("SUDO_USER is required")?;
    let sudo_user = CString::new(sudo_user).context("SUDO_USER is invalid")?;
    unsafe {
        let passwd = libc::getpwuid(sudo_uid);
        if passwd.is_null()
            || (*passwd).pw_uid != sudo_uid
            || (*passwd).pw_gid != sudo_gid
            || CStr::from_ptr((*passwd).pw_name).to_bytes() != sudo_user.as_bytes()
        {
            bail!("sudo caller identity does not match the local account database");
        }
        let mut count = 0;
        libc::getgrouplist(
            sudo_user.as_ptr(),
            sudo_gid,
            std::ptr::null_mut(),
            &mut count,
        );
        if !(1..=1024).contains(&count) {
            bail!("sudo caller group vector is outside the fixed bound");
        }
        let mut groups = vec![0 as libc::gid_t; count as usize];
        if libc::getgrouplist(
            sudo_user.as_ptr(),
            sudo_gid,
            groups.as_mut_ptr(),
            &mut count,
        ) < 0
            || count < 1
            || count as usize > groups.len()
        {
            bail!("sudo caller group vector changed during verification");
        }
        Ok(groups[..count as usize].to_vec())
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory {} for sync", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

fn tombstone_body(request: &Request) -> String {
    format!(
        "{{\"schema_version\":\"{RESPONSE_SCHEMA}\",\"status\":\"deleted\",\"source_path\":\"{}\",\"capture_path\":\"{}\",\"expected_uid\":\"{}\",\"expected_gid\":\"{}\",\"expected_device\":\"{}\",\"expected_inode\":\"{}\"}}\n",
        request.source_path.display(),
        request.capture_path.display(),
        request.expected_uid,
        request.expected_gid,
        request.expected_device,
        request.expected_inode,
    )
}

fn verify_tombstone(path: &Path, request: &Request) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect tombstone {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > 1024
    {
        bail!("helper tombstone has unsafe ownership or mode");
    }
    let mut value = String::new();
    File::open(path)
        .with_context(|| format!("open tombstone {}", path.display()))?
        .read_to_string(&mut value)
        .context("read helper tombstone")?;
    if value != tombstone_body(request) {
        bail!("helper tombstone does not match the exact deleted identity");
    }
    Ok(())
}

fn verify_prior_tombstone(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect prior tombstone {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > 1024
    {
        bail!("prior helper tombstone has unsafe ownership or mode");
    }
    Ok(())
}

fn write_tombstone(path: &Path, request: &Request) -> Result<()> {
    if metadata_if_present(path)?.is_some() {
        verify_tombstone(path, request)?;
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create tombstone {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("protect tombstone {}", path.display()))?;
    file.write_all(tombstone_body(request).as_bytes())
        .context("write helper tombstone")?;
    file.sync_all().context("sync helper tombstone")?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("verify tombstone {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("helper tombstone was substituted after creation");
    }
    verify_tombstone(path, request)
}

fn verify_tree(path: &Path, uid: u32, gid: u32, count: &mut usize, depth: usize) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        bail!("captured tree exceeds the fixed depth bound");
    }
    for entry in fs::read_dir(path).with_context(|| format!("enumerate {}", path.display()))? {
        *count += 1;
        if *count > MAX_TREE_ENTRIES {
            bail!("captured tree exceeds the fixed entry bound");
        }
        let entry = entry.context("read captured tree entry")?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)
            .with_context(|| format!("inspect captured entry {}", child.display()))?;
        if metadata.uid() != uid || metadata.gid() != gid {
            bail!("captured tree contains a foreign owner");
        }
        let kind = metadata.file_type();
        if kind.is_dir() && !kind.is_symlink() {
            verify_tree(&child, uid, gid, count, depth + 1)?;
        } else if (kind.is_file() || kind.is_symlink()) && metadata.nlink() == 1 {
            continue;
        } else {
            bail!("captured tree contains a special type or hard link");
        }
    }
    Ok(())
}

fn delete_tree(path: &Path, depth: usize) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        bail!("captured tree exceeds the fixed deletion depth");
    }
    for entry in
        fs::read_dir(path).with_context(|| format!("enumerate {} for deletion", path.display()))?
    {
        let entry = entry.context("read captured deletion entry")?;
        let child = entry.path();
        let kind = fs::symlink_metadata(&child)
            .with_context(|| format!("inspect {} before deletion", child.display()))?
            .file_type();
        if kind.is_dir() && !kind.is_symlink() {
            delete_tree(&child, depth + 1)?;
            fs::remove_dir(&child)
                .with_context(|| format!("remove directory {}", child.display()))?;
        } else {
            fs::remove_file(&child).with_context(|| format!("remove leaf {}", child.display()))?;
        }
    }
    Ok(())
}

fn execute(request: &Request) -> Result<&'static str> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("helper must execute as root through sudo");
    }
    let sudo_uid = env::var("SUDO_UID")
        .context("SUDO_UID is required")?
        .parse::<u32>()
        .context("SUDO_UID is invalid")?;
    let sudo_gid = env::var("SUDO_GID")
        .context("SUDO_GID is required")?
        .parse::<u32>()
        .context("SUDO_GID is invalid")?;
    let supplementary_gids = sudo_caller_supplementary_groups(sudo_uid, sudo_gid)?;
    if sudo_uid == 0
        || request.expected_uid != sudo_uid
        || !expected_group_is_authorized(sudo_gid, &supplementary_gids, request.expected_gid)
    {
        bail!("request owner does not match the non-root sudo caller");
    }

    require_root_directory(Path::new("/build"), false)?;
    require_root_directory(Path::new("/build/agentic-sandbox"), false)?;
    let vm_root = fs::symlink_metadata(VM_ROOT).context("inspect fixed VM root")?;
    if !vm_root.file_type().is_dir() || vm_root.file_type().is_symlink() {
        bail!("fixed VM root is unsafe");
    }
    let capture_root = ensure_root_only_directory(Path::new(CAPTURE_ROOT))?;
    if vm_root.dev() != capture_root.dev() {
        bail!("VM and capture roots are not on the same filesystem");
    }
    let run_path = Path::new(CAPTURE_ROOT).join(&request.run_id);
    let run_root = ensure_root_only_directory(&run_path)?;
    if run_root.dev() != capture_root.dev() {
        bail!("exact-run capture root crossed a filesystem boundary");
    }

    let expected_name = request
        .capture_path
        .file_name()
        .ok_or_else(|| anyhow!("capture path has no final component"))?;
    let tombstone = tombstone_path(&request.capture_path);
    let expected_tombstone_name = tombstone
        .file_name()
        .ok_or_else(|| anyhow!("tombstone path has no final component"))?;
    for entry in fs::read_dir(&run_path).context("enumerate exact-run capture root")? {
        let entry = entry.context("read exact-run capture candidate")?;
        let name = entry.file_name();
        if name != expected_name && name != expected_tombstone_name {
            if let Some(name) = name.to_str() {
                if safe_tombstone_name(name) {
                    verify_prior_tombstone(&entry.path())?;
                    continue;
                }
            }
            bail!("exact-run capture root contains an unknown candidate");
        }
    }

    let source = metadata_if_present(&request.source_path)?;
    let captured = metadata_if_present(&request.capture_path)?;
    let tombstone_present = metadata_if_present(&tombstone)?.is_some();
    let captured = match (source, captured, tombstone_present) {
        (Some(_), _, true) | (_, Some(_), true) => {
            bail!("helper tombstone conflicts with a live source or capture")
        }
        (Some(_), Some(_), false) => bail!("source and final capture both exist"),
        (Some(source), None, false) => {
            exact_identity(&source, request, "source quarantine")?;
            fs::rename(&request.source_path, &request.capture_path)
                .context("atomically move quarantine into root-only final capture")?;
            sync_directory(Path::new(VM_ROOT))?;
            sync_directory(&run_path)?;
            let captured = fs::symlink_metadata(&request.capture_path)
                .context("verify atomically captured final directory")?;
            exact_identity(&captured, request, "captured quarantine")?;
            captured
        }
        (None, Some(captured), false) => {
            exact_identity(&captured, request, "restart capture")?;
            captured
        }
        (None, None, true) => {
            verify_tombstone(&tombstone, request)?;
            return Ok("absent");
        }
        (None, None, false) => {
            bail!("source and final capture are absent without a helper tombstone")
        }
    };
    exact_identity(&captured, request, "root-only capture")?;
    let mut count = 0;
    verify_tree(
        &request.capture_path,
        request.expected_uid,
        request.expected_gid,
        &mut count,
        0,
    )?;
    delete_tree(&request.capture_path, 0)?;
    fs::remove_dir(&request.capture_path).context("remove exact root-only capture directory")?;
    write_tombstone(&tombstone, request)?;
    sync_directory(&run_path)?;
    Ok("deleted")
}

fn read_request() -> Result<Request> {
    if env::args_os().count() != 1 {
        bail!("helper accepts no arguments");
    }
    let mut source = String::new();
    io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_string(&mut source)
        .context("read bounded helper request")?;
    if source.is_empty() || source.len() as u64 > MAX_INPUT_BYTES {
        bail!("helper request is empty or exceeds its bound");
    }
    parse_request(&source)
}

fn main() {
    match read_request().and_then(|request| {
        let capture_path = request.capture_path.display().to_string();
        execute(&request).map(|status| (status, capture_path))
    }) {
        Ok((status, capture_path)) => println!(
            "{{\"schema_version\":\"{RESPONSE_SCHEMA}\",\"status\":\"{status}\",\"capture_path\":\"{capture_path}\"}}"
        ),
        Err(_) => {
            println!(
                "{{\"schema_version\":\"{RESPONSE_SCHEMA}\",\"status\":\"refused\",\"reason_code\":\"QEMU_CLEANUP_HELPER_REFUSED\"}}"
            );
            std::process::exit(4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(source: &str, capture: &str) -> String {
        format!(
            "{{\"schema_version\":\"{REQUEST_SCHEMA}\",\"operation\":\"{OPERATION}\",\"run_id\":\"titan-123\",\"source_path\":\"{source}\",\"capture_path\":\"{capture}\",\"expected_uid\":\"1000\",\"expected_gid\":\"1000\",\"expected_device\":\"8\",\"expected_inode\":\"9\"}}"
        )
    }

    #[test]
    fn exact_storage_group_must_be_primary_or_supplementary() {
        assert!(expected_group_is_authorized(1000, &[4, 27, 992], 1000));
        assert!(expected_group_is_authorized(1000, &[4, 27, 992], 992));
        assert!(!expected_group_is_authorized(1000, &[4, 27, 992], 991));
    }

    #[test]
    fn accepts_only_the_fixed_quarantine_and_deterministic_capture() {
        let source =
            "/build/agentic-sandbox/vms/.celld-owned.cleanup-123e4567-e89b-42d3-a456-426614174000";
        let capture = "/build/agentic-sandbox/.celld-qemu-cleanup/titan-123/.celld-owned.cleanup-123e4567-e89b-42d3-a456-426614174000.final";
        assert!(parse_request(&request(source, capture)).is_ok());
        assert!(parse_request(&request("/tmp/foreign", capture)).is_err());
        assert!(parse_request(&request(source, "/tmp/capture")).is_err());
    }

    #[test]
    fn rejects_arguments_encoded_as_unknown_or_duplicate_fields() {
        let source =
            "/build/agentic-sandbox/vms/.celld-owned.cleanup-123e4567-e89b-42d3-a456-426614174000";
        let capture = "/build/agentic-sandbox/.celld-qemu-cleanup/titan-123/.celld-owned.cleanup-123e4567-e89b-42d3-a456-426614174000.final";
        let valid = request(source, capture);
        assert!(parse_request(&valid.replace("}", ",\"argv\":\"--path\"}")).is_err());
        assert!(parse_request(&valid.replace(
            "\"operation\":\"capture-delete\"",
            "\"operation\":\"capture-delete\",\"operation\":\"capture-delete\""
        ))
        .is_err());
    }
}
