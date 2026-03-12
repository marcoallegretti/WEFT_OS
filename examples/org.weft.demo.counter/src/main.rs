wit_bindgen::generate!({
    path: "wit",
    world: "app",
    with: {
        "weft:app/notify@0.1.0": generate,
        "weft:app/ipc@0.1.0": generate,
    },
});

use weft::app::{ipc, notify};

fn main() {
    notify::ready();

    let mut count: i32 = 0;

    loop {
        if let Some(raw) = ipc::recv() {
            let reply = match raw.trim() {
                "increment" => {
                    count += 1;
                    format!("{{\"count\":{count}}}")
                }
                "decrement" => {
                    count -= 1;
                    format!("{{\"count\":{count}}}")
                }
                "reset" => {
                    count = 0;
                    format!("{{\"count\":{count}}}")
                }
                "get" => {
                    format!("{{\"count\":{count}}}")
                }
                _ => continue,
            };
            let _ = ipc::send(&reply);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
