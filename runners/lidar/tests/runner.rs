//! The `qualia-lidar` process contract that does not need a device: the exit
//! codes and the lines it prints when it cannot attach or cannot open a port.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use qualia_shm::ShmRegion;

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

#[test]
fn a_missing_region_exits_one_and_says_which_region() {
    let name = format!(
        "/qualia_lidar_missing_{}_{}",
        std::process::id(),
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_qualia-lidar"))
        .env("QUALIA_SHM_NAME", &name)
        .output()
        .expect("run qualia-lidar");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("qualia-lidar: failed to open shm '{name}'")),
        "{stderr}"
    );
}

#[test]
fn an_unopenable_port_exits_two_after_attaching() {
    let name = format!(
        "/qualia_lidar_port_{}_{}",
        std::process::id(),
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );
    let shm = ShmRegion::create(&name).expect("create region");

    let output = Command::new(env!("CARGO_BIN_EXE_qualia-lidar"))
        .env("QUALIA_SHM_NAME", &name)
        .env("QUALIA_LIDAR_PORT", "/dev/qualia-no-such-port")
        .env("QUALIA_LIDAR_BAUD", "115200")
        .output()
        .expect("run qualia-lidar");

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("qualia-lidar: opening serial port /dev/qualia-no-such-port @ 115200"),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("qualia-lidar: open failed:"), "{stderr}");

    drop(shm);
}
