//! Test component that calls every grant-controlled operation once, and
//! probes the runtime's memory and message limits, then prints one
//! `PROBE <name> ok <detail>` or `PROBE <name> err <message>` line for each.
//!
//! Fetch targets are read from `/probe/targets.txt` (`<name> <url>` per
//! line), a directory the harness preopens read-only next to the grants
//! under test. The target named `fetch-ok` is also requested with a forged
//! `Host` header and with a method carrying a second request line.

wit_bindgen::generate!({
    path: "wit",
    world: "app",
    generate_all,
});

use weft::app::{clipboard, fetch, ipc, notifications, notify};

/// An allocation larger than the runtime's default memory limit.
const MEMORY_PROBE: usize = 300 << 20;

fn report<T: std::fmt::Debug, E: std::fmt::Display>(name: &str, result: Result<T, E>) {
    match result {
        Ok(value) => println!("PROBE {name} ok {value:?}"),
        Err(e) => println!("PROBE {name} err {}", e.to_string().replace('\n', " ")),
    }
}

fn main() {
    report("data-read", std::fs::read_to_string("/data/seed.txt"));
    report("data-write", std::fs::write("/data/written.txt", "probe"));
    report("data-overwrite", std::fs::write("/data/seed.txt", "changed"));
    report("data-mkdir", std::fs::create_dir("/data/made"));
    report("data-rename", std::fs::rename("/data/seed.txt", "/data/moved.txt"));
    report("data-remove", std::fs::remove_file("/data/seed.txt"));
    report("notifications", notifications::notify("probe", "probe", None));
    report("clipboard-read", clipboard::read());
    report("clipboard-write", clipboard::write("probe"));

    let targets = std::fs::read_to_string("/probe/targets.txt").unwrap_or_default();
    for (name, url) in targets.lines().filter_map(|line| line.split_once(' ')) {
        report(name, fetch::fetch(url, "GET", &[], None).map(|r| r.status));
        if name == "fetch-ok" {
            let forged = [("Host".to_owned(), "localhost".to_owned())];
            report(
                "fetch-host-header",
                fetch::fetch(url, "GET", &forged, None).map(|r| r.status),
            );
            report(
                "fetch-bad-method",
                fetch::fetch(url, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\nGET", &[], None)
                    .map(|r| r.status),
            );
        }
    }
    let mut big: Vec<u8> = Vec::new();
    report(
        "memory-grow",
        big.try_reserve(MEMORY_PROBE).map(|()| big.capacity()),
    );
    drop(big);
    report("ipc-newline", ipc::send("two\nmessages"));
    report("ipc-oversize", ipc::send(&"x".repeat(64 * 1024 + 1)));
    report(
        "fetch-file-scheme",
        fetch::fetch("file:///etc/hostname", "GET", &[], None).map(|r| r.status),
    );
    notify::ready();
}
