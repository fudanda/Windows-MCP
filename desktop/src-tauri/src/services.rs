use crate::{
    AppError,
    manager::{Manager, Snapshot},
    transfer::TransferManager,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Control {
    gate: Mutex<()>,
    epoch: AtomicU64,
    stops: AtomicUsize,
    exiting: AtomicBool,
}

#[derive(Clone)]
pub struct Services {
    manager: Manager,
    transfer: TransferManager,
    control: Arc<Control>,
}

impl Services {
    pub fn new(manager: Manager, transfer: TransferManager) -> Self {
        Self {
            manager,
            transfer,
            control: Arc::new(Control::default()),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.manager.snapshot()
    }
    pub fn ticket(&self) -> u64 {
        self.control.epoch.load(Ordering::SeqCst)
    }
    fn current(&self, ticket: u64) -> bool {
        self.ticket() == ticket && self.control.stops.load(Ordering::SeqCst) == 0 && !self.exiting()
    }
    pub fn exiting(&self) -> bool {
        self.control.exiting.load(Ordering::SeqCst)
    }
    pub fn begin_exit(&self) {
        self.control.exiting.store(true, Ordering::SeqCst);
    }
    pub fn start(&self, generation: u64, ticket: u64, files: bool) -> Result<Snapshot, AppError> {
        let _gate = self.control.gate.lock().unwrap_or_else(|e| e.into_inner());
        let mut snapshot = self.snapshot();
        // A stop invalidates requests already queued by either window or tray.
        if !self.current(ticket) || snapshot.generation != generation {
            return Ok(snapshot);
        }
        if snapshot.phase != "running" {
            snapshot = self.manager.start_if_current(generation)?;
        }
        if !self.current(ticket) || snapshot.phase != "running" {
            return Ok(self.snapshot());
        }
        if files {
            self.start_files_inner().map_err(|e| {
                AppError::new("transfer_start_failed", format!("{}: {}", e.code, e.detail))
            })?;
        }
        Ok(self.snapshot())
    }
    pub fn request_stop(&self) -> StopRequest {
        self.control.stops.fetch_add(1, Ordering::SeqCst);
        self.control.epoch.fetch_add(1, Ordering::SeqCst);
        StopRequest {
            services: self.clone(),
        }
    }
    pub fn start_files(&self, ticket: u64) -> Result<crate::transfer::TransferSnapshot, AppError> {
        let _gate = self.control.gate.lock().unwrap_or_else(|e| e.into_inner());
        if !self.current(ticket) {
            return Ok(self.transfer.snapshot());
        }
        self.start_files_inner()
    }
    fn start_files_inner(&self) -> Result<crate::transfer::TransferSnapshot, AppError> {
        // Reap a previously exited Dufs process before treating it as running.
        self.transfer.snapshot();
        let snapshot = self.transfer.start()?;
        if !snapshot.running {
            return Err(AppError::new(
                "transfer_engine_failed",
                "Dufs exited during startup",
            ));
        }
        Ok(snapshot)
    }
    pub fn shutdown(&self) {
        self.begin_exit();
        self.request_stop().run(true);
        self.transfer.shutdown();
    }
}

pub struct StopRequest {
    services: Services,
}
impl StopRequest {
    pub fn run(self, files: bool) -> Snapshot {
        // Interrupt a blocked MCP startup before waiting for its operation gate.
        self.services.manager.stop();
        let _gate = self
            .services
            .control
            .gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if files {
            self.services.transfer.stop();
        }
        self.services.snapshot()
    }
}
impl Drop for StopRequest {
    fn drop(&mut self) {
        self.services.control.stops.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, transfer::SettingsPatch};
    use std::{fs, net::TcpListener, path::PathBuf, thread, time::Duration};

    fn fixture() -> (Services, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "wmcp-services-test-{}",
            config::generate_token().unwrap()
        ));
        let manager = Manager::new(root.join("mcp"), root.clone()).unwrap();
        let transfer = TransferManager::new(
            root.join("files"),
            root.join("inbox"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        (Services::new(manager, transfer), root)
    }
    fn cleanup(services: Services, root: PathBuf) {
        services.shutdown();
        drop(services);
        assert!(root.starts_with(std::env::temp_dir()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wmcp-services-test-")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_invalidates_queued_starts_and_exit_blocks_future_starts() {
        let (services, root) = fixture();
        let ticket = services.ticket();
        let request = services.request_stop();
        assert!(!services.current(ticket));
        assert!(!services.current(services.ticket()));
        assert_eq!(request.run(true).phase, "stopped");
        let snapshot = services
            .start(services.snapshot().generation, ticket, true)
            .unwrap();
        assert_eq!(snapshot.phase, "stopped");
        assert!(!services.transfer.snapshot().running);
        assert!(!services.start_files(ticket).unwrap().running);
        services.begin_exit();
        assert!(!services.current(services.ticket()));
        cleanup(services, root);
    }
    #[test]
    fn concurrent_stop_requests_keep_starts_blocked_until_all_finish() {
        let (services, root) = fixture();
        let first = services.request_stop();
        let second = services.request_stop();
        first.run(true);
        assert!(!services.current(services.ticket()));
        second.run(true);
        assert!(services.current(services.ticket()));
        cleanup(services, root);
    }
    #[test]
    fn stop_drains_a_file_start_already_inside_the_gate() {
        let (services, root) = fixture();
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        services
            .transfer
            .configure(SettingsPatch {
                port: Some(port),
                lan_enabled: Some(false),
                ..Default::default()
            })
            .unwrap();
        let gate = services.control.gate.lock().unwrap();
        let request = services.request_stop();
        let stopping = thread::spawn(move || request.run(true));
        drop(reservation);
        services.transfer.start().unwrap();
        assert!(services.transfer.snapshot().running);
        drop(gate);
        assert_eq!(stopping.join().unwrap().phase, "stopped");
        assert!(!services.transfer.snapshot().running);
        // Gateway shutdown may finish just after the last handle is dropped.
        for _ in 0..20 {
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
        cleanup(services, root);
    }
    #[test]
    fn mcp_start_failure_does_not_start_file_service() {
        let (services, root) = fixture();
        let occupied = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let mut config = services.snapshot().config;
        config.port = occupied.local_addr().unwrap().port();
        let saved = services.manager.save(config).unwrap();
        assert_eq!(
            services
                .start(saved.generation, services.ticket(), true)
                .err()
                .unwrap()
                .code,
            "port_in_use"
        );
        assert!(!services.transfer.snapshot().running);
        cleanup(services, root);
    }
}
