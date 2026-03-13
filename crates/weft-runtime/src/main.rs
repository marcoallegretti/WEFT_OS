use std::path::PathBuf;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        anyhow::bail!(
            "usage: weft-runtime <app_id> <session_id> \
             [--preopen HOST::GUEST]... [--ipc-socket PATH]"
        );
    }
    let app_id = &args[1];
    let session_id: u64 = args[2]
        .parse()
        .with_context(|| format!("invalid session_id: {}", args[2]))?;

    let mut preopen: Vec<(String, String)> = Vec::new();
    let mut ipc_socket: Option<String> = None;

    let mut i = 3usize;
    while i < args.len() {
        match args[i].as_str() {
            "--preopen" => {
                i += 1;
                let spec = args.get(i).context("--preopen requires an argument")?;
                if let Some((host, guest)) = spec.split_once("::") {
                    preopen.push((host.to_string(), guest.to_string()));
                } else {
                    preopen.push((spec.clone(), spec.clone()));
                }
            }
            "--ipc-socket" => {
                i += 1;
                ipc_socket = Some(
                    args.get(i)
                        .context("--ipc-socket requires an argument")?
                        .clone(),
                );
            }
            other => anyhow::bail!("unexpected argument: {other}"),
        }
        i += 1;
    }

    #[cfg(feature = "seccomp")]
    apply_seccomp_filter().context("apply seccomp filter")?;

    tracing::info!(session_id, %app_id, "weft-runtime starting");

    let pkg_dir = resolve_package(app_id)?;
    tracing::info!(path = %pkg_dir.display(), "package resolved");

    let wasm_path = pkg_dir.join("app.wasm");
    if !wasm_path.exists() {
        anyhow::bail!("app.wasm not found at {}", wasm_path.display());
    }

    tracing::info!(session_id, %app_id, wasm = %wasm_path.display(), "executing module");
    run_module(&wasm_path, &preopen, ipc_socket.as_deref())?;

    tracing::info!(session_id, %app_id, "exiting");
    Ok(())
}

fn resolve_package(app_id: &str) -> anyhow::Result<PathBuf> {
    for store_root in package_store_roots() {
        let pkg_dir = store_root.join(app_id);
        let manifest = pkg_dir.join("wapp.toml");
        if manifest.exists() {
            return Ok(pkg_dir);
        }
    }
    anyhow::bail!("package '{}' not found in any package store", app_id)
}

#[cfg(not(feature = "wasmtime-runtime"))]
fn run_module(
    _wasm_path: &std::path::Path,
    _preopen: &[(String, String)],
    _ipc_socket: Option<&str>,
) -> anyhow::Result<()> {
    println!("READY");
    Ok(())
}

#[cfg(feature = "wasmtime-runtime")]
fn run_module(
    wasm_path: &std::path::Path,
    preopen: &[(String, String)],
    ipc_socket: Option<&str>,
) -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};
    use wasmtime::{
        Config, Engine, Store,
        component::{Component, Linker},
    };
    use wasmtime_wasi::{
        DirPerms, FilePerms, IoView, ResourceTable, WasiCtx, WasiCtxBuilder, WasiView,
        add_to_linker_sync, bindings::sync::Command,
    };

    struct IpcState {
        socket: std::os::unix::net::UnixStream,
        recv_buf: Vec<u8>,
    }

    impl IpcState {
        fn connect(path: &str) -> Option<Self> {
            let socket = std::os::unix::net::UnixStream::connect(path).ok()?;
            socket.set_nonblocking(true).ok()?;
            Some(Self {
                socket,
                recv_buf: Vec::new(),
            })
        }

        fn send(&mut self, payload: &str) -> Result<(), String> {
            use std::io::Write;
            let _ = self.socket.set_nonblocking(false);
            let mut line = payload.to_owned();
            line.push('\n');
            let result = self
                .socket
                .write_all(line.as_bytes())
                .map_err(|e| e.to_string());
            let _ = self.socket.set_nonblocking(true);
            result
        }

        fn recv(&mut self) -> Option<String> {
            use std::io::Read;
            let mut chunk = [0u8; 4096];
            loop {
                match self.socket.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => self.recv_buf.extend_from_slice(&chunk[..n]),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            if let Some(pos) = self.recv_buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = self.recv_buf.drain(..=pos).collect();
                return String::from_utf8(raw)
                    .ok()
                    .map(|s| s.trim_end_matches('\n').trim_end_matches('\r').to_owned());
            }
            None
        }
    }

    struct State {
        ctx: WasiCtx,
        table: ResourceTable,
        ipc: Arc<Mutex<Option<IpcState>>>,
    }

    impl IoView for State {
        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
    }

    impl WasiView for State {
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.ctx
        }
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).context("create engine")?;

    let component = Component::from_file(&engine, wasm_path)
        .with_context(|| format!("load component {}", wasm_path.display()))?;

    let mut linker: Linker<State> = Linker::new(&engine);
    add_to_linker_sync(&mut linker).context("add WASI to linker")?;
    let ipc_state: Arc<Mutex<Option<IpcState>>> = Arc::new(Mutex::new(None));

    {
        let ipc_send = Arc::clone(&ipc_state);
        let ipc_recv = Arc::clone(&ipc_state);
        linker
            .instance("weft:app/notify@0.1.0")
            .context("define weft:app/notify instance")?
            .func_wrap("ready", |_: wasmtime::StoreContextMut<'_, State>, ()| {
                println!("READY");
                Ok::<(), wasmtime::Error>(())
            })
            .context("define weft:app/notify#ready")?;

        let mut ipc_instance = linker
            .instance("weft:app/ipc@0.1.0")
            .context("define weft:app/ipc instance")?;

        ipc_instance
            .func_wrap(
                "send",
                move |_: wasmtime::StoreContextMut<'_, State>,
                      (payload,): (String,)|
                      -> wasmtime::Result<(Result<(), String>,)> {
                    let mut guard = ipc_send.lock().unwrap_or_else(|p| p.into_inner());
                    match guard.as_mut() {
                        Some(ipc) => Ok((ipc.send(&payload),)),
                        None => Ok((Err("IPC not connected".to_owned()),)),
                    }
                },
            )
            .context("define weft:app/ipc#send")?;

        ipc_instance
            .func_wrap(
                "recv",
                move |_: wasmtime::StoreContextMut<'_, State>,
                      ()|
                      -> wasmtime::Result<(Option<String>,)> {
                    let mut guard = ipc_recv.lock().unwrap_or_else(|p| p.into_inner());
                    Ok((guard.as_mut().and_then(|ipc| ipc.recv()),))
                },
            )
            .context("define weft:app/ipc#recv")?;
    }

    linker
        .instance("weft:app/fetch@0.1.0")
        .context("define weft:app/fetch instance")?
        .func_wrap(
            "fetch",
            |_: wasmtime::StoreContextMut<'_, State>,
             (url, method, headers, body): (
                String,
                String,
                Vec<(String, String)>,
                Option<Vec<u8>>,
            )|
             -> wasmtime::Result<(Result<(u16, String, Vec<u8>), String>,)> {
                let result = host_fetch(&url, &method, &headers, body.as_deref());
                Ok((result,))
            },
        )
        .context("define weft:app/fetch#fetch")?;

    linker
        .instance("weft:app/notifications@0.1.0")
        .context("define weft:app/notifications instance")?
        .func_wrap(
            "notify",
            |_: wasmtime::StoreContextMut<'_, State>,
             (title, body, icon): (String, String, Option<String>)|
             -> wasmtime::Result<(Result<(), String>,)> {
                let result = host_notify(&title, &body, icon.as_deref());
                Ok((result,))
            },
        )
        .context("define weft:app/notifications#notify")?;

    {
        let mut clipboard = linker
            .instance("weft:app/clipboard@0.1.0")
            .context("define weft:app/clipboard instance")?;
        clipboard
            .func_wrap(
                "read",
                |_: wasmtime::StoreContextMut<'_, State>,
                 ()|
                 -> wasmtime::Result<(Result<String, String>,)> {
                    Ok((host_clipboard_read(),))
                },
            )
            .context("define weft:app/clipboard#read")?;
        clipboard
            .func_wrap(
                "write",
                |_: wasmtime::StoreContextMut<'_, State>,
                 (text,): (String,)|
                 -> wasmtime::Result<(Result<(), String>,)> {
                    Ok((host_clipboard_write(&text),))
                },
            )
            .context("define weft:app/clipboard#write")?;
    }

    let mut ctx_builder = WasiCtxBuilder::new();
    ctx_builder.inherit_stdout().inherit_stderr();

    if let Some(socket_path) = ipc_socket {
        ctx_builder.env("WEFT_IPC_SOCKET", socket_path);
        if let Some(ipc) = IpcState::connect(socket_path) {
            *ipc_state.lock().unwrap_or_else(|p| p.into_inner()) = Some(ipc);
        } else {
            tracing::warn!("weft:app/ipc: could not connect to IPC socket {socket_path}");
        }
    }

    if let Ok(portal_socket) = std::env::var("WEFT_FILE_PORTAL_SOCKET") {
        ctx_builder.env("WEFT_FILE_PORTAL_SOCKET", &portal_socket);
    }

    for (host_path, guest_path) in preopen {
        ctx_builder
            .preopened_dir(host_path, guest_path, DirPerms::all(), FilePerms::all())
            .with_context(|| format!("preopen dir {host_path}"))?;
    }

    let ctx = ctx_builder.build();
    let mut store = Store::new(
        &engine,
        State {
            ctx,
            table: ResourceTable::new(),
            ipc: ipc_state,
        },
    );

    let command =
        Command::instantiate(&mut store, &component, &linker).context("instantiate component")?;

    command
        .wasi_cli_run()
        .call_run(&mut store)
        .context("call run")?
        .map_err(|()| anyhow::anyhow!("wasm component run exited with error"))
}

#[cfg(all(feature = "wasmtime-runtime", feature = "net-fetch"))]
fn host_fetch(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Result<(u16, String, Vec<u8>), String> {
    use std::io::Read;
    let mut req = ureq::request(method, url);
    for (name, value) in headers {
        req = req.set(name, value);
    }
    let response = match body {
        Some(b) => req.send_bytes(b),
        None => req.call(),
    }
    .map_err(|e| e.to_string())?;
    let status = response.status();
    let content_type = response.content_type().to_owned();
    let mut body_bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body_bytes)
        .map_err(|e| e.to_string())?;
    Ok((status, content_type, body_bytes))
}

#[cfg(all(feature = "wasmtime-runtime", not(feature = "net-fetch")))]
fn host_fetch(
    _url: &str,
    _method: &str,
    _headers: &[(String, String)],
    _body: Option<&[u8]>,
) -> Result<(u16, String, Vec<u8>), String> {
    Err("net-fetch capability not compiled in".to_owned())
}

#[cfg(feature = "wasmtime-runtime")]
fn host_clipboard_read() -> Result<String, String> {
    let out = std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        String::from_utf8(out.stdout).map_err(|e| e.to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

#[cfg(feature = "wasmtime-runtime")]
fn host_clipboard_write(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy exited with {status}"))
    }
}

#[cfg(feature = "wasmtime-runtime")]
fn host_notify(title: &str, body: &str, icon: Option<&str>) -> Result<(), String> {
    let mut cmd = std::process::Command::new("notify-send");
    if let Some(i) = icon {
        cmd.arg("--icon").arg(i);
    }
    cmd.arg("--").arg(title).arg(body);
    cmd.status().map_err(|e| e.to_string()).and_then(|s| {
        if s.success() {
            Ok(())
        } else {
            Err(format!("notify-send exited with {s}"))
        }
    })
}

#[cfg(feature = "seccomp")]
fn apply_seccomp_filter() -> anyhow::Result<()> {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
    use std::collections::BTreeMap;
    use std::convert::TryInto;

    #[cfg(target_arch = "x86_64")]
    let arch = seccompiler::TargetArch::x86_64;
    #[cfg(target_arch = "aarch64")]
    let arch = seccompiler::TargetArch::aarch64;

    let blocked: &[i64] = &[
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kexec_load,
        libc::SYS_personality,
        libc::SYS_syslog,
        libc::SYS_reboot,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_setuid,
        libc::SYS_setgid,
        libc::SYS_setreuid,
        libc::SYS_setregid,
        libc::SYS_setresuid,
        libc::SYS_setresgid,
        libc::SYS_chroot,
        libc::SYS_pivot_root,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_acct,
    ];

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for &syscall in blocked {
        rules.insert(syscall, vec![]);
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::KillProcess,
        arch,
    )?;
    let bpf: BpfProgram = filter.try_into()?;
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    anyhow::ensure!(
        ret == 0,
        "prctl PR_SET_NO_NEW_PRIVS failed: {}",
        std::io::Error::last_os_error()
    );
    seccompiler::apply_filter(&bpf)?;
    Ok(())
}

fn package_store_roots() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return vec![PathBuf::from(explicit)];
    }

    let mut roots = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("weft")
                .join("apps"),
        );
    }

    roots.push(PathBuf::from("/usr/share/weft/apps"));

    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_store_roots_includes_system_path() {
        let roots = package_store_roots();
        assert!(
            roots
                .iter()
                .any(|p| p == &PathBuf::from("/usr/share/weft/apps"))
        );
    }

    #[test]
    fn package_store_roots_uses_weft_app_store_when_set() {
        // SAFETY: test binary is single-threaded at this point.
        unsafe { std::env::set_var("WEFT_APP_STORE", "/custom/store") };
        let roots = package_store_roots();
        assert_eq!(roots, vec![PathBuf::from("/custom/store")]);
        unsafe { std::env::remove_var("WEFT_APP_STORE") };
    }

    #[test]
    fn resolve_package_finds_installed_package() {
        use std::fs;
        let store =
            std::env::temp_dir().join(format!("weft_runtime_resolve_{}", std::process::id()));
        let pkg_dir = store.join("com.example.resolve");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("wapp.toml"),
            "[package]\nid=\"com.example.resolve\"\n",
        )
        .unwrap();

        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };

        let result = resolve_package("com.example.resolve");

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        let _ = fs::remove_dir_all(&store);

        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("com.example.resolve"));
    }

    #[test]
    fn resolve_package_errors_on_unknown_id() {
        let store =
            std::env::temp_dir().join(format!("weft_runtime_resolve_empty_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&store);

        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };

        let result = resolve_package("com.does.not.exist");

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        let _ = std::fs::remove_dir_all(&store);

        assert!(result.is_err());
    }
}
