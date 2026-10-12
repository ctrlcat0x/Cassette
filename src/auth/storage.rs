//! Native credential storage. Never falls back to plaintext persistence.
#[cfg(target_os = "windows")]
use anyhow::Context;
use anyhow::{Result, bail};
use std::path::Path;
use zeroize::Zeroizing;

#[cfg(target_os = "windows")]
pub fn restrict(path: &Path) -> Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT,
                SetNamedSecurityInfoW,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    // Owner and LocalSystem only; exclude inherited broad access on temporary exports.
    let sddl: Vec<u16> = if path.is_dir() {
        "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)\0"
    } else {
        "D:P(A;;FA;;;OW)(A;;FA;;;SY)\0"
    }
    .encode_utf16()
    .collect();
    let mut descriptor = ptr::null_mut();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            bail!("Could not create private session permissions");
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
            || dacl.is_null()
        {
            LocalFree(descriptor);
            bail!("Could not read private session permissions");
        }
        // Unlike SetFileSecurity, this propagates inheritable rules to existing children.
        let result = SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        );
        LocalFree(descriptor);
        if result != 0 {
            bail!("Could not protect session file permissions");
        }
    }
    Ok(())
}

/// Keep private storage out of ordinary Explorer views and content indexing.
/// Access control and credential encryption provide the actual protection.
#[cfg(target_os = "windows")]
pub fn hide_private(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
        GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let attributes = GetFileAttributesW(path.as_ptr());
        if attributes == INVALID_FILE_ATTRIBUTES
            || SetFileAttributesW(
                path.as_ptr(),
                (attributes & !FILE_ATTRIBUTE_NORMAL)
                    | FILE_ATTRIBUTE_HIDDEN
                    | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
            ) == 0
        {
            bail!("Could not hide private session storage");
        }
    }
    Ok(())
}
#[cfg(not(target_os = "windows"))]
pub fn hide_private(_: &Path) -> Result<()> {
    Ok(())
}

/// Protect new files through inheritance and repair existing private metadata.
pub fn prepare_data_dir() -> Result<()> {
    let directory = crate::library::data_dir();
    std::fs::create_dir_all(&directory)?;
    restrict(&directory)?;
    hide_private(&directory)?;
    for name in ["library.json", "session.dpapi", "pages", "lyrics"] {
        let path = directory.join(name);
        if path.exists() {
            restrict(&path)?;
            hide_private(&path)?;
            if path.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    restrict(&entry?.path())?;
                }
            }
        }
    }
    Ok(())
}

pub fn export_dir() -> Result<std::path::PathBuf> {
    let directory = crate::library::data_dir().join("session-exports");
    std::fs::create_dir_all(&directory)?;
    restrict(&directory)?;
    hide_private(&directory)?;
    Ok(directory)
}

/// Run only after acquiring the application's instance lock.
pub fn clear_abandoned_exports() -> Result<()> {
    for entry in std::fs::read_dir(export_dir()?)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
#[cfg(unix)]
pub fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn crypt(bytes: &[u8], encrypt: bool) -> Result<Zeroizing<Vec<u8>>> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    unsafe {
        let result = if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if result == 0 {
            bail!("Windows could not access the protected YouTube session");
        }
        let result = Zeroizing::new(
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec(),
        );
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}
#[cfg(target_os = "windows")]
pub fn load() -> Result<Option<Zeroizing<Vec<u8>>>> {
    let path = crate::library::data_dir().join("session.dpapi");
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        crate::resource::read_limited(std::fs::File::open(path)?, super::SESSION_LIMIT + 4096)?;
    Ok(Some(crypt(&bytes, false)?))
}
#[cfg(target_os = "windows")]
pub fn save(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let directory = crate::library::data_dir();
    std::fs::create_dir_all(&directory)?;
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    restrict(file.path())?;
    hide_private(file.path())?;
    file.write_all(&crypt(bytes, true)?)?;
    file.as_file().sync_all()?;
    file.persist(directory.join("session.dpapi"))
        .context("Could not save protected session")?;
    Ok(())
}
#[cfg(target_os = "windows")]
pub fn delete() -> Result<()> {
    match std::fs::remove_file(crate::library::data_dir().join("session.dpapi")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(target_os = "windows"))]
fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new("app.raag.desktop", "youtube-session")?)
}
#[cfg(not(target_os = "windows"))]
pub fn load() -> Result<Option<Zeroizing<Vec<u8>>>> {
    match entry()?.get_secret() {
        Ok(bytes) => {
            if bytes.len() as u64 > super::SESSION_LIMIT {
                bail!("Stored session is too large");
            }
            Ok(Some(Zeroizing::new(bytes)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
#[cfg(not(target_os = "windows"))]
pub fn save(bytes: &[u8]) -> Result<()> {
    Ok(entry()?.set_secret(bytes)?)
}
#[cfg(not(target_os = "windows"))]
pub fn delete() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
