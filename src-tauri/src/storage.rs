use crate::model::{Persisted, VERSION};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn new(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        Ok(Self {
            path: directory.join("state.bin"),
        })
    }
    pub fn load(&self) -> Result<Persisted> {
        if !self.path.exists() {
            let state = Persisted::new()?;
            self.save(&state)?;
            return Ok(state);
        }
        let bytes = fs::read(&self.path)?;
        ensure!(bytes.len() <= 32 * 1024 * 1024, "Local state is too large");
        let state: Persisted = serde_json::from_slice(&unprotect(&bytes)?)
            .context("Cannot read protected state; original data has been preserved")?;
        ensure!(state.version == VERSION, "Unsupported local state version");
        state.key()?;
        ensure!(state.contacts.len() <= 256, "Too many cached contacts");
        for workspace in state.workspaces.values() {
            workspace.snapshot.verify()?;
        }
        Ok(state)
    }
    pub fn save(&self, state: &Persisted) -> Result<()> {
        let bytes = protect(&serde_json::to_vec(state)?)?;
        let temporary = self.path.with_extension("new");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, &self.path)
    }
}

#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, true)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, false)
}
#[cfg(windows)]
fn dpapi(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI is user-scoped: never use CRYPTPROTECT_LOCAL_MACHINE.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as *mut _);
        Ok(result)
    }
}
#[cfg(not(windows))]
fn protect(_: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("Dump v0.1 storage requires Windows DPAPI")
}
#[cfg(not(windows))]
fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("Dump v0.1 storage requires Windows DPAPI")
}

pub fn replace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let src: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<_> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe {
            MoveFileExW(
                src.as_ptr(),
                dst.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)?;
        Ok(())
    }
}

pub fn commit_download(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
        let src: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<_> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(source, destination)?;
        fs::remove_file(source)?;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn protected_persistence_and_no_overwrite() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path())?;
        let state = store.load()?;
        assert!(!String::from_utf8_lossy(&fs::read(&store.path)?).contains(&state.identity));
        assert_eq!(store.load()?.identity, state.identity);
        let part = dir.path().join("x.part");
        let final_path = dir.path().join("x.txt");
        fs::write(&part, b"new")?;
        fs::write(&final_path, b"old")?;
        assert!(commit_download(&part, &final_path).is_err());
        assert_eq!(fs::read(&final_path)?, b"old");
        Ok(())
    }
}
