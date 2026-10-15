use crate::AppError;
use std::{
    fs,
    io::Write,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    process::Child,
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    },
};

fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, AppError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI binds the ciphertext to the current Windows user; no UI prompts.
    let success = unsafe {
        if encrypt {
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
        }
    };
    if success == 0 {
        return Err(AppError::new(
            "credential_failed",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

pub fn protect(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    crypt(bytes, true)
}
pub fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    crypt(bytes, false)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let temporary = path.with_extension("json.tmp");
    let mut file = fs::File::create(&temporary).map_err(AppError::storage)?;
    file.write_all(bytes).map_err(AppError::storage)?;
    file.sync_all().map_err(AppError::storage)?;
    drop(file);
    let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(AppError::storage(std::io::Error::last_os_error()));
    }
    Ok(())
}

pub fn rename_new(source: &Path, target: &Path) -> Result<(), AppError> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // No REPLACE_EXISTING: a late destination collision must never overwrite user data.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(AppError::new(
            "transfer_conflict",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

pub struct Job(OwnedHandle);

impl Job {
    pub fn new() -> Result<Self, AppError> {
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(AppError::new(
                "process_failed",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let result = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if result == 0 {
            return Err(AppError::new(
                "process_failed",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(Self(handle))
    }

    pub fn assign(&self, child: &Child) -> Result<(), AppError> {
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(AppError::new(
                "process_failed",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }
}
