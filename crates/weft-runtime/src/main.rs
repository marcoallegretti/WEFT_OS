use std::path::PathBuf;

use anyhow::Context;

mod grants;
#[cfg(feature = "wasmtime-runtime")]
mod ipc;
#[cfg(feature = "wasmtime-runtime")]
mod limits;

/// The default limit on a component's linear memory, in MiB.
const DEFAULT_MAX_MEMORY_MIB: usize = 256;

use grants::{Grants, Preopen};

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
            "usage: weft-runtime <app_id> <session_id> --module PATH \
             [--preopen HOST::GUEST::ro|rw]... [--grant CAPABILITY]... [--ipc-socket PATH] \
             [--max-memory-mib N]"
        );
    }
    let app_id = &args[1];
    let session_id: u64 = args[2]
        .parse()
        .with_context(|| format!("invalid session_id: {}", args[2]))?;

    let mut preopen: Vec<Preopen> = Vec::new();
    let mut grants = Grants::default();
    let mut ipc_socket: Option<String> = None;
    let mut module: Option<PathBuf> = None;
    let mut max_memory_mib = DEFAULT_MAX_MEMORY_MIB;

    let mut i = 3usize;
    while i < args.len() {
        match args[i].as_str() {
            "--preopen" => {
                i += 1;
                let spec = args.get(i).context("--preopen requires an argument")?;
                preopen.push(Preopen::parse(spec)?);
            }
            "--grant" => {
                i += 1;
                grants.add(args.get(i).context("--grant requires an argument")?)?;
            }
            "--module" => {
                i += 1;
                module = Some(PathBuf::from(
                    args.get(i).context("--module requires an argument")?,
                ));
            }
            "--max-memory-mib" => {
                i += 1;
                let value = args
                    .get(i)
                    .context("--max-memory-mib requires an argument")?;
                max_memory_mib = value
                    .parse::<usize>()
                    .ok()
                    .filter(|&mib| mib > 0 && mib <= 1 << 20)
                    .with_context(|| format!("invalid --max-memory-mib: {value}"))?;
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

    // weft-appd resolves the package and passes the component it selected;
    // the runtime never looks for the package itself.
    let wasm_path = module.context("--module is required")?;

    tracing::info!(session_id, %app_id, wasm = %wasm_path.display(), "executing module");
    run_module(
        &wasm_path,
        &preopen,
        grants,
        ipc_socket.as_deref(),
        max_memory_mib << 20,
    )?;

    tracing::info!(session_id, %app_id, "exiting");
    Ok(())
}

/// The readiness line for weft-appd: `READY <token>` with the session token
/// from `WEFT_READY_TOKEN`, or `READY` when run without appd.
#[cfg(feature = "wasmtime-runtime")]
fn ready_line() -> String {
    match std::env::var("WEFT_READY_TOKEN") {
        Ok(token) => format!("READY {token}"),
        Err(_) => "READY".to_owned(),
    }
}

#[cfg(not(feature = "wasmtime-runtime"))]
fn run_module(
    _wasm_path: &std::path::Path,
    _preopen: &[Preopen],
    _grants: Grants,
    _ipc_socket: Option<&str>,
    _max_memory: usize,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "weft-runtime was built without the wasmtime-runtime feature and cannot execute components"
    )
}

#[cfg(feature = "wasmtime-runtime")]
fn run_module(
    wasm_path: &std::path::Path,
    preopen: &[Preopen],
    grants: Grants,
    ipc_socket: Option<&str>,
    max_memory: usize,
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
    use weft_ipc_types::capability::Access;

    struct State {
        ctx: WasiCtx,
        table: ResourceTable,
        limits: limits::Limits,
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

    let grants = Arc::new(grants);
    let mut linker: Linker<State> = Linker::new(&engine);
    add_to_linker_sync(&mut linker).context("add WASI to linker")?;
    let ipc_state: Arc<Mutex<Option<ipc::IpcState>>> = Arc::new(Mutex::new(None));

    {
        let ipc_send = Arc::clone(&ipc_state);
        let ipc_recv = Arc::clone(&ipc_state);
        linker
            .instance("weft:app/notify@0.1.0")
            .context("define weft:app/notify instance")?
            .func_wrap("ready", |_: wasmtime::StoreContextMut<'_, State>, ()| {
                println!("{}", ready_line());
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
                    if let Err(e) = ipc::check_payload(&payload) {
                        return Ok((Err(e),));
                    }
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

    // Every import below checks the session's grants on each call; an
    // ungranted call returns an error to the component.
    let fetch_grants = Arc::clone(&grants);
    linker
        .instance("weft:app/fetch@0.1.0")
        .context("define weft:app/fetch instance")?
        .func_wrap(
            "fetch",
            move |_: wasmtime::StoreContextMut<'_, State>,
                  (url, method, headers, body): FetchRequest|
                  -> wasmtime::Result<(FetchResult,)> {
                let result = host_fetch(&fetch_grants, &url, &method, &headers, body.as_deref());
                Ok((result,))
            },
        )
        .context("define weft:app/fetch#fetch")?;

    let notify_grants = Arc::clone(&grants);
    linker
        .instance("weft:app/notifications@0.1.0")
        .context("define weft:app/notifications instance")?
        .func_wrap(
            "notify",
            move |_: wasmtime::StoreContextMut<'_, State>,
                  (title, body, icon): (String, String, Option<String>)|
                  -> wasmtime::Result<(Result<(), String>,)> {
                let result = notify_grants
                    .notifications()
                    .and_then(|()| host_notify(&title, &body, icon.as_deref()));
                Ok((result,))
            },
        )
        .context("define weft:app/notifications#notify")?;

    {
        let mut clipboard = linker
            .instance("weft:app/clipboard@0.1.0")
            .context("define weft:app/clipboard instance")?;
        let read_grants = Arc::clone(&grants);
        clipboard
            .func_wrap(
                "read",
                move |_: wasmtime::StoreContextMut<'_, State>,
                      ()|
                      -> wasmtime::Result<(Result<String, String>,)> {
                    Ok((read_grants
                        .clipboard_read()
                        .and_then(|()| host_clipboard_read()),))
                },
            )
            .context("define weft:app/clipboard#read")?;
        let write_grants = Arc::clone(&grants);
        clipboard
            .func_wrap(
                "write",
                move |_: wasmtime::StoreContextMut<'_, State>,
                      (text,): (String,)|
                      -> wasmtime::Result<(Result<(), String>,)> {
                    Ok((write_grants
                        .clipboard_write()
                        .and_then(|()| host_clipboard_write(&text)),))
                },
            )
            .context("define weft:app/clipboard#write")?;
    }

    let mut ctx_builder = WasiCtxBuilder::new();
    ctx_builder.inherit_stdout().inherit_stderr();

    if let Some(socket_path) = ipc_socket {
        ctx_builder.env("WEFT_IPC_SOCKET", socket_path);
        if let Some(ipc) = ipc::IpcState::connect(socket_path) {
            ipc.exit_on_hangup().context("watch the IPC connection")?;
            *ipc_state.lock().unwrap_or_else(|p| p.into_inner()) = Some(ipc);
        } else {
            // Without its connection the session cannot be reached or
            // ended; the runtime would only be left behind.
            anyhow::bail!("cannot connect to the IPC socket {socket_path}");
        }
    }

    if let Ok(portal_socket) = std::env::var("WEFT_FILE_PORTAL_SOCKET") {
        ctx_builder.env("WEFT_FILE_PORTAL_SOCKET", &portal_socket);
    }

    for dir in preopen {
        let (dir_perms, file_perms) = match dir.access {
            Access::Read => (DirPerms::READ, FilePerms::READ),
            Access::ReadWrite => (DirPerms::all(), FilePerms::all()),
        };
        ctx_builder
            .preopened_dir(&dir.host, &dir.guest, dir_perms, file_perms)
            .with_context(|| format!("preopen dir {}", dir.host))?;
    }

    let ctx = ctx_builder.build();
    let mut store = Store::new(
        &engine,
        State {
            ctx,
            table: ResourceTable::new(),
            limits: limits::Limits::new(max_memory),
        },
    );
    store.limiter(|state| &mut state.limits);

    let command =
        Command::instantiate(&mut store, &component, &linker).context("instantiate component")?;

    command
        .wasi_cli_run()
        .call_run(&mut store)
        .context("call run")?
        .map_err(|()| anyhow::anyhow!("wasm component run exited with error"))
}

/// Arguments of `weft:app/fetch#fetch`: URL, method, headers and body.
#[cfg(feature = "wasmtime-runtime")]
type FetchRequest = (String, String, Vec<(String, String)>, Option<Vec<u8>>);

/// `weft:app/fetch.response`.
#[cfg(feature = "wasmtime-runtime")]
#[derive(
    wasmtime::component::ComponentType, wasmtime::component::Lift, wasmtime::component::Lower,
)]
#[component(record)]
struct FetchResponse {
    status: u16,
    #[component(name = "content-type")]
    content_type: String,
    body: Vec<u8>,
}

/// Result of `weft:app/fetch#fetch`.
#[cfg(feature = "wasmtime-runtime")]
type FetchResult = Result<FetchResponse, String>;

/// Request headers a component may not set.
#[cfg(all(feature = "wasmtime-runtime", feature = "net-fetch"))]
const FORBIDDEN_FETCH_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
];

/// Longest response body returned to a component.
#[cfg(all(feature = "wasmtime-runtime", feature = "net-fetch"))]
const MAX_FETCH_RESPONSE: u64 = 16 * 1024 * 1024;

/// Performs a granted HTTP request. Redirects are returned to the component
/// as responses rather than followed, so every destination it reaches is
/// checked against its grants. HTTP error statuses are responses too; only
/// policy and transport failures are errors.
#[cfg(all(feature = "wasmtime-runtime", feature = "net-fetch"))]
fn host_fetch(
    grants: &Grants,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> FetchResult {
    use std::io::Read;
    use std::time::Duration;
    grants.fetch_any()?;
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("unsupported URL scheme '{}'", parsed.scheme()));
    }
    let host = parsed.host_str().ok_or("URL has no host")?;
    grants.fetch_host(host)?;
    // The method is written into the request line as given, so only known
    // methods are accepted.
    if !matches!(
        method,
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    ) {
        return Err(format!("unsupported HTTP method '{method}'"));
    }
    // The destination and message framing belong to the runtime: a component
    // must not redirect a granted request to another host through `Host` or
    // change how the body is delimited.
    for (name, _) in headers {
        if FORBIDDEN_FETCH_HEADERS
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
        {
            return Err(format!("header '{name}' is set by the runtime"));
        }
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build();
    let mut req = agent.request_url(method, &parsed);
    for (name, value) in headers {
        req = req.set(name, value);
    }
    let response = match body {
        Some(b) => req.send_bytes(b),
        None => req.call(),
    };
    let response = match response {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(e) => return Err(e.to_string()),
    };
    let status = response.status();
    let content_type = response.content_type().to_owned();
    let mut body_bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_FETCH_RESPONSE + 1)
        .read_to_end(&mut body_bytes)
        .map_err(|e| e.to_string())?;
    if body_bytes.len() as u64 > MAX_FETCH_RESPONSE {
        return Err(format!("response body exceeds {MAX_FETCH_RESPONSE} bytes"));
    }
    Ok(FetchResponse {
        status,
        content_type,
        body: body_bytes,
    })
}

#[cfg(all(feature = "wasmtime-runtime", not(feature = "net-fetch")))]
fn host_fetch(
    grants: &Grants,
    _url: &str,
    _method: &str,
    _headers: &[(String, String)],
    _body: Option<&[u8]>,
) -> FetchResult {
    grants.fetch_any()?;
    Err("this runtime was built without network fetch support".to_owned())
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

#[cfg(all(test, not(feature = "wasmtime-runtime")))]
mod tests {
    use super::*;

    #[test]
    fn engine_disabled_build_refuses_to_run_components() {
        let err = run_module(
            std::path::Path::new("app.wasm"),
            &[],
            Grants::default(),
            None,
            DEFAULT_MAX_MEMORY_MIB << 20,
        )
        .expect_err("a build without wasmtime-runtime must not report success");
        assert!(err.to_string().contains("wasmtime-runtime"));
    }
}
