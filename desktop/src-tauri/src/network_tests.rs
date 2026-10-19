use super::*;
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};

#[test]
fn unused_port_is_available() {
    // HP-01: select an ephemeral port without depending on machine services.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    check_port(port, None).unwrap();
}

#[test]
fn specific_ipv4_listener_reports_owner_without_terminating_it() {
    // ERR-01, HP-02, ERR-02: a specific-address listener can shadow a wildcard
    // backend on Windows; a bind-only check is therefore insufficient.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let error = check_port(address.port(), None).unwrap_err();
    assert_eq!(error.code, "port_in_use");
    assert!(error.detail.contains("127.0.0.1"), "{}", error.detail);
    assert!(error.detail.contains(&pid.to_string()), "{}", error.detail);
    check_port(address.port(), Some(pid)).unwrap();
    let other_pid = pid.checked_add(1).unwrap_or(pid - 1);
    let error = check_port(address.port(), Some(other_pid)).unwrap_err();
    assert_eq!(error.code, "port_in_use");
    // Diagnosis must never stop an unrelated service.
    TcpStream::connect(address).unwrap();
}

#[test]
fn ipv6_listener_is_detected_when_ipv6_is_available() {
    // EDGE-01: IPv6 may be disabled by the host, so skip only bind failure.
    let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else {
        return;
    };
    let address = listener.local_addr().unwrap();
    let error = check_port(address.port(), None).unwrap_err();
    assert_eq!(error.code, "port_in_use");
    assert!(error.detail.contains(&std::process::id().to_string()));
    check_port(address.port(), Some(std::process::id())).unwrap();
    TcpStream::connect(address).unwrap();
}
