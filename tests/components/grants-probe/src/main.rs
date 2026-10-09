//! Test component that calls every grant-controlled operation once and
//! prints one `PROBE <name> ok <detail>` or `PROBE <name> err <message>`
//! line for each.
//!
//! Fetch targets are read from `/probe/targets.txt` (`<name> <url>` per
//! line), a directory the harness preopens read-only next to the grants
//! under test.

wit_bindgen::generate!({
    path: "wit",
    world: "app",
    generate_all,
});

use weft::app::{clipboard, fetch, notifications, notify};

fn report<T: std::fmt::Debug, E: std::fmt::Display>(name: &str, result: Result<T, E>) {
    match result {
        Ok(value) => println!("PROBE {name} ok {value:?}"),
        Err(e) => println!("PROBE {name} err {}", e.to_string().replace('\n', " ")),
    }
}

fn main() {
    report("data-read", std::fs::read_to_string("/data/seed.txt"));
    report("data-write", std::fs::write("/data/written.txt", "probe"));
    report("notifications", notifications::notify("probe", "probe", None));
    report("clipboard-read", clipboard::read());
    report("clipboard-write", clipboard::write("probe"));
    let targets = std::fs::read_to_string("/probe/targets.txt").unwrap_or_default();
    for (name, url) in targets.lines().filter_map(|line| line.split_once(' ')) {
        report(name, fetch::fetch(url, "GET", &[], None).map(|r| r.status));
    }
    notify::ready();
}
