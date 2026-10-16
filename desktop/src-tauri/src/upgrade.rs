//! Installer-only preparation and one-shot restoration of running services.

use crate::{AppError, services::Services};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
};
use tauri::Manager;
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, SetEvent, TerminateProcess, WaitForSingleObject,
        },
    },
};

const MAIN: &str = "windows-mcp-manager.exe";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Resume {
    pub mcp: bool,
    pub files: bool,
}
impl Resume {
    pub fn from_args(args: &[String]) -> Option<Self> {
        args.iter().any(|v| v == "--after-upgrade").then(|| Self {
            mcp: args.iter().any(|v| v == "--restart-mcp"),
            files: args.iter().any(|v| v == "--restart-files"),
        })
    }
    pub fn restore(self, services: &Services) -> Result<(), AppError> {
        if self.mcp {
            let snapshot = services.start(
                services.snapshot().generation,
                services.ticket(),
                self.files,
            )?;
            if let Some(error) = snapshot.error {
                return Err(error);
            }
        } else if self.files {
            services.start_files(services.ticket())?;
        }
        Ok(())
    }
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}
fn event_name(executable: &Path) -> Vec<u16> {
    let digest = Sha256::digest(normalized(executable).as_bytes());
    OsStr::new(&format!("Local\\WindowsMCP.Upgrade.{digest:x}"))
        .encode_wide()
        .chain(Some(0))
        .collect()
}

/// New versions can shut down through their normal service lifecycle. Legacy
/// versions are stopped by their verified process handles below.
pub fn listen(app: tauri::AppHandle) -> Result<(), AppError> {
    let executable = std::env::current_exe().map_err(AppError::storage)?;
    let name = event_name(&executable);
    let raw = unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) };
    if raw.is_null() {
        return Err(AppError::storage(std::io::Error::last_os_error()));
    }
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    std::thread::spawn(move || {
        if unsafe { WaitForSingleObject(event.as_raw_handle(), INFINITE) } == WAIT_OBJECT_0 {
            app.state::<Services>().begin_exit();
            app.exit(0);
        }
    });
    Ok(())
}

struct Process {
    handle: OwnedHandle,
    path: PathBuf,
}
impl Process {
    fn wait(&self, milliseconds: u32) -> u32 {
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), milliseconds) }
    }
    fn stop(&self) -> Result<(), AppError> {
        if self.wait(0) == WAIT_OBJECT_0 {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.handle.as_raw_handle(), 0) } == 0 {
            let error = std::io::Error::last_os_error();
            // Closing the manager's job can already be terminating this child.
            // TerminateProcess then reports access denied before the process
            // handle is signaled; wait for that in-flight exit to complete.
            if self.wait(10_000) == WAIT_OBJECT_0 {
                return Ok(());
            }
            return Err(AppError::new("upgrade_stop_failed", error.to_string()));
        }
        if self.wait(10_000) != WAIT_OBJECT_0 {
            return Err(AppError::new(
                "upgrade_stop_failed",
                "A program process did not exit",
            ));
        }
        Ok(())
    }
}

fn installed_processes(directory: &Path) -> Result<Vec<Process>, AppError> {
    let expected = [
        directory.join(MAIN),
        directory.join("backend/windows-mcp-backend.exe"),
        directory.join("file-server/dufs.exe"),
    ]
    .map(|p| normalized(&p));
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(AppError::storage(std::io::Error::last_os_error()));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut processes = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|v| *v == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if [MAIN, "windows-mcp-backend.exe", "dufs.exe"]
            .iter()
            .any(|v| name.eq_ignore_ascii_case(v))
        {
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    0,
                    entry.th32ProcessID,
                )
            };
            if !raw.is_null() {
                let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
                let mut buffer = vec![0u16; 32768];
                let mut length = buffer.len() as u32;
                if unsafe {
                    QueryFullProcessImageNameW(
                        handle.as_raw_handle(),
                        0,
                        buffer.as_mut_ptr(),
                        &mut length,
                    )
                } != 0
                {
                    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
                    if expected.contains(&normalized(&path)) {
                        processes.push(Process { handle, path });
                    }
                }
            }
        }
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(processes)
}

fn prepare(directory: &Path, plan: &Path) -> Result<(), AppError> {
    let directory = directory.canonicalize().map_err(AppError::storage)?;
    if directory.parent().is_none()
        || !directory.join(MAIN).is_file()
        || !directory.join("uninstall.exe").is_file()
    {
        return Err(AppError::new(
            "upgrade_directory",
            "Expected an existing manager installation",
        ));
    }
    let processes = installed_processes(&directory)?;
    let resume = Resume {
        mcp: processes.iter().any(|p| {
            p.path.file_name().is_some_and(|v| {
                v.to_string_lossy()
                    .eq_ignore_ascii_case("windows-mcp-backend.exe")
            })
        }),
        files: processes.iter().any(|p| {
            p.path
                .file_name()
                .is_some_and(|v| v.to_string_lossy().eq_ignore_ascii_case("dufs.exe"))
        }),
    };
    fs::write(
        plan,
        format!(
            "[resume]\nmcp={}\nfiles={}\n",
            u8::from(resume.mcp),
            u8::from(resume.files)
        ),
    )
    .map_err(AppError::storage)?;
    let name = event_name(&directory.join(MAIN));
    let raw = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if !raw.is_null() {
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };
        unsafe {
            SetEvent(event.as_raw_handle());
        }
        for process in &processes {
            if process
                .path
                .file_name()
                .is_some_and(|v| v.to_string_lossy().eq_ignore_ascii_case(MAIN))
            {
                process.wait(15_000);
            }
        }
    }
    // Stop the owner first: closing its jobs also stops its children. Handles
    // retain process identity, so PID reuse can never target a different app.
    for manager in processes.iter().filter(|p| {
        p.path
            .file_name()
            .is_some_and(|v| v.to_string_lossy().eq_ignore_ascii_case(MAIN))
    }) {
        manager.stop()?;
    }
    for process in &processes {
        process.stop()?;
    }
    if installed_processes(&directory)?
        .iter()
        .any(|p| p.wait(0) == WAIT_TIMEOUT)
    {
        return Err(AppError::new(
            "upgrade_stop_failed",
            "The installed program restarted during upgrade",
        ));
    }
    Ok(())
}

/// Runs from the installer's private copy of this executable, before Tauri starts.
pub fn command() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|v| v != "--prepare-upgrade") {
        return None;
    }
    if args.len() != 4 {
        return Some(2);
    }
    let plan = Path::new(&args[3]);
    match prepare(Path::new(&args[2]), plan) {
        Ok(()) => Some(0),
        Err(error) => {
            let _ = fs::write(plan.with_extension("error"), error.to_string());
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
        time::Duration,
    };

    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn preparation_stops_only_this_installation_and_records_running_services() {
        let root = std::env::temp_dir().join(format!(
            "wmcp-upgrade-processes-{}",
            crate::config::generate_token().unwrap()
        ));
        let target = root.join("installed");
        let other = root.join("unrelated");
        fs::create_dir_all(target.join("backend")).unwrap();
        fs::create_dir_all(target.join("file-server")).unwrap();
        fs::create_dir_all(other.join("backend")).unwrap();
        let ping = PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32/ping.exe");
        let launch = |path: PathBuf| {
            fs::copy(&ping, &path).unwrap();
            Running(
                Command::new(path)
                    .args(["-t", "127.0.0.1"])
                    .creation_flags(0x0800_0000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        };
        let mut manager = launch(target.join(MAIN));
        let mut backend = launch(target.join("backend/windows-mcp-backend.exe"));
        let mut files = launch(target.join("file-server/dufs.exe"));
        let mut unrelated = launch(other.join("backend/windows-mcp-backend.exe"));
        fs::write(target.join("uninstall.exe"), b"fixture").unwrap();
        let original = b"configuration and token must remain untouched";
        fs::write(target.join("manager.json"), original).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(backend.0.try_wait().unwrap().is_none());
        prepare(&target, &root.join("resume.ini")).unwrap();
        assert!(manager.0.try_wait().unwrap().is_some());
        assert!(backend.0.try_wait().unwrap().is_some());
        assert!(files.0.try_wait().unwrap().is_some());
        assert!(unrelated.0.try_wait().unwrap().is_none());
        assert_eq!(
            fs::read_to_string(root.join("resume.ini")).unwrap(),
            "[resume]\nmcp=1\nfiles=1\n"
        );
        assert_eq!(fs::read(target.join("manager.json")).unwrap(), original);
        drop((manager, backend, files, unrelated));
        assert!(root.starts_with(std::env::temp_dir()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wmcp-upgrade-processes-")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resume_is_explicit_and_preserves_service_selection() {
        assert_eq!(Resume::from_args(&[]), None);
        assert_eq!(Resume::from_args(&["--restart-mcp".into()]), None);
        assert_eq!(
            Resume::from_args(&["--after-upgrade".into(), "--restart-files".into()]),
            Some(Resume {
                mcp: false,
                files: true
            })
        );
    }
    #[test]
    fn shutdown_event_is_scoped_to_the_exact_installation() {
        assert_eq!(
            event_name(Path::new(r"C:\Apps\Manager.exe")),
            event_name(Path::new(r"\\?\c:\apps\manager.exe"))
        );
        assert_ne!(
            event_name(Path::new(r"C:\A\Manager.exe")),
            event_name(Path::new(r"C:\B\Manager.exe"))
        );
    }
    #[test]
    fn preparation_rejects_a_non_installation_without_writing_a_plan() {
        let root = std::env::temp_dir().join(format!(
            "wmcp-upgrade-{}",
            crate::config::generate_token().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        assert!(prepare(&root, &root.join("resume.ini")).is_err());
        assert!(!root.join("resume.ini").exists());
        fs::remove_dir(root).unwrap();
    }
}
