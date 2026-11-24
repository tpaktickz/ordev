//! Portable registry surface.
//!
//! Windows implements this against the real Win32 registry. Linux has no
//! registry, so every read answers `false`/empty and every write is a silent
//! no-op — business code above stays branch-free.
//!
//! The higher-level helpers (`backup_matching_values`, `restore_backup_values`,
//! `patch_first_byte_of_matching_values`) exist so the sandbox logic stays
//! readable instead of drowning in `RegEnumValueW` plumbing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegRoot {
    Machine,
    User,
}

/// A registry value as raw bytes plus its type tag.
#[derive(Clone, Debug)]
pub struct RegValue {
    pub name: String,
    pub ty: u32,
    pub data: Vec<u8>,
}

/// Prefix applied to values the sandbox backs up.
pub const BACKUP_PREFIX: &str = "OpenRevoBackup_";

#[cfg(windows)]
mod imp {
    use super::{RegRoot, RegValue, BACKUP_PREFIX};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
        REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW,
        RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, REG_OPTION_NON_VOLATILE,
    };

    fn root_hkey(root: RegRoot) -> HKEY {
        match root {
            RegRoot::Machine => HKEY_LOCAL_MACHINE,
            RegRoot::User => HKEY_CURRENT_USER,
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        format!("{s}\0").encode_utf16().collect()
    }

    fn open(root: RegRoot, subkey: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
        let sub = wide(subkey);
        let mut hkey = HKEY::default();
        let rc = unsafe { RegOpenKeyExW(root_hkey(root), PCWSTR(sub.as_ptr()), 0, access, &mut hkey) };
        if rc == ERROR_SUCCESS { Some(hkey) } else { None }
    }

    fn close(hkey: HKEY) {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
    }

    pub fn key_exists(root: RegRoot, subkey: &str) -> bool {
        match open(root, subkey, KEY_READ) {
            Some(hkey) => {
                close(hkey);
                true
            }
            None => false,
        }
    }

    pub fn set_dword(root: RegRoot, subkey: &str, name: &str, val: u32) {
        set_value_raw(root, subkey, name, REG_DWORD.0, &val.to_le_bytes());
    }

    /// Like [`set_dword`], but creates the subkey first when missing.
    pub fn set_dword_or_create(root: RegRoot, subkey: &str, name: &str, val: u32) {
        let sub = wide(subkey);
        let mut hkey = HKEY::default();
        let mut disposition = 0u32;
        let rc = unsafe {
            RegCreateKeyExW(
                root_hkey(root),
                PCWSTR(sub.as_ptr()),
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                Some(&mut disposition),
            )
        };
        if rc != ERROR_SUCCESS {
            return;
        }
        let name_w = wide(name);
        let bytes = val.to_le_bytes();
        unsafe {
            let _ = RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), 0, REG_DWORD, Some(&bytes));
        }
        close(hkey);
    }

    pub fn set_value_raw(root: RegRoot, subkey: &str, name: &str, ty: u32, data: &[u8]) {
        let Some(hkey) = open(root, subkey, KEY_WRITE) else {
            return;
        };
        let name_w = wide(name);
        unsafe {
            let _ = RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), 0, REG_VALUE_TYPE(ty), Some(data));
        }
        close(hkey);
    }

    /// Write a REG_SZ value (UTF-16LE), creating the subkey when missing.
    pub fn set_string(root: RegRoot, subkey: &str, name: &str, val: &str) {
        let sub = wide(subkey);
        let mut hkey = HKEY::default();
        let rc = unsafe {
            RegCreateKeyExW(
                root_hkey(root),
                PCWSTR(sub.as_ptr()),
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return;
        }
        let name_w = wide(name);
        let data_w = wide(val);
        let bytes =
            unsafe { std::slice::from_raw_parts(data_w.as_ptr() as *const u8, data_w.len() * 2) };
        unsafe {
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                0,
                windows::Win32::System::Registry::REG_SZ,
                Some(bytes),
            );
        }
        close(hkey);
    }

    pub fn delete_value(root: RegRoot, subkey: &str, name: &str) {
        let Some(hkey) = open(root, subkey, KEY_WRITE) else {
            return;
        };
        let name_w = wide(name);
        unsafe {
            let _ = RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr()));
        }
        close(hkey);
    }

    /// Single-value read with an explicit expected type; `None` on any mismatch.
    pub fn read_dword(root: RegRoot, subkey: &str, name: &str) -> Option<u32> {
        let raw = read_value(root, subkey, name)?;
        if raw.ty != REG_DWORD || raw.data.len() < 4 {
            return None;
        }
        Some(u32::from_ne_bytes([raw.data[0], raw.data[1], raw.data[2], raw.data[3]]))
    }

    pub fn read_string(root: RegRoot, subkey: &str, name: &str) -> Option<String> {
        let raw = read_value(root, subkey, name)?;
        if raw.ty != REG_SZ && raw.ty != REG_EXPAND_SZ {
            return None;
        }
        let units: Vec<u16> = raw
            .data
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .take_while(|c| *c != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn read_value(root: RegRoot, subkey: &str, name: &str) -> Option<RegValue> {
        let hkey = open(root, subkey, KEY_READ)?;
        let name_w = wide(name);
        let mut ty = 0u32;
        let mut data_buf = [0u8; 4096];
        let mut data_len = data_buf.len() as u32;
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut ty),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        close(hkey);
        if rc != ERROR_SUCCESS {
            return None;
        }
        Some(RegValue {
            name: name.to_string(),
            ty,
            data: data_buf[..(data_len as usize).min(data_buf.len())].to_vec(),
        })
    }

    pub fn enum_values(root: RegRoot, subkey: &str) -> Vec<RegValue> {
        let Some(hkey) = open(root, subkey, KEY_READ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 512];
            let mut name_len = name_buf.len() as u32;
            let mut ty = 0u32;
            let mut data_buf = [0u8; 4096];
            let mut data_len = data_buf.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    hkey,
                    index,
                    PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    Some(&mut ty),
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            out.push(RegValue {
                name: String::from_utf16_lossy(&name_buf[..name_len as usize]),
                ty,
                data: data_buf[..data_len as usize].to_vec(),
            });
            index += 1;
        }
        close(hkey);
        out
    }

    pub fn enum_subkeys(root: RegRoot, subkey: &str) -> Vec<String> {
        let Some(hkey) = open(root, subkey, KEY_READ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    hkey,
                    index,
                    PWSTR(buf.as_mut_ptr()),
                    &mut len,
                    None,
                    PWSTR::null(),
                    None,
                    None,
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
            index += 1;
        }
        close(hkey);
        out
    }

    /// Rename every value whose name matches one of `patterns` to
    /// `OpenRevoBackup_<name>`, preserving type and data. Returns how many moved.
    pub fn backup_matching_values(root: RegRoot, subkey: &str, patterns: &[&str]) -> usize {
        let mut moved = 0;
        for value in enum_values(root, subkey) {
            if value.name.starts_with(BACKUP_PREFIX) {
                continue;
            }
            let lower = value.name.to_lowercase();
            if !patterns.iter().any(|p| lower.contains(p)) {
                continue;
            }
            let backup_name = format!("{BACKUP_PREFIX}{}", value.name);
            set_value_raw(root, subkey, &backup_name, value.ty, &value.data);
            delete_value(root, subkey, &value.name);
            moved += 1;
        }
        moved
    }

    /// Undo [`backup_matching_values`]: restore the original names and drop the
    /// backup copies.
    pub fn restore_backup_values(root: RegRoot, subkey: &str) -> usize {
        let mut restored = 0;
        for value in enum_values(root, subkey) {
            let Some(orig) = value.name.strip_prefix(BACKUP_PREFIX) else {
                continue;
            };
            set_value_raw(root, subkey, orig, value.ty, &value.data);
            delete_value(root, subkey, &value.name);
            restored += 1;
        }
        restored
    }

    /// Overwrite byte 0 of every matching value that has at least one byte and
    /// differs. Explains itself: StartupApproved flags live in byte 0.
    pub fn patch_first_byte_of_matching_values(
        root: RegRoot,
        subkey: &str,
        patterns: &[&str],
        byte: u8,
    ) -> usize {
        let mut patched = 0;
        for value in enum_values(root, subkey) {
            let lower = value.name.to_lowercase();
            if !patterns.iter().any(|p| lower.contains(p)) {
                continue;
            }
            if value.data.first() == Some(&byte) || value.data.is_empty() {
                continue;
            }
            let mut data = value.data.clone();
            data[0] = byte;
            set_value_raw(root, subkey, &value.name, value.ty, &data);
            patched += 1;
        }
        patched
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{RegRoot, RegValue};

    pub fn read_dword(_root: RegRoot, _subkey: &str, _name: &str) -> Option<u32> {
        None
    }

    pub fn read_string(_root: RegRoot, _subkey: &str, _name: &str) -> Option<String> {
        None
    }

    pub fn key_exists(_root: RegRoot, _subkey: &str) -> bool {
        false
    }
    pub fn set_dword(_root: RegRoot, _subkey: &str, _name: &str, _val: u32) {}
    pub fn set_dword_or_create(_root: RegRoot, _subkey: &str, _name: &str, _val: u32) {}
    pub fn set_value_raw(_root: RegRoot, _subkey: &str, _name: &str, _ty: u32, _data: &[u8]) {}
    pub fn set_string(_root: RegRoot, _subkey: &str, _name: &str, _val: &str) {}
    pub fn delete_value(_root: RegRoot, _subkey: &str, _name: &str) {}
    pub fn enum_values(_root: RegRoot, _subkey: &str) -> Vec<RegValue> {
        Vec::new()
    }
    pub fn enum_subkeys(_root: RegRoot, _subkey: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn backup_matching_values(_root: RegRoot, _subkey: &str, _patterns: &[&str]) -> usize {
        0
    }
    pub fn restore_backup_values(_root: RegRoot, _subkey: &str) -> usize {
        0
    }
    pub fn patch_first_byte_of_matching_values(
        _root: RegRoot,
        _subkey: &str,
        _patterns: &[&str],
        _byte: u8,
    ) -> usize {
        0
    }
}

pub use imp::*;
