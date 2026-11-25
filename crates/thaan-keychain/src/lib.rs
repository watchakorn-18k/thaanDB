//! Connection secrets in the OS keychain (macOS Keychain, Windows Credential Manager,
//! Secret Service on Linux), shared by the desktop app and the CLI / `thaan mcp`. One entry per connection and parameter, under the service
//! `dev.thaandb.app` and the account `<connection id>/<parameter>`.
//!
//! macOS: entries are read and written through Apple's own `/usr/bin/security` tool, not
//! by this app directly. The Keychain trusts an entry's creator per code signature;
//! `security` is signed by Apple and never changes, so access does not break (and no
//! "wants to use your confidential information" prompt appears) when this app is rebuilt
//! or updated. The trade-off: any program running as the same user can read these
//! entries through that tool without a prompt. Secrets never go on a command line (other
//! users could see it in `ps`): writes are fed to `security -i` on stdin.

use thaan_core::secrets::SecretStore;

const SERVICE: &str = "dev.thaandb.app";

pub struct Keychain;

fn account(connection_id: &str, key: &str) -> String {
    format!("{connection_id}/{key}")
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    const TOOL: &str = "/usr/bin/security";

    /// A double-quoted argument for `security -i` (backslash escapes `"` and `\`).
    fn quoted(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// `security -w` prints a password as hex when it has bytes outside printable ASCII
    /// (Thai, emoji…). Hex output always decodes to such bytes, and a printable password
    /// is printed as is, so the two never collide.
    fn decode(out: &str) -> String {
        let hex = out.len().is_multiple_of(2)
            && !out.is_empty()
            && out.bytes().all(|b| b.is_ascii_hexdigit());
        if hex {
            let bytes: Option<Vec<u8>> = (0..out.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&out[i..i + 2], 16).ok())
                .collect();
            if let Some(bytes) = bytes {
                if bytes.iter().any(|b| !(0x20..0x7f).contains(b)) {
                    if let Ok(s) = String::from_utf8(bytes) {
                        return s;
                    }
                }
            }
        }
        out.to_string()
    }

    pub(super) fn get(account: &str) -> Option<String> {
        let out = Command::new(TOOL)
            .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        Some(decode(text.strip_suffix('\n').unwrap_or(&text)))
    }

    pub(super) fn set(account: &str, value: &str) -> Result<(), String> {
        if value.contains(['\n', '\r']) {
            return Err("passwords with line breaks are not supported".into());
        }
        // An entry another program created (an older build of this app) cannot be
        // updated by `security` without a prompt; replacing it can.
        delete(account);
        let line = format!(
            "add-generic-password -U -s {} -a {} -w {}\n",
            quoted(SERVICE),
            quoted(account),
            quoted(value)
        );
        let mut child = Command::new(TOOL)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("no stdin")?
            .write_all(line.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // `security -i` exits 0 even when a command fails; its error text is the signal.
        if out.status.success() && err.is_empty() {
            Ok(())
        } else {
            Err(if err.is_empty() {
                "could not save the password".into()
            } else {
                err
            })
        }
    }

    pub(super) fn delete(account: &str) {
        Command::new(TOOL)
            .args(["delete-generic-password", "-s", SERVICE, "-a", account])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn decodes_hex_only_for_non_ascii() {
            assert_eq!(decode("plain pw"), "plain pw");
            // A printable password made of hex digits stays as typed.
            assert_eq!(decode("616263"), "616263");
            assert_eq!(decode("e0b984e0b897e0b8a2"), "ไทย");
        }

        #[test]
        fn quotes_for_security_i() {
            assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    fn entry(account: &str) -> keyring::Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, account)
    }

    pub(super) fn get(account: &str) -> Option<String> {
        entry(account).ok()?.get_password().ok()
    }

    pub(super) fn set(account: &str, value: &str) -> Result<(), String> {
        entry(account)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    pub(super) fn delete(account: &str) {
        if let Ok(e) = entry(account) {
            e.delete_credential().ok();
        }
    }
}

impl SecretStore for Keychain {
    fn get(&self, connection_id: &str, key: &str) -> Option<String> {
        imp::get(&account(connection_id, key))
    }

    fn set(&self, connection_id: &str, key: &str, value: &str) -> thaan_core::Result<()> {
        imp::set(&account(connection_id, key), value).map_err(thaan_core::Error::Secret)
    }

    fn delete(&self, connection_id: &str, key: &str) {
        imp::delete(&account(connection_id, key))
    }
}
