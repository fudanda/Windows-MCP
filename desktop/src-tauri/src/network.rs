use crate::AppError;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
    net::{Ipv4Addr, Ipv6Addr},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_SUPPORTED, INVALID_HANDLE_VALUE, NO_ERROR},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    },
    Networking::WinSock::{AF_INET, AF_INET6},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
};

#[derive(Clone, Serialize)]
pub struct Address {
    pub name: String,
    pub ip: String,
}

pub fn addresses() -> Result<Vec<Address>, AppError> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .map_err(|e| AppError::new("network_check_failed", e.to_string()))?
        .into_iter()
        .filter(|interface| interface.oper_status == if_addrs::IfOperStatus::Up)
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() && !ip.is_loopback() => Some(Address {
                name: interface.name,
                ip: ip.to_string(),
            }),
            _ => None,
        })
        .collect();
    addresses.sort_by(|a, b| a.ip.cmp(&b.ip));
    addresses.dedup_by(|a, b| a.ip == b.ip);
    Ok(addresses)
}

struct Listener {
    address: String,
    pid: u32,
}

fn table(family: u32) -> Result<Vec<u32>, AppError> {
    // DWORD storage keeps the native rows aligned. The table may grow between calls.
    let mut buffer: Vec<u32> = Vec::new();
    for _ in 0..3 {
        let mut bytes = (buffer.len() * size_of::<u32>()) as u32;
        let status = unsafe {
            GetExtendedTcpTable(
                if buffer.is_empty() {
                    ptr::null_mut()
                } else {
                    buffer.as_mut_ptr().cast()
                },
                &mut bytes,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            buffer.resize((bytes as usize).div_ceil(size_of::<u32>()), 0);
            continue;
        }
        if status == ERROR_NOT_SUPPORTED && family == AF_INET6 as u32 {
            return Ok(vec![0]);
        }
        if status != NO_ERROR {
            return Err(AppError::new(
                "network_check_failed",
                format!("GetExtendedTcpTable: {status}"),
            ));
        }
        buffer.truncate((bytes as usize).div_ceil(size_of::<u32>()));
        return Ok(buffer);
    }
    Err(AppError::new(
        "network_check_failed",
        "TCP listener table changed repeatedly",
    ))
}

fn listeners(port: u16) -> Result<Vec<Listener>, AppError> {
    let mut result = Vec::new();
    for family in [AF_INET as u32, AF_INET6 as u32] {
        let buffer = table(family)?;
        let count = buffer.first().copied().unwrap_or(0) as usize;
        let row_size = if family == AF_INET as u32 {
            size_of::<MIB_TCPROW_OWNER_PID>()
        } else {
            size_of::<MIB_TCP6ROW_OWNER_PID>()
        };
        if count > buffer.len().saturating_sub(1) * size_of::<u32>() / row_size {
            return Err(AppError::new(
                "network_check_failed",
                "Invalid TCP listener table",
            ));
        }
        for index in 0..count {
            // The bounds above cover the table header and every complete native row.
            let row = unsafe {
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(size_of::<u32>() + index * row_size)
            };
            let listener = if family == AF_INET as u32 {
                let row = unsafe { ptr::read_unaligned(row.cast::<MIB_TCPROW_OWNER_PID>()) };
                if u16::from_be(row.dwLocalPort as u16) != port {
                    continue;
                }
                Listener {
                    address: format!("{}:{port}", Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes())),
                    pid: row.dwOwningPid,
                }
            } else {
                let row = unsafe { ptr::read_unaligned(row.cast::<MIB_TCP6ROW_OWNER_PID>()) };
                if u16::from_be(row.dwLocalPort as u16) != port {
                    continue;
                }
                let ip = Ipv6Addr::from(row.ucLocalAddr);
                Listener {
                    address: if row.dwLocalScopeId == 0 {
                        format!("[{ip}]:{port}")
                    } else {
                        format!("[{ip}%{}]:{port}", row.dwLocalScopeId)
                    },
                    pid: row.dwOwningPid,
                }
            };
            result.push(listener);
        }
    }
    result.sort_by(|a, b| (&a.address, a.pid).cmp(&(&b.address, b.pid)));
    Ok(result)
}

struct Process {
    parent: u32,
    name: String,
}

fn processes() -> Result<HashMap<u32, Process>, AppError> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(AppError::new(
            "network_check_failed",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut valid = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    let mut result = HashMap::new();
    while valid != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        result.insert(
            entry.th32ProcessID,
            Process {
                parent: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..len]),
            },
        );
        valid = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    Ok(result)
}

fn image(pid: u32) -> Option<String> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut buffer = vec![0u16; 32768];
    let mut len = buffer.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(handle.as_raw_handle(), 0, buffer.as_mut_ptr(), &mut len)
    } == 0
    {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn ancestors(pid: u32, processes: &HashMap<u32, Process>) -> Vec<u32> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut current = pid;
    while current != 0 && seen.insert(current) && result.len() < 64 {
        result.push(current);
        let Some(process) = processes.get(&current) else {
            break;
        };
        current = process.parent;
    }
    result
}

/// Inspect both address families: a wildcard bind alone can miss an older
/// Windows listener bound to a specific LAN IP. Never stop foreign processes.
pub fn check_port(port: u16, managed_pid: Option<u32>) -> Result<(), AppError> {
    let listeners = listeners(port)?;
    if listeners.is_empty() {
        return if managed_pid.is_some() {
            Err(AppError::new(
                "connection_failed",
                format!("No TCP listener on port {port}"),
            ))
        } else {
            Ok(())
        };
    }
    let processes = processes()?;
    let mut conflicts = Vec::new();
    let mut legacy = false;
    for listener in listeners {
        let chain = ancestors(listener.pid, &processes);
        if managed_pid.is_some_and(|pid| chain.contains(&pid)) {
            continue;
        }
        let process = processes.get(&listener.pid);
        let path = image(listener.pid);
        let is_mcp = chain.iter().take(6).any(|pid| {
            image(*pid).is_some_and(|path| {
                let path = path.to_ascii_lowercase().replace('\\', "/");
                let name = path.rsplit('/').next().unwrap_or("");
                name.starts_with("windows-mcp")
                    || name.starts_with("windows_mcp")
                    || path.contains("/uv/tools/windows-mcp/")
                    || path.ends_with("/windows-mcp/.venv/scripts/python.exe")
            })
        });
        legacy |= is_mcp;
        conflicts.push(format!(
            "{} | PID {} | {} | {}{}",
            listener.address,
            listener.pid,
            process.map(|p| p.name.as_str()).unwrap_or("unknown"),
            path.as_deref().unwrap_or("unknown"),
            if is_mcp { " [Windows-MCP]" } else { "" }
        ));
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        if legacy {
            "legacy_instance"
        } else {
            "port_in_use"
        },
        conflicts.join("\n"),
    ))
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
