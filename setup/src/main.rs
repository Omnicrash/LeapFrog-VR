//! `leapfrog-register.exe`: the engine behind the installer. Registers the LeapFrog driver folder
//! this exe sits in with SteamVR and Windows (Settings > Apps), sets up the UltraleapTracking
//! service, and takes all of it out again. The wizard (`leapfrog.nsi`) copies the files and calls
//! this quietly; from a folder of your own, run it directly. No arguments: a menu. Subcommands:
//!
//!   register [--to DIR] [--no-apps-entry]       in place; --to copies the folder there first;
//!                                               --no-apps-entry leaves Settings > Apps alone
//!   unregister [--purge-settings] [--restore-service] [--remove-folder]
//!   status
//!   common: --quiet (no output, no prompts), --pause (wait for Enter at the end)
//!
//! SteamVR's registration (vrpathreg) and the Windows entry are per user, so they happen in this
//! process. Only the service step needs elevation: this exe relaunches itself elevated for that
//! step alone (`service-setup --user-sid SID`, `service-restore`), and a folder that holds this
//! exe is deleted by a temp copy (`delete-folder --folder DIR`). Those three are internal.

mod win;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const SERVICE: &str = "UltraleapTracking";
const DRIVER_NAME: &str = "leapfrog";
/// Under HKCU.
const APP_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\LeapFrog";
/// Query status, start, stop, read control: what the driver needs from the service.
const SERVICE_ACE_RIGHTS: &str = "LCRPWPRC";

/// Wait for Enter before exiting: --pause, or a menu choice; cleared when a temp copy takes over
/// the console, since this exe must exit for its folder to go.
static PAUSE: AtomicBool = AtomicBool::new(false);

struct Options {
    command: String,
    /// Copy the folder there before registering; None registers in place.
    to: Option<PathBuf>,
    no_apps_entry: bool,
    purge_settings: bool,
    restore_service: bool,
    /// Also delete the driver folder (the wizard's uninstaller removes the files itself).
    remove_folder: bool,
    quiet: bool,
    /// delete-folder: the folder to remove.
    folder: Option<PathBuf>,
    /// service-setup: the unelevated user's SID.
    user_sid: Option<String>,
}

fn main() {
    let opts = parse(std::env::args().skip(1).collect());
    let code = match run(&opts) {
        Ok(()) => 0,
        Err(e) => {
            say(&opts, &format!("Error: {e}"));
            1
        }
    };
    if PAUSE.load(Ordering::Relaxed) && !opts.quiet {
        wait_for_enter();
    }
    std::process::exit(code);
}

fn wait_for_enter() {
    println!("\nPress Enter to close.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

fn parse(args: Vec<String>) -> Options {
    let mut o = Options {
        command: String::new(),
        to: None,
        no_apps_entry: false,
        purge_settings: false,
        restore_service: false,
        remove_folder: false,
        quiet: false,
        folder: None,
        user_sid: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "register" | "unregister" | "status" | "service-setup" | "service-restore" | "delete-folder" => o.command = args[i].clone(),
            "--to" => {
                i += 1;
                o.to = args.get(i).map(PathBuf::from);
            }
            "--folder" => {
                i += 1;
                o.folder = args.get(i).map(PathBuf::from);
            }
            "--user-sid" => {
                i += 1;
                o.user_sid = args.get(i).cloned();
            }
            "--no-apps-entry" => o.no_apps_entry = true,
            "--purge-settings" => o.purge_settings = true,
            "--remove-folder" => o.remove_folder = true,
            "--restore-service" => o.restore_service = true,
            "--quiet" => o.quiet = true,
            "--pause" => PAUSE.store(true, Ordering::Relaxed),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    o
}

fn say(o: &Options, text: &str) {
    if !o.quiet {
        println!("{text}");
    }
}

fn run(o: &Options) -> Result<(), String> {
    match o.command.as_str() {
        "status" => {
            print!("{}", status_text()?);
            Ok(())
        }
        "register" => install(o),
        "unregister" => uninstall(o),
        "service-setup" => service_setup(o),
        "service-restore" => service_restore(),
        "delete-folder" => delete_folder(o),
        _ => interactive(),
    }
}

// ---------------------------------------------------------------------------------------------
// Where things are

fn exe_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| "own location unknown".to_string())
}

fn run_cmd(program: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("could not run {}: {e}", program.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

struct SteamVr {
    vrpathreg: PathBuf,
    config_dir: PathBuf,
}

/// SteamVR's locations from %LOCALAPPDATA%\openvr\openvrpaths.vrpath (a small JSON file).
fn steamvr() -> Result<SteamVr, String> {
    let local = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set")?;
    let file = Path::new(&local).join("openvr").join("openvrpaths.vrpath");
    let text = std::fs::read_to_string(&file).map_err(|_| "SteamVR is not installed (no openvrpaths.vrpath)".to_string())?;
    let runtime = json_first_string(&text, "runtime").ok_or("openvrpaths.vrpath has no runtime")?;
    let config = json_first_string(&text, "config").ok_or("openvrpaths.vrpath has no config path")?;
    let vrpathreg = Path::new(&runtime).join("bin").join("win64").join("vrpathreg.exe");
    if !vrpathreg.exists() {
        return Err(format!("vrpathreg not found at {}", vrpathreg.display()));
    }
    Ok(SteamVr { vrpathreg, config_dir: PathBuf::from(config) })
}

/// First string of the array under `key` in a small JSON text; JSON escapes undone.
fn json_first_string(text: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\"");
    let rest = &text[text.find(&k)? + k.len()..];
    let rest = &rest[rest.find('[')? + 1..];
    let start = rest.find('"')? + 1;
    let mut out = String::new();
    let mut chars = rest[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                '"' => out.push('"'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

impl SteamVr {
    /// The registered path of the leapfrog driver, from `vrpathreg show`.
    fn registered(&self) -> Result<Option<PathBuf>, String> {
        let (_, text) = run_cmd(&self.vrpathreg, &["show"])?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix(&format!("{DRIVER_NAME} : ")))
            .map(|p| PathBuf::from(p.trim()))
            .next())
    }

    fn add_driver(&self, dir: &Path) -> Result<(), String> {
        let (ok, text) = run_cmd(&self.vrpathreg, &["adddriver", &dir.to_string_lossy()])?;
        if ok { Ok(()) } else { Err(format!("vrpathreg adddriver failed: {}", text.trim())) }
    }

    fn remove_driver(&self, dir: &Path) -> Result<(), String> {
        let (ok, text) = run_cmd(&self.vrpathreg, &["removedriver", &dir.to_string_lossy()])?;
        if ok { Ok(()) } else { Err(format!("vrpathreg removedriver failed: {}", text.trim())) }
    }
}

fn steamvr_running() -> bool {
    win::process_running("vrserver.exe")
}

// ---------------------------------------------------------------------------------------------
// The tracking service

struct ServiceInfo {
    exists: bool,
    start_type: String,
    state: String,
    sddl: String,
}

fn sc(args: &[&str]) -> Result<(bool, String), String> {
    run_cmd(&win::system_tool("sc.exe"), args)
}

/// Three sc.exe queries: read it once per process and pass it along.
fn service_info() -> Result<ServiceInfo, String> {
    let (ok, qc) = sc(&["qc", SERVICE])?;
    if !ok {
        return Ok(ServiceInfo { exists: false, start_type: String::new(), state: String::new(), sddl: String::new() });
    }
    let field = |text: &str, name: &str| {
        text.lines()
            .find(|l| l.contains(name))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    let (_, query) = sc(&["query", SERVICE])?;
    let (_, sd) = sc(&["sdshow", SERVICE])?;
    Ok(ServiceInfo {
        exists: true,
        start_type: field(&qc, "START_TYPE"),
        state: field(&query, "STATE"),
        sddl: sd.lines().map(str::trim).find(|l| l.starts_with("D:")).unwrap_or("").to_string(),
    })
}

/// A local or domain user's SID (S-1-5-21-...), the only kind the service ACE is meant for;
/// anything else on the elevated command line is refused.
fn valid_user_sid(sid: &str) -> bool {
    sid.strip_prefix("S-1-5-21-").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-') && !rest.ends_with('-') && !rest.contains("--"))
}

fn user_has_service_rights(sddl: &str, sid: &str) -> bool {
    sddl.contains(&format!(";;;{sid})")) && sddl.contains(&format!("(A;;{SERVICE_ACE_RIGHTS};;;{sid})"))
}

/// Already on manual start with the user's rights: nothing to elevate for.
fn service_configured(info: &ServiceInfo, sid: &str) -> bool {
    info.start_type == "DEMAND_START" && user_has_service_rights(&info.sddl, sid)
}

/// Manual start, and start/stop rights for `sid`. Needs an elevated process.
fn configure_service(sid: &str, info: &ServiceInfo) -> Result<(), String> {
    let (ok, text) = sc(&["config", SERVICE, "start=", "demand"])?;
    if !ok {
        return Err(format!("sc config failed (not elevated?): {}", text.trim()));
    }
    if user_has_service_rights(&info.sddl, sid) {
        return Ok(());
    }
    // Append the user's ACE to the DACL; an SACL part (S:...) if present stays after it.
    let (dacl, rest) = match info.sddl.find("S:") {
        Some(i) => info.sddl.split_at(i),
        None => (info.sddl.as_str(), ""),
    };
    let sddl = format!("{dacl}(A;;{SERVICE_ACE_RIGHTS};;;{sid}){rest}");
    let (ok, text) = sc(&["sdset", SERVICE, &sddl])?;
    if ok { Ok(()) } else { Err(format!("sc sdset failed: {}", text.trim())) }
}

// ---------------------------------------------------------------------------------------------
// Windows "Apps" entry

fn register_app(dir: &Path, version: &str) -> Result<(), String> {
    use win::RegValue::{Dword, Sz};
    let exe = dir.join("leapfrog-register.exe");
    let size_kb = (dir_size(dir) / 1024).min(u32::MAX as u64) as u32;
    win::reg_set_values(
        APP_KEY,
        &[
            ("DisplayName", Sz("LeapFrog".into())),
            ("DisplayVersion", Sz(version.into())),
            ("Publisher", Sz("Omnicrash".into())),
            ("InstallLocation", Sz(dir.to_string_lossy().into())),
            ("InstallDate", Sz(win::local_date())),
            ("DisplayIcon", Sz(exe.to_string_lossy().into())),
            ("UninstallString", Sz(format!("\"{}\" unregister --remove-folder --pause", exe.display()))),
            ("QuietUninstallString", Sz(format!("\"{}\" unregister --remove-folder --quiet", exe.display()))),
            ("EstimatedSize", Dword(size_kb)),
            ("NoModify", Dword(1)),
            ("NoRepair", Dword(1)),
        ],
    )
}

fn unregister_app() -> Result<(), String> {
    win::reg_delete_tree(APP_KEY)
}

fn app_registered() -> bool {
    win::reg_has_value(APP_KEY, "InstallLocation")
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            total += if p.is_dir() { dir_size(&p) } else { p.metadata().map(|m| m.len()).unwrap_or(0) };
        }
    }
    total
}

// ---------------------------------------------------------------------------------------------
// Files

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("cannot copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn is_dist_folder(dir: &Path) -> bool {
    dir.join("driver.vrdrivermanifest").exists() && dir.join("bin").join("win64").join("driver_leapfrog.dll").exists()
}

fn version_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("VERSION")).map(|v| v.trim().to_string()).unwrap_or_else(|_| "unknown".to_string())
}

/// Drops `leapfrog-panel.vrmanifest` from SteamVR's registered application manifests
/// (`appconfig.json`, `manifest_paths`), which the panel added for the startup overlay list.
/// With SteamVR closed the file is the registry, and a stale entry would stay listed otherwise.
fn remove_overlay_manifest(config_dir: &Path) -> Result<bool, String> {
    let file = config_dir.join("appconfig.json");
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(false) };
    let mut out = String::with_capacity(text.len());
    let mut removed = false;
    for line in text.split_inclusive('\n') {
        if line.contains("leapfrog-panel.vrmanifest") {
            removed = true;
            // The entry before this one may now carry a trailing comma it must lose.
            if !line.trim_end().ends_with(',') {
                if let Some(comma) = out.trim_end().strip_suffix(',') {
                    let keep = comma.len();
                    out.truncate(keep);
                    out.push('\n');
                }
            }
            continue;
        }
        out.push_str(line);
    }
    if removed {
        std::fs::write(&file, out).map_err(|e| format!("cannot update {}: {e}", file.display()))?;
    }
    Ok(removed)
}

/// Removes the `driver_leapfrog` section from steamvr.vrsettings, after a backup copy.
fn purge_settings(config_dir: &Path) -> Result<bool, String> {
    let file = config_dir.join("steamvr.vrsettings");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let Some(key_at) = text.find("\"driver_leapfrog\"") else { return Ok(false) };
    let Some(open) = text[key_at..].find('{').map(|i| key_at + i) else { return Ok(false) };
    let mut depth = 0;
    let mut close = None;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else { return Ok(false) };
    // Take a following comma, or else a preceding one (last member).
    let mut start = key_at;
    let mut end = close;
    let after = text[close..].trim_start();
    if after.starts_with(',') {
        end = close + (text[close..].len() - after.len()) + 1;
    } else {
        let before = text[..key_at].trim_end();
        if before.ends_with(',') {
            start = before.len() - 1;
        }
    }
    std::fs::copy(&file, file.with_extension("vrsettings.leapfrog.bak")).map_err(|e| format!("backup failed: {e}"))?;
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    std::fs::write(&file, out).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    Ok(true)
}


// ---------------------------------------------------------------------------------------------
// Commands

fn status_text() -> Result<String, String> {
    let mut s = String::new();
    let _ = writeln!(s, "LeapFrog setup");
    match steamvr() {
        Ok(vr) => {
            let _ = writeln!(s, "  SteamVR:   {} ({})", vr.vrpathreg.parent().and_then(Path::parent).and_then(Path::parent).map(|p| p.display().to_string()).unwrap_or_default(), if steamvr_running() { "running" } else { "closed" });
            match vr.registered()? {
                Some(dir) => {
                    let _ = writeln!(s, "  Driver:    registered at {} (version {}){}", dir.display(), version_of(&dir), if dir.exists() { "" } else { ", folder missing" });
                }
                None => {
                    let _ = writeln!(s, "  Driver:    not registered");
                }
            }
        }
        Err(e) => {
            let _ = writeln!(s, "  SteamVR:   {e}");
        }
    }
    let _ = writeln!(s, "  Apps list: {}", if app_registered() { "LeapFrog is listed" } else { "not listed" });
    let info = service_info()?;
    if info.exists {
        let sid = win::current_user_sid().unwrap_or_default();
        let _ = writeln!(s, "  Service:   {SERVICE} {}, {}, user {} start/stop it", info.start_type.to_lowercase(), info.state.to_lowercase(), if user_has_service_rights(&info.sddl, &sid) { "may" } else { "may NOT" });
    } else {
        let _ = writeln!(s, "  Service:   {SERVICE} not installed (Ultraleap Gemini missing)");
    }
    Ok(s)
}

/// Runs one internal subcommand in an elevated copy of this exe and maps its exit code.
fn elevated_step(o: &Options, what: &str, args: &[&str]) -> Result<(), String> {
    say(o, what);
    let mut args = args.to_vec();
    if o.quiet {
        args.push("--quiet");
    }
    let code = win::relaunch_elevated(&args)?;
    if code == 0 { Ok(()) } else { Err(format!("the elevated step exited with code {code}")) }
}

fn install(o: &Options) -> Result<(), String> {
    let vr = steamvr()?;
    if steamvr_running() {
        return Err("SteamVR is running; close it first (the driver file is locked while it runs)".into());
    }
    let service = service_info()?;
    if !service.exists {
        return Err(format!("the {SERVICE} service is not installed: install Ultraleap Gemini first"));
    }
    let source = exe_dir()?;
    if !is_dist_folder(&source) {
        return Err(format!("{} is not a LeapFrog driver folder (driver.vrdrivermanifest, bin\\win64\\driver_leapfrog.dll)", source.display()));
    }
    let sid = win::current_user_sid()?;
    let target = o.to.clone().unwrap_or_else(|| source.clone());
    let same = target.canonicalize().ok() == source.canonicalize().ok();
    if !same {
        if let Some(old) = vr.registered()? {
            if old != target && old.exists() {
                say(o, &format!("Replacing the registration of {}", old.display()));
                let _ = vr.remove_driver(&old);
            }
        }
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
        }
        copy_dir(&source, &target)?;
        say(o, &format!("Copied to {}", target.display()));
    }
    if vr.registered()?.as_deref() != Some(target.as_path()) {
        vr.add_driver(&target)?;
    }
    say(o, "Registered with SteamVR");
    if service_configured(&service, &sid) {
        // Nothing to change: no UAC prompt.
    } else if win::is_elevated() {
        configure_service(&sid, &service)?;
    } else {
        elevated_step(o, "Elevating for the tracking service's settings...", &["service-setup", "--user-sid", &sid])?;
    }
    say(o, &format!("{SERVICE}: manual start, start/stop allowed for this user"));
    if !o.no_apps_entry {
        register_app(&target, &version_of(&target))?;
        say(o, "Listed in Settings > Apps");
    }
    say(o, "\nDone. Start SteamVR; the two Leap hands appear next to your controllers.");
    Ok(())
}

/// Elevated child of `register`: the service step and nothing else.
fn service_setup(o: &Options) -> Result<(), String> {
    let sid = o.user_sid.as_deref().filter(|s| valid_user_sid(s)).ok_or("service-setup needs --user-sid with a user's SID")?;
    let info = service_info()?;
    if !info.exists {
        return Err(format!("the {SERVICE} service is not installed"));
    }
    configure_service(sid, &info)
}

/// Elevated child of `unregister --restore-service`, or its own step when already elevated.
fn service_restore() -> Result<(), String> {
    let (ok, text) = sc(&["config", SERVICE, "start=", "auto"])?;
    if ok { Ok(()) } else { Err(format!("sc config failed: {}", text.trim())) }
}

fn uninstall(o: &Options) -> Result<(), String> {
    let vr = steamvr()?;
    if steamvr_running() {
        return Err("SteamVR is running; close it first".into());
    }
    let registered = vr.registered()?;
    let me = exe_dir()?;
    // A folder to remove: the registered one, or this exe's own when it holds the driver (a
    // previous uninstall that got as far as unregistering).
    let own = me.join("driver.vrdrivermanifest").exists().then(|| me.clone());
    let folder = if o.remove_folder { registered.clone().or(own) } else { None };
    // Never hold the folder open while removing it: a working directory inside it (inherited by
    // the temp copy as well) made remove_dir_all fail on the folder itself.
    let temp = PathBuf::from(std::env::var("TEMP").map_err(|_| "TEMP not set")?);
    let _ = std::env::set_current_dir(&temp);
    // The small registrations first, so a folder that resists leaves nothing else behind.
    if let Some(dir) = &registered {
        vr.remove_driver(dir)?;
        say(o, "Unregistered from SteamVR");
    }
    if remove_overlay_manifest(&vr.config_dir)? {
        say(o, "Removed the settings panel from SteamVR's startup overlay apps");
    }
    unregister_app()?;
    say(o, "Removed from Settings > Apps");
    if o.purge_settings {
        if purge_settings(&vr.config_dir)? {
            say(o, "Removed the driver_leapfrog settings (backup: steamvr.vrsettings.leapfrog.bak)");
        }
    } else {
        say(o, "Settings kept (driver_leapfrog in steamvr.vrsettings); --purge-settings removes them");
    }
    if o.restore_service {
        if win::is_elevated() {
            service_restore()?;
        } else {
            elevated_step(o, "Elevating to restore the service's start type...", &["service-restore"])?;
        }
        say(o, &format!("{SERVICE} back to automatic start"));
    } else {
        say(o, &format!("{SERVICE} left on manual start; --restore-service puts it back to automatic"));
    }
    if let Some(dir) = folder.filter(|d| d.exists()) {
        let inside = match (me.canonicalize(), dir.canonicalize()) {
            (Ok(m), Ok(d)) => m.starts_with(d),
            _ => false,
        };
        if inside {
            // This exe is in the folder: a temp copy deletes it once this process has exited,
            // and takes over the console (and the pause) for that.
            let copy = temp.join("leapfrog-register.exe");
            std::fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &copy).map_err(|e| format!("cannot copy to temp: {e}"))?;
            let mut args: Vec<String> = vec!["delete-folder".into(), "--folder".into(), dir.to_string_lossy().into()];
            if o.quiet {
                args.push("--quiet".into());
            }
            if PAUSE.swap(false, Ordering::Relaxed) {
                args.push("--pause".into());
            }
            Command::new(&copy).args(&args).spawn().map_err(|e| format!("cannot relaunch from temp: {e}"))?;
            return Ok(());
        }
        remove_folder(o, &dir)?;
    }
    say(o, "\nDone.");
    Ok(())
}

fn remove_folder(o: &Options, dir: &Path) -> Result<(), String> {
    std::fs::remove_dir_all(dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    say(o, &format!("Removed {}", dir.display()));
    Ok(())
}

/// Temp copy of `unregister --remove-folder`: deletes the driver folder and nothing else.
fn delete_folder(o: &Options) -> Result<(), String> {
    let dir = o.folder.as_deref().ok_or("delete-folder needs --folder")?;
    if !dir.join("driver.vrdrivermanifest").exists() {
        return Err(format!("{} is not a LeapFrog driver folder", dir.display()));
    }
    // The parent's exe in that folder stays locked until it has exited.
    let mut tries = 0;
    loop {
        match remove_folder(o, dir) {
            Err(_) if tries < 40 && dir.exists() => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            result => break result?,
        }
    }
    say(o, "\nDone.");
    Ok(())
}

fn interactive() -> Result<(), String> {
    print!("{}", status_text()?);
    let here = exe_dir()?;
    let registrable = is_dist_folder(&here);
    println!();
    if registrable {
        println!("  [R] Register this folder with SteamVR");
    }
    println!("  [U] Unregister and remove the driver folder");
    println!("  [Q] Quit");
    print!("> ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let defaults = parse(vec![]);
    match line.trim().to_ascii_lowercase().as_str() {
        "r" if registrable => {
            PAUSE.store(true, Ordering::Relaxed);
            install(&Options { command: "register".into(), ..defaults })
        }
        "u" => {
            PAUSE.store(true, Ordering::Relaxed);
            uninstall(&Options { command: "unregister".into(), remove_folder: true, ..defaults })
        }
        _ => Ok(()),
    }
}
