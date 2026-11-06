//! Bounded service-manager operations over an operator-owned allowlist.
//!
//! A self restart is deliberately queued before the native manager is called.
//! The ticket survives the daemon; a later boot is observed separately from a
//! successful manager-command exit, which self termination may prevent recording.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

use kamimusuhi_runtime::agent_exec::process::{self, ProcessSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{spool::Spool, state::Shared};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const RESTART_DELAY: Duration = Duration::from_secs(1);
const TICKET_BYTES: u64 = 8192;
static TICKET_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Launchd,
    SystemdSystem,
    SystemdUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub name: String,
    pub manager: ServiceManager,
    pub unit: String,
    #[serde(default)]
    pub self_service: bool,
    #[serde(default)]
    pub privileged: bool,
    /// Opt in only when the already configured supervisor restarts clean exits.
    #[serde(default)]
    pub restart_via_exit: bool,
}

fn label(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && text.as_bytes()[0].is_ascii_alphanumeric()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

impl ServiceConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.restart_via_exit && !self.self_service {
            return Err("restart_via_exit requires self_service=true".to_owned());
        }
        if !label(&self.name, 64) {
            return Err("service name must be a bounded alphanumeric label".to_owned());
        }
        let valid = match self.manager {
            ServiceManager::Launchd => {
                let parts: Vec<_> = self.unit.split('/').collect();
                parts.len() == 3
                    && matches!(parts[0], "gui" | "user")
                    && !parts[1].is_empty()
                    && parts[1].len() <= 10
                    && parts[1].bytes().all(|b| b.is_ascii_digit())
                    && label(parts[2], 160)
                    && !self.privileged
            }
            ServiceManager::SystemdSystem | ServiceManager::SystemdUser => {
                self.unit.ends_with(".service")
                    && self.unit.len() > ".service".len()
                    && self.unit.len() <= 200
                    && self.unit.as_bytes()[0].is_ascii_alphanumeric()
                    && self
                        .unit
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._@:-".contains(&b))
                    && (self.manager == ServiceManager::SystemdSystem || !self.privileged)
            }
        };
        if valid {
            Ok(())
        } else {
            Err("service manager/unit configuration is invalid".to_owned())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NativeCommand {
    program: &'static str,
    args: Vec<String>,
}

fn command(service: &ServiceConfig, restart: bool) -> NativeCommand {
    let mut args = Vec::new();
    match service.manager {
        ServiceManager::Launchd => {
            args.push(if restart { "kickstart" } else { "print" }.to_owned());
            if restart {
                args.push("-k".to_owned());
            }
            args.push(service.unit.clone());
            NativeCommand {
                program: "/bin/launchctl",
                args,
            }
        }
        ServiceManager::SystemdSystem | ServiceManager::SystemdUser => {
            let program = if restart && service.privileged {
                args.extend(["-n".to_owned(), "/usr/bin/systemctl".to_owned()]);
                "/usr/bin/sudo"
            } else {
                "/usr/bin/systemctl"
            };
            if service.manager == ServiceManager::SystemdUser {
                args.push("--user".to_owned());
            }
            if restart {
                args.extend(["restart".to_owned(), service.unit.clone()]);
            } else {
                args.extend([
                    "show".to_owned(),
                    service.unit.clone(),
                    "--property=ActiveState,SubState,MainPID".to_owned(),
                    "--no-pager".to_owned(),
                ]);
            }
            NativeCommand { program, args }
        }
    }
}

struct Outcome {
    success: bool,
    exit_code: Option<i32>,
    duration_ms: u64,
    stdout: String,
}

fn run(cmd: &NativeCommand) -> Result<Outcome, ()> {
    process::run(
        &ProcessSpec {
            program: cmd.program,
            args: &cmd.args,
            cwd: None,
            // Required by systemctl --user; service/node credentials remain absent.
            env_passthrough: &[
                "XDG_RUNTIME_DIR".to_owned(),
                "DBUS_SESSION_BUS_ADDRESS".to_owned(),
            ],
            extra_env: &[],
            timeout: COMMAND_TIMEOUT,
        },
        &AtomicBool::new(false),
        |_| {},
    )
    .map(|o| Outcome {
        success: o.success(),
        exit_code: o.exit_code,
        duration_ms: o.duration_ms,
        stdout: o.stdout,
    })
    .map_err(|_| ())
}

/// Only manager-owned state fields leave this module. In particular a
/// launchctl document's Environment and inherited environment never do.
fn status_view(service: &ServiceConfig, outcome: Outcome) -> Value {
    if !outcome.success {
        return json!({"ok":false,"error":"service status command failed","exit_code":outcome.exit_code});
    }
    let mut fields = serde_json::Map::new();
    for line in outcome.stdout.lines() {
        // launchctl nests Environment and other dictionaries by two or more
        // tabs. Only the job's own state fields are eligible for projection.
        if service.manager == ServiceManager::Launchd
            && line.chars().take_while(|c| c.is_whitespace()).count() > 1
        {
            continue;
        }
        let line = line.trim();
        let pair = match service.manager {
            ServiceManager::Launchd => line.split_once(" = ").filter(|(key, _)| {
                matches!(
                    *key,
                    "state" | "pid" | "runs" | "last exit code" | "last terminating signal"
                )
            }),
            _ => line
                .split_once('=')
                .filter(|(key, _)| matches!(*key, "ActiveState" | "SubState" | "MainPID")),
        };
        if let Some((key, value)) = pair {
            // A small projection also bounds malformed native-manager output.
            fields.insert(
                key.to_owned(),
                json!(value.chars().take(100).collect::<String>()),
            );
        }
    }
    let active = match service.manager {
        ServiceManager::Launchd => fields.get("state").is_some_and(|v| v == "running"),
        _ => fields.get("ActiveState").is_some_and(|v| v == "active"),
    };
    json!({"ok":true,"result":{"name":service.name,"manager":service.manager,"active":active,"fields":fields,"duration_ms":outcome.duration_ms}})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestState {
    Queued,
    Running,
    AwaitingSupervisor,
    CommandSucceeded,
    CommandFailed,
    ObservedRestart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    request_id: String,
    service: String,
    node: String,
    self_service: bool,
    boot_epoch: u64,
    state: RequestState,
    created_at: u64,
    updated_at: u64,
    exit_code: Option<i32>,
    command_duration_ms: Option<u64>,
    observed_boot_epoch: Option<u64>,
}

fn ticket_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn ticket_path(dir: &Path, id: &str) -> Result<PathBuf, &'static str> {
    if !ticket_id(id) {
        return Err("request_id must be 32 lowercase hexadecimal characters");
    }
    Ok(dir.join(format!("{id}.json")))
}

fn save(path: &Path, ticket: &Ticket) -> Result<(), &'static str> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "cannot create service request directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| "cannot protect service request directory")?;
        }
    }
    let data = serde_json::to_vec(ticket).map_err(|_| "cannot encode service request")?;
    crate::util::atomic_write(path, &data).map_err(|_| "cannot save service request")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect service request")?;
    }
    Ok(())
}

fn load(path: &Path, id: &str) -> Result<Ticket, &'static str> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|_| "unknown service request")?;
    let mut data = Vec::new();
    file.take(TICKET_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| "cannot read service request")?;
    if data.len() as u64 > TICKET_BYTES {
        return Err("service request exceeds size limit");
    }
    let ticket: Ticket =
        serde_json::from_slice(&data).map_err(|_| "invalid service request state")?;
    if ticket.request_id != id {
        return Err("service request identity mismatch");
    }
    Ok(ticket)
}

fn observe(ticket: &mut Ticket, node: &str, epoch: u64, at: u64) -> bool {
    if ticket.node == node
        && ticket.self_service
        && epoch > ticket.boot_epoch
        && matches!(
            ticket.state,
            RequestState::Queued
                | RequestState::Running
                | RequestState::AwaitingSupervisor
                | RequestState::CommandSucceeded
        )
    {
        ticket.state = RequestState::ObservedRestart;
        ticket.observed_boot_epoch = Some(epoch);
        ticket.updated_at = at;
        return true;
    }
    false
}

fn journal(spool: &Spool, ticket: &Ticket) {
    let _ = spool.append_sync("logs/service-control", json!({"request_id":ticket.request_id,"service":ticket.service,"state":ticket.state,"boot_epoch":ticket.boot_epoch,"exit_code":ticket.exit_code,"observed_boot_epoch":ticket.observed_boot_epoch}));
}

fn execute_restart(
    path: &Path,
    mut ticket: Ticket,
    service: &ServiceConfig,
    spool: &Spool,
    runner: impl FnOnce(&NativeCommand) -> Result<Outcome, ()>,
) {
    {
        let _guard = TICKET_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        ticket.state = RequestState::Running;
        ticket.updated_at = crate::util::unix_now();
        if save(path, &ticket).is_err() {
            return;
        }
        journal(spool, &ticket);
    }
    let outcome = runner(&command(service, true));
    ticket.updated_at = crate::util::unix_now();
    match outcome {
        Ok(o) => {
            ticket.state = if o.success {
                RequestState::CommandSucceeded
            } else {
                RequestState::CommandFailed
            };
            ticket.exit_code = o.exit_code;
            ticket.command_duration_ms = Some(o.duration_ms);
        }
        Err(()) => ticket.state = RequestState::CommandFailed,
    }
    let _guard = TICKET_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _ = save(path, &ticket);
    journal(spool, &ticket);
}

/// The configured supervisor owns the restart. Persist and sync the handoff
/// before exiting, so the next resident can observe it without requiring sudo.
fn exit_for_supervisor(
    path: &Path,
    mut ticket: Ticket,
    spool: &Spool,
    exit: impl FnOnce(i32),
) -> Result<(), &'static str> {
    if !ticket.self_service {
        return Err("only the resident can request its own supervisor restart");
    }
    {
        let _guard = TICKET_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        ticket.state = RequestState::AwaitingSupervisor;
        ticket.updated_at = crate::util::unix_now();
        save(path, &ticket)?;
        journal(spool, &ticket);
    }
    exit(0);
    Ok(())
}

pub fn definitions() -> Vec<Value> {
    vec![json!({"type":"function","function":{
        "name":"service_control",
        "description":"List, inspect or restart services on this node's configured allowlist. restart returns a durable request_id before a one-second delayed native-manager or configured supervisor request; request_status inspects it after reconnecting. A self-service restart interrupts the node and potentially its current conversation. observed_restart means a later resident boot was observed, not proof of a zero command exit. No shell, arbitrary PID, service registration, host reboot or credentials are accepted.",
        "parameters":{"type":"object","additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","status","restart","request_status"]},
            "name":{"type":"string","description":"configured service name; status/restart only"},
            "request_id":{"type":"string","pattern":"^[a-f0-9]{32}$","description":"required for request_status; optional idempotency key for restart"}
        },"required":["action"]}
    }})]
}

fn fail(message: &str) -> (u16, Value) {
    (200, json!({"ok":false,"error":message}))
}

pub fn call(shared: &Shared, args: &Value) -> (u16, Value) {
    let Some(map) = args.as_object() else {
        return fail("service arguments must be an object");
    };
    let action = args["action"].as_str().unwrap_or("");
    let allowed: &[&str] = match action {
        "list" => &["action"],
        "status" => &["action", "name"],
        "restart" => &["action", "name", "request_id"],
        "request_status" => &["action", "request_id"],
        _ => return fail("unknown service action"),
    };
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return fail("unsupported service argument");
    }
    let services = &shared.config.tools.services;
    if action == "list" {
        return (200, json!({"ok":true,"result":{"services":services}}));
    }
    let dir = shared.config.paths.current_state().join("service-control");
    if action == "request_status" {
        let Some(id) = args["request_id"].as_str() else {
            return fail("request_id is required");
        };
        let path = match ticket_path(&dir, id) {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        let _guard = TICKET_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut ticket = match load(&path, id) {
            Ok(t) => t,
            Err(e) => return fail(e),
        };
        let Some(service) = services.iter().find(|s| s.name == ticket.service) else {
            return fail("service is no longer configured");
        };
        if ticket.node != shared.config.node.id || ticket.self_service != service.self_service {
            return fail("service request identity mismatch");
        }
        if observe(
            &mut ticket,
            &shared.config.node.id,
            shared.boot_epoch,
            crate::util::unix_now(),
        ) {
            if let Err(e) = save(&path, &ticket) {
                return fail(e);
            }
            journal(&shared.spool, &ticket);
        }
        return (200, json!({"ok":true,"result":ticket}));
    }
    let Some(name) = args["name"].as_str() else {
        return fail("service name is required");
    };
    let Some(service) = services.iter().find(|s| s.name == name) else {
        return fail("service is not configured");
    };
    if service.validate().is_err() {
        return fail("service configuration is invalid");
    }
    if action == "status" {
        return match run(&command(service, false)) {
            Ok(outcome) => (200, status_view(service, outcome)),
            Err(()) => fail("could not execute service status command"),
        };
    }
    let id = match args.get("request_id") {
        Some(Value::String(id)) if ticket_id(id) => id.clone(),
        Some(_) => return fail("request_id must be 32 lowercase hexadecimal characters"),
        None => match process::random_hex() {
            Some(id) => id,
            None => return fail("could not create request id"),
        },
    };
    let path = match ticket_path(&dir, &id) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let _guard = TICKET_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if path.exists() {
        return match load(&path, &id) {
            Ok(t) if t.node == shared.config.node.id && t.service == name => (
                200,
                json!({"ok":true,"result":{"accepted":true,"duplicate":true,"request_id":id,"state":t.state}}),
            ),
            _ => fail("request_id is already used by a different service request"),
        };
    }
    let at = crate::util::unix_now();
    let ticket = Ticket {
        request_id: id.clone(),
        service: name.to_owned(),
        node: shared.config.node.id.clone(),
        self_service: service.self_service,
        boot_epoch: shared.boot_epoch,
        state: RequestState::Queued,
        created_at: at,
        updated_at: at,
        exit_code: None,
        command_duration_ms: None,
        observed_boot_epoch: None,
    };
    if let Err(e) = save(&path, &ticket) {
        return fail(e);
    }
    journal(&shared.spool, &ticket);
    let service = service.clone();
    let execution_path = path.clone();
    let spool: Arc<Spool> = Arc::clone(&shared.spool);
    if std::thread::Builder::new()
        .name("service-restart".to_owned())
        .spawn(move || {
            std::thread::sleep(RESTART_DELAY);
            if service.restart_via_exit {
                let _ = exit_for_supervisor(&execution_path, ticket, &spool, |code| {
                    std::process::exit(code)
                });
            } else {
                execute_restart(&execution_path, ticket, &service, &spool, run);
            }
        })
        .is_err()
    {
        if let Ok(mut failed) = load(&path, &id) {
            failed.state = RequestState::CommandFailed;
            failed.updated_at = crate::util::unix_now();
            let _ = save(&path, &failed);
            journal(&shared.spool, &failed);
        }
        return fail("could not schedule service restart");
    }
    (
        200,
        json!({"ok":true,"result":{"accepted":true,"request_id":id,"state":"queued","delay_ms":RESTART_DELAY.as_millis()}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(manager: ServiceManager) -> ServiceConfig {
        ServiceConfig {
            name: "resident".to_owned(),
            manager,
            unit: if manager == ServiceManager::Launchd {
                "gui/501/com.example.resident"
            } else {
                "example.service"
            }
            .to_owned(),
            self_service: true,
            privileged: false,
            restart_via_exit: false,
        }
    }
    fn ticket() -> Ticket {
        Ticket {
            request_id: "0123456789abcdef0123456789abcdef".to_owned(),
            service: "resident".to_owned(),
            node: "test".to_owned(),
            self_service: true,
            boot_epoch: 7,
            state: RequestState::Queued,
            created_at: 100,
            updated_at: 100,
            exit_code: None,
            command_duration_ms: None,
            observed_boot_epoch: None,
        }
    }
    #[test]
    fn service_control_native_arguments_are_fixed_and_bounded() {
        let mac = service(ServiceManager::Launchd);
        assert!(mac.validate().is_ok());
        assert_eq!(
            command(&mac, true),
            NativeCommand {
                program: "/bin/launchctl",
                args: vec![
                    "kickstart".into(),
                    "-k".into(),
                    "gui/501/com.example.resident".into()
                ]
            }
        );
        let mut linux = service(ServiceManager::SystemdSystem);
        linux.privileged = true;
        assert_eq!(
            command(&linux, true),
            NativeCommand {
                program: "/usr/bin/sudo",
                args: vec![
                    "-n".into(),
                    "/usr/bin/systemctl".into(),
                    "restart".into(),
                    "example.service".into()
                ]
            }
        );
        let status = command(&linux, false);
        assert_eq!(status.program, "/usr/bin/systemctl");
        assert!(!status.args.iter().any(|a| a.contains("Environment")));
        let user = service(ServiceManager::SystemdUser);
        assert_eq!(
            command(&user, true).args,
            vec!["--user", "restart", "example.service"]
        );
        for bad in [
            "--all.service",
            "other.service;reboot",
            "../example.service",
            "example*.service",
            "example.socket",
        ] {
            linux.unit = bad.to_owned();
            assert!(linux.validate().is_err(), "{bad}");
        }
        for bad in [
            "system/com.example.resident",
            "gui/501/--all",
            "gui/x/com.example",
            "gui/501/com.example/extra",
        ] {
            let mut cfg = mac.clone();
            cfg.unit = bad.to_owned();
            assert!(cfg.validate().is_err(), "{bad}");
        }
    }
    #[test]
    fn service_control_status_projects_state_and_never_environment() {
        let cfg = service(ServiceManager::Launchd);
        let reply = status_view(
            &cfg,
            Outcome {
                success: true,
                exit_code: Some(0),
                duration_ms: 1,
                stdout: "state = running\npid = 42\nenvironment = {\n TOKEN = private-secret\n}\n"
                    .to_owned(),
            },
        );
        assert_eq!(reply["result"]["active"], true);
        assert!(!reply.to_string().contains("private-secret"));
        let linux = service(ServiceManager::SystemdSystem);
        let reply = status_view(
            &linux,
            Outcome {
                success: true,
                exit_code: Some(0),
                duration_ms: 1,
                stdout:
                    "ActiveState=active\nSubState=running\nMainPID=3\nEnvironment=private-secret\n"
                        .to_owned(),
            },
        );
        assert_eq!(reply["result"]["active"], true);
        assert!(!reply.to_string().contains("private-secret"));
    }
    #[test]
    fn service_control_later_boot_is_observation_not_command_success() {
        let mut t = ticket();
        assert!(!observe(&mut t, "test", 7, 101));
        assert!(!observe(&mut t, "other", 8, 101));
        assert!(observe(&mut t, "test", 8, 102));
        assert_eq!(t.state, RequestState::ObservedRestart);
        assert_eq!(t.exit_code, None);
        let mut failed = ticket();
        failed.state = RequestState::CommandFailed;
        assert!(!observe(&mut failed, "test", 9, 103));
        let mut other = ticket();
        other.self_service = false;
        assert!(!observe(&mut other, "test", 8, 102));
    }
    #[test]
    fn service_control_fake_runner_persists_and_audits_completion() {
        let dir = tempfile::tempdir().unwrap();
        let t = ticket();
        let path = ticket_path(&dir.path().join("tickets"), &t.request_id).unwrap();
        save(&path, &t).unwrap();
        let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
        let cfg = service(ServiceManager::SystemdSystem);
        execute_restart(&path, t, &cfg, &spool, |cmd| {
            assert_eq!(cmd.args, vec!["restart", "example.service"]);
            Ok(Outcome {
                success: true,
                exit_code: Some(0),
                duration_ms: 4,
                stdout: "secret-output-is-not-saved".to_owned(),
            })
        });
        let t = load(&path, "0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(t.state, RequestState::CommandSucceeded);
        assert_eq!(t.exit_code, Some(0));
        let data = fs::read_to_string(&path).unwrap();
        assert!(!data.contains("secret-output"));
        execute_restart(&path, t, &cfg, &spool, |_| Err(()));
        assert_eq!(
            load(&path, "0123456789abcdef0123456789abcdef")
                .unwrap()
                .state,
            RequestState::CommandFailed
        );
        for bad in ["../request", "ABCDEF0123456789ABCDEF0123456789", "abc"] {
            assert!(ticket_path(dir.path(), bad).is_err());
        }
    }
    #[test]
    fn service_control_unconfigured_and_arbitrary_operations_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg=serde_json::from_value(json!({"node":{"id":"test","role":"cognition","listen":"127.0.0.1:0"},"paths":{"root":dir.path()}})).unwrap();
        let shared = Shared::new(
            cfg,
            Spool::new(dir.path().join("spool"), "test").unwrap(),
            7,
            None,
        );
        assert_eq!(
            call(&shared, &json!({"action":"list"})).1["result"]["services"],
            json!([])
        );
        for args in [
            json!({"action":"restart","name":"resident"}),
            json!({"action":"reboot"}),
            json!({"action":"status","name":"resident","command":"sh"}),
            json!({"action":"request_status","request_id":"../secret"}),
        ] {
            assert_eq!(call(&shared, &args).1["ok"], false);
        }
    }
    #[test]
    fn service_control_api_observes_boot_and_deduplicates_without_reexecution() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = serde_json::from_value(json!({
            "node":{"id":"test","role":"cognition","listen":"127.0.0.1:0"},
            "paths":{"root":dir.path()},
            "tools":{"services":[{"name":"resident","manager":"systemd_system","unit":"example.service","self_service":true}]}
        })).unwrap();
        let shared = Shared::new(
            cfg,
            Spool::new(dir.path().join("spool"), "test").unwrap(),
            8,
            None,
        );
        let t = ticket();
        let path = ticket_path(
            &shared.config.paths.current_state().join("service-control"),
            &t.request_id,
        )
        .unwrap();
        save(&path, &t).unwrap();
        let (_, reply) = call(
            &shared,
            &json!({"action":"restart","name":"resident","request_id":t.request_id}),
        );
        assert_eq!(reply["result"]["duplicate"], true);
        let (_, reply) = call(
            &shared,
            &json!({"action":"request_status","request_id":t.request_id}),
        );
        assert_eq!(reply["result"]["state"], "observed_restart");
        assert_eq!(reply["result"]["observed_boot_epoch"], 8);
        assert!(reply["result"]["exit_code"].is_null());
        assert_eq!(
            load(&path, &t.request_id).unwrap().state,
            RequestState::ObservedRestart
        );
    }
    #[test]
    fn service_control_supervisor_exit_is_persisted_before_exit_and_observed_later() {
        use std::cell::Cell;
        let dir = tempfile::tempdir().unwrap();
        let mut t = ticket();
        let path = ticket_path(&dir.path().join("tickets"), &t.request_id).unwrap();
        let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
        let called = Cell::new(false);
        exit_for_supervisor(&path, t.clone(), &spool, |code| {
            assert_eq!(code, 0);
            assert_eq!(
                load(&path, &t.request_id).unwrap().state,
                RequestState::AwaitingSupervisor
            );
            called.set(true);
        })
        .unwrap();
        assert!(called.get());
        t = load(&path, &t.request_id).unwrap();
        assert!(observe(&mut t, "test", 8, 101));
        assert_eq!(t.state, RequestState::ObservedRestart);
        assert_eq!(t.exit_code, None);
        let mut cfg = service(ServiceManager::SystemdSystem);
        cfg.restart_via_exit = true;
        assert!(cfg.validate().is_ok());
        cfg.self_service = false;
        assert!(cfg.validate().is_err());
        t.self_service = false;
        assert!(
            exit_for_supervisor(&path, t, &spool, |_| panic!(
                "must not exit for another service"
            ))
            .is_err()
        );
    }
}
