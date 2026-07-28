//! `verify-deb`: checks that a built `.deb` really does declare every runtime dep it uses.
//!
//! The scanning in `auto_runtime_deb_deps` can only *guess* at deps; this measures the
//! real thing. Install the package, boot the app under `strace`, and check every library
//! it loads and program it spawns against the declared `Depends` closure.
//!
//! Two modes:
//! * Container (default, needs docker or podman): installs the `.deb` into a minimal
//!   image with `Depends` only, which also proves it's installable, then boots it against
//!   a virtual display with software GL. The clean-room check, meant for CI.
//! * `--host`: boots the extracted binary on the build host instead. Weaker, since the
//!   host has extra packages and its own GPU stack, but needs no container.
//!
//! Either way we only *enforce* libs the binary itself references (`DT_NEEDED` + embedded
//! dlopen sonames). Transitive loads are the declared deps' problem, and host GPU vendor
//! dispatch like NVIDIA's `libEGL_nvidia` would otherwise be a false positive. Everything
//! else still gets reported as a warning.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{dt_needed_sonames, find_soname_strings, lib_stem, TempDir, MAKEPAD_OPTIONAL_DLOPEN_STEM_PREFIXES};

pub struct VerifyDebArgs {
    pub deb_path: PathBuf,
    /// Container image to test in, e.g. "ubuntu:24.04". Defaults to the host's distro.
    pub image: Option<String>,
    /// Which binary in the .deb's usr/bin to boot; required only if there are several.
    pub binary_name: Option<String>,
    /// How long to let the app run before concluding it boots fine.
    pub run_secs: u32,
    pub host_mode: bool,
}

/// Everything observed from one traced boot of the app.
#[derive(Default)]
struct Trace {
    /// Successfully-opened `*.so*` paths under /lib or /usr/lib.
    loaded_libs: BTreeSet<String>,
    /// Successfully-exec'd program paths, in order. The harness wrappers and the
    /// app binary itself lead this list and are skipped during analysis.
    execs: Vec<String>,
    /// Successfully-opened paths under /etc or /usr/share.
    data_opens: BTreeSet<String>,
    /// `.so*` paths that failed with ENOENT (dlopen/loader probes).
    failed_lib_probes: BTreeSet<String>,
}

/// The result of running the app in a verification environment.
struct RunOutcome {
    /// Whether `apt` could install the .deb from Depends alone. None in host mode.
    install_ok: Option<bool>,
    /// Exit code of the (timeout-wrapped) app: 124 = ran the full window.
    app_exit: Option<i32>,
    /// The app's own stdout/stderr, so a crash can be diagnosed from the report.
    app_log: String,
    trace: Trace,
    /// Recursive closure of the declared Depends (plus the package itself).
    closure: BTreeSet<String>,
    /// Essential/required packages: implicit dependencies per Debian Policy.
    implicit: BTreeSet<String>,
    /// Closure of the test harness packages (strace/Xvfb/mesa); empty in host mode.
    harness: BTreeSet<String>,
    /// Normalized file path -> owning package.
    file_owner: BTreeMap<String, String>,
}

pub fn verify_deb(args: &VerifyDebArgs) -> std::io::Result<()> {
    let deb = fs::canonicalize(&args.deb_path)?;
    let pkg = deb_field(&deb, "Package")?;
    let depends = deb_field(&deb, "Depends")?;
    let depends_roots = parse_depends(&depends);

    // Extract the .deb to find the app binary and the sonames it references.
    let extract_dir = TempDir::new("rpc-verify-extract-")?;
    run_checked(Command::new("dpkg-deb").arg("-x").arg(&deb).arg(extract_dir.path()))?;
    let binary = find_app_binary(extract_dir.path(), args.binary_name.as_deref())?;
    let binary_name = binary.file_name().unwrap().to_string_lossy().into_owned();
    let referenced = referenced_lib_stems(&binary)?;

    println!("verify-deb: {}", deb.display());
    println!("  package: {pkg}, binary: {binary_name}, declared Depends: {}", depends_roots.len());

    let outcome = if args.host_mode {
        run_on_host(&pkg, &binary, &depends_roots, args.run_secs)?
    } else {
        run_in_container(&deb, &pkg, &binary_name, args.run_secs, args.image.as_deref())?
    };

    let pass = analyze_and_report(&outcome, &referenced, &binary_name, args.run_secs);
    if pass {
        println!("VERDICT: PASS");
        Ok(())
    } else {
        println!("VERDICT: FAIL");
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "verify-deb found dependency problems (see report above)",
        ))
    }
}

//------------------------------------------------------------------------------
// Container mode
//------------------------------------------------------------------------------

/// The script run inside the container (bash, positional args: package, binary, secs).
/// It writes all of its artifacts into the bind-mounted `/out` directory.
const CONTAINER_SCRIPT: &str = r#"#!/bin/bash
set -u
PKG="$1"; BIN="$2"; SECS="$3"
export DEBIAN_FRONTEND=noninteractive
REC="--no-recommends --no-suggests --no-conflicts --no-breaks --no-replaces --no-enhances"

echo "== [container] apt-get update" >&2
apt-get update -qq >/dev/null 2>&1

echo "== [container] installing the package with Depends only (no Recommends)" >&2
if apt-get install -y -qq --no-install-recommends /pkg.deb >/out/install.log 2>&1; then
    echo ok >/out/install_status
else
    echo fail >/out/install_status
    exit 0
fi
apt-cache depends --recurse $REC "$PKG" 2>/dev/null | grep -E '^[A-Za-z0-9]' | sort -u >/out/app_closure.txt

# A GUI app needs a session to boot into, not just its libraries: a display, a GL
# driver behind libEGL/libGL, and a sound server. Toolkits routinely hard-fail when
# any of these is missing, which would look like a dependency problem but isn't.
HARNESS="strace xvfb libgl1-mesa-dri libegl-mesa0 libglx-mesa0 pulseaudio"
echo "== [container] installing the test harness ($HARNESS)" >&2
apt-get install -y -qq $HARNESS >>/out/install.log 2>&1
apt-cache depends --recurse $REC $HARNESS 2>/dev/null | grep -E '^[A-Za-z0-9]' | sort -u >/out/harness_closure.txt

# Essential/required packages are implicit dependencies per Debian Policy.
dpkg-query -W -f='${Package}\t${Essential}\t${Priority}\n' \
    | awk -F'\t' '$2=="yes" || $3=="required" {print $1}' | sort -u >/out/implicit.txt

# Map every installed file to its owning package.
for f in /var/lib/dpkg/info/*.list; do
    p=$(basename "$f" .list); p=${p%%:*}
    sed "s|^|$p |" "$f"
done >/out/files.map

echo "== [container] booting $BIN under strace for ${SECS}s" >&2
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
# System mode, since we're root here and PulseAudio refuses to run as root otherwise.
# Null sink and source included: a container has no audio hardware, and toolkits tend to
# assume a default device exists, aborting inside libpulse when an operation returns null.
pulseaudio --system --daemonize --exit-idle-time=-1 --disallow-exit \
    --load="module-null-sink sink_name=dummy" \
    --load="module-null-source source_name=dummy_source" >/dev/null 2>&1 || true
sleep 2
# timeout runs *inside* strace so strace exits naturally when the window closes.
strace -o /out/trace.log -f -qq -s 256 -e trace=openat,openat2,execve,execveat \
    timeout "$SECS" env DISPLAY=:99 HOME=/root PULSE_SERVER=unix:/run/pulse/native "$BIN" \
    >/out/app.log 2>&1
echo $? >/out/app_exit
"#;

fn run_in_container(
    deb: &Path,
    pkg: &str,
    binary_name: &str,
    run_secs: u32,
    image: Option<&str>,
) -> std::io::Result<RunOutcome> {
    let engine = ["docker", "podman"]
        .into_iter()
        .find(|e| Command::new(e).arg("--version").output().is_ok_and(|o| o.status.success()))
        .ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "container mode needs docker or podman; install one, or re-run with --host",
        ))?;
    let image = image.map(str::to_string).unwrap_or_else(host_matching_image);

    let out = TempDir::new("rpc-verify-out-")?;
    fs::write(out.path().join("verify.sh"), CONTAINER_SCRIPT)?;

    println!("  environment: {image} via {engine}");
    let status = Command::new(engine)
        .args(["run", "--rm", "--cap-add=SYS_PTRACE"])
        .arg("-v").arg(format!("{}:/pkg.deb:ro", deb.display()))
        .arg("-v").arg(format!("{}:/out", out.path().display()))
        .arg(&image)
        .args(["bash", "/out/verify.sh", pkg])
        .arg(format!("/usr/bin/{binary_name}"))
        .arg(run_secs.to_string())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{engine} run exited with {status}"),
        ));
    }

    let read_lines = |name: &str| -> BTreeSet<String> {
        fs::read_to_string(out.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    };
    let install_ok = fs::read_to_string(out.path().join("install_status"))
        .map(|s| s.trim() == "ok")
        .unwrap_or(false);
    if !install_ok {
        // Show why apt refused; this is itself a verification result.
        let log = fs::read_to_string(out.path().join("install.log")).unwrap_or_default();
        for line in log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev() {
            println!("  [apt] {line}");
        }
    }

    let mut closure = read_lines("app_closure.txt");
    closure.insert(pkg.to_string());
    let file_owner = fs::read_to_string(out.path().join("files.map"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(p, path)| (normalize_path(path), p.to_string()))
        .collect();

    Ok(RunOutcome {
        install_ok: Some(install_ok),
        app_exit: fs::read_to_string(out.path().join("app_exit")).ok().and_then(|s| s.trim().parse().ok()),
        app_log: fs::read_to_string(out.path().join("app.log")).unwrap_or_default(),
        trace: parse_strace(&fs::read_to_string(out.path().join("trace.log")).unwrap_or_default()),
        closure,
        implicit: read_lines("implicit.txt"),
        harness: read_lines("harness_closure.txt"),
        file_owner,
    })
}

/// Picks a container image matching the build host, so the test distro's package
/// names/versions line up with what dpkg-shlibdeps computed at build time.
fn host_matching_image() -> String {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let get = |key: &str| os_release.lines()
        .find_map(|l| l.strip_prefix(key))
        .map(|v| v.trim().trim_matches('"').to_string());
    match (get("ID="), get("VERSION_ID=")) {
        (Some(id), Some(ver)) if id == "ubuntu" || id == "debian" => format!("{id}:{ver}"),
        _ => "debian:stable-slim".to_string(),
    }
}

//------------------------------------------------------------------------------
// Host mode
//------------------------------------------------------------------------------

fn run_on_host(
    pkg: &str,
    binary: &Path,
    depends_roots: &[String],
    run_secs: u32,
) -> std::io::Result<RunOutcome> {
    if !Command::new("strace").arg("--version").output().is_ok_and(|o| o.status.success()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "host mode needs strace; install it with 'sudo apt-get install strace'",
        ));
    }

    let mut closure = apt_depends_closure(depends_roots)?;
    closure.insert(pkg.to_string());
    let implicit = host_implicit_packages()?;

    let out = TempDir::new("rpc-verify-out-")?;
    let trace_log = out.path().join("trace.log");
    let app_log = fs::File::create(out.path().join("app.log"))?;

    // Prefer an isolated virtual display (no window flashing on the user's desktop,
    // and it forces the X11 path like the container does); fall back to the live session.
    let have_xvfb = Command::new("xvfb-run").arg("--help").output().is_ok_and(|o| o.status.success());
    let display_desc = if have_xvfb { "Xvfb virtual display" } else { "live session display" };
    println!("  environment: build host, {display_desc}");
    if !have_xvfb {
        println!("  note: xvfb-run not installed; the app window may appear briefly.");
    }

    let mut cmd = if have_xvfb {
        let mut c = Command::new("xvfb-run");
        c.args(["-a", "strace"]);
        c
    } else {
        Command::new("strace")
    };
    // timeout runs *inside* strace so strace exits naturally when the window closes.
    let status = cmd
        .arg("-o").arg(&trace_log)
        .args(["-f", "-qq", "-s", "256", "-e", "trace=openat,openat2,execve,execveat"])
        .args(["timeout", &run_secs.to_string()])
        .arg(binary)
        .stdout(app_log.try_clone()?)
        .stderr(app_log)
        .status()?;

    let trace = parse_strace(&fs::read_to_string(&trace_log).unwrap_or_default());
    let file_owner = host_file_owners(
        trace.loaded_libs.iter()
            .chain(trace.execs.iter())
            .chain(trace.data_opens.iter()),
    )?;

    Ok(RunOutcome {
        install_ok: None,
        app_exit: status.code(),
        app_log: fs::read_to_string(out.path().join("app.log")).unwrap_or_default(),
        trace,
        closure,
        implicit,
        harness: BTreeSet::new(),
        file_owner,
    })
}

/// Recursive Depends closure of the given packages, per the host's apt metadata.
fn apt_depends_closure(roots: &[String]) -> std::io::Result<BTreeSet<String>> {
    if roots.is_empty() {
        return Ok(BTreeSet::new());
    }
    let output = Command::new("apt-cache")
        .args([
            "depends", "--recurse", "--no-recommends", "--no-suggests",
            "--no-conflicts", "--no-breaks", "--no-replaces", "--no-enhances",
        ])
        .args(roots)
        .output()?;
    // apt-cache prints recursed package names unindented; dependency lines are indented.
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()))
        .map(|l| l.trim().to_string())
        .collect())
}

/// Essential/required packages: implicit dependencies that a .deb need not declare.
fn host_implicit_packages() -> std::io::Result<BTreeSet<String>> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f", "${Package}\t${Essential}\t${Priority}\n"])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let pkg = f.next()?;
            let essential = f.next().unwrap_or("");
            let priority = f.next().unwrap_or("");
            (essential == "yes" || priority == "required").then(|| pkg.to_string())
        })
        .collect())
}

/// Maps the given paths to their owning packages with batched `dpkg -S` calls,
/// querying both usr-merge alias forms of each path.
fn host_file_owners<'a>(
    paths: impl Iterator<Item = &'a String>,
) -> std::io::Result<BTreeMap<String, String>> {
    let mut candidates = Vec::new();
    for p in paths {
        candidates.push(p.clone());
        candidates.push(usr_merge_alias(p));
    }
    let mut owners = BTreeMap::new();
    for chunk in candidates.chunks(100) {
        // Non-zero exit just means some paths matched nothing; parse what did match.
        let output = Command::new("dpkg").arg("-S").args(chunk).output()?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.starts_with("diversion") {
                continue;
            }
            let Some((pkgs, path)) = line.split_once(": ") else { continue };
            let Some(pkg) = pkgs.split([':', ',']).next() else { continue };
            owners.insert(normalize_path(path.trim()), pkg.to_string());
        }
    }
    Ok(owners)
}

//------------------------------------------------------------------------------
// Shared analysis
//------------------------------------------------------------------------------

/// Prints the verification report; returns whether the .deb passed.
fn analyze_and_report(
    o: &RunOutcome,
    referenced: &BTreeSet<String>,
    binary_name: &str,
    run_secs: u32,
) -> bool {
    let mut pass = true;

    // Packages that must be added to the .deb's Depends to make this verification pass.
    let mut missing_pkgs: BTreeSet<String> = BTreeSet::new();

    if let Some(ok) = o.install_ok {
        println!("  install from Depends alone: {}", if ok { "OK" } else { "FAILED" });
        if !ok {
            println!();
            println!("HOW TO FIX");
            println!("  apt could not satisfy this package's declared dependencies (see the");
            println!("  [apt] output above). Usually one of:");
            println!("    * a dependency does not exist under that name on the test distro");
            println!("      (e.g. `libfoo1t64` does not exist on older releases) -- build the");
            println!("      .deb on the OLDEST distro you intend to support, or test a distro");
            println!("      that matches your build host via --image <distro:version>;");
            println!("    * a version constraint is unsatisfiable on the test distro.");
            return false;
        }
    }

    // 124 = killed by our timeout, i.e. it ran the whole window without crashing.
    let boot_ok = matches!(o.app_exit, Some(124) | Some(0));
    match o.app_exit {
        Some(124) => println!("  app boot: OK (ran the full {run_secs}s window)"),
        Some(0) => println!("  app boot: OK (exited cleanly)"),
        Some(code) => println!("  app boot: CRASHED (exit code {code})"),
        None => println!("  app boot: could not determine exit status"),
    }
    // The app's own output is the only way to tell a missing dependency apart from an
    // app-level failure, so show it whenever the boot didn't go cleanly.
    if !boot_ok && !o.app_log.trim().is_empty() {
        println!("  --- app output (last 20 lines) ---");
        let lines: Vec<&str> = o.app_log.lines().collect();
        for line in lines.iter().skip(lines.len().saturating_sub(20)) {
            println!("  | {line}");
        }
        println!("  --- end app output ---");
    }
    // Whether a crash counts as a packaging failure is decided at the end, once we know
    // whether any dependency evidence actually explains it.

    let covered: BTreeSet<&String> = o.closure.union(&o.implicit).collect();
    let owner_of = |path: &str| o.file_owner.get(&normalize_path(path));

    // Enforced check: every loaded library the binary itself references must be
    // owned by a package in the Depends closure.
    let mut ok_count = 0usize;
    let mut undeclared: Vec<String> = Vec::new();
    let mut harness_only: Vec<String> = Vec::new();
    let mut unowned: Vec<String> = Vec::new();
    let mut other_uncovered: Vec<String> = Vec::new();
    for lib in &o.trace.loaded_libs {
        let stem = Path::new(lib).file_name()
            .map(|f| lib_stem(&f.to_string_lossy()).to_string())
            .unwrap_or_default();
        let is_referenced = referenced.contains(&stem);
        match owner_of(lib) {
            Some(pkg) if covered.contains(pkg) => ok_count += 1,
            Some(pkg) if o.harness.contains(pkg) => {
                if is_referenced {
                    harness_only.push(format!("{lib} ({pkg})"));
                    missing_pkgs.insert(pkg.clone());
                }
            }
            Some(pkg) => {
                if is_referenced {
                    undeclared.push(format!("{lib} -> package {pkg}"));
                    missing_pkgs.insert(pkg.clone());
                } else {
                    other_uncovered.push(format!("{lib} ({pkg})"));
                }
            }
            None => {
                if is_referenced {
                    unowned.push(lib.clone());
                }
            }
        }
    }

    println!("  library loads: {} distinct, {ok_count} covered by the Depends closure", o.trace.loaded_libs.len());
    if !undeclared.is_empty() {
        pass = false;
        println!("  ✗ UNDECLARED dependencies (loaded, referenced by the binary, not in the closure):");
        for l in &undeclared {
            println!("      {l}");
        }
    }
    if !harness_only.is_empty() {
        pass = false;
        println!("  ✗ referenced libraries provided only by the test harness (a user system may lack them):");
        for l in &harness_only {
            println!("      {l}");
        }
    }
    for l in &unowned {
        println!("  ⚠ referenced library loaded from an unpackaged path: {l}");
    }

    // Referenced libraries that were probed but never found.
    let loaded_stems: BTreeSet<String> = o.trace.loaded_libs.iter()
        .filter_map(|l| Path::new(l).file_name().map(|f| lib_stem(&f.to_string_lossy()).to_string()))
        .collect();
    let missing_probes: BTreeSet<String> = o.trace.failed_lib_probes.iter()
        .filter_map(|p| Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()))
        .filter(|name| {
            let stem = lib_stem(name).to_string();
            referenced.contains(&stem) && !loaded_stems.contains(&stem)
        })
        .collect();
    if !missing_probes.is_empty() {
        pass = false;
        println!("  ✗ referenced libraries that could NOT be found at runtime:");
        for name in &missing_probes {
            println!("      {name}");
        }
    }

    // Spawned programs must be covered too. Everything up to the app's own exec is the
    // harness wrapper chain (strace, timeout, xvfb-run), so skip past it -- but if we
    // can't find that exec, check everything: wrapper noise beats missing real spawns.
    let app_spawns = o.trace.execs.iter()
        .position(|e| Path::new(e).file_name().is_some_and(|f| f == binary_name))
        .map_or(&o.trace.execs[..], |i| &o.trace.execs[i + 1..]);
    for exe in BTreeSet::from_iter(app_spawns) {
        match owner_of(exe) {
            Some(pkg) if covered.contains(pkg) => println!("  spawned program covered: {exe} ({pkg})"),
            Some(pkg) if o.harness.contains(pkg) => {}
            Some(pkg) => {
                pass = false;
                println!("  ✗ UNDECLARED spawned program: {exe} -> package {pkg}");
                missing_pkgs.insert(pkg.clone());
            }
            None => println!("  ⚠ spawned program from an unpackaged path: {exe}"),
        }
    }

    // Informational: non-referenced loads and data files outside the closure.
    if !other_uncovered.is_empty() {
        println!("  ⚠ loads outside the closure but not referenced by the binary (transitive/vendor; informational):");
        for l in other_uncovered.iter().take(10) {
            println!("      {l}");
        }
    }
    // Data files are informational rather than enforced: most are opened
    // opportunistically, so the owning package is printed for you to judge.
    let data_uncovered: Vec<String> = o.trace.data_opens.iter()
        .filter_map(|p| owner_of(p)
            .filter(|pkg| !covered.contains(pkg) && !o.harness.contains(*pkg))
            .map(|pkg| format!("{p} ({pkg})")))
        .collect();
    if !data_uncovered.is_empty() {
        println!("  ⚠ data files opened outside the closure (informational):");
        for p in data_uncovered.iter().take(25) {
            println!("      {p}");
        }
    }

    // A crash is only a packaging failure if some dependency evidence explains it -- a
    // missing library, or something loaded that nothing declares. With all of that clean,
    // the app died for its own reasons (no audio device, no GPU, no config), and failing
    // on that would make this check useless anywhere but a full desktop session.
    if !boot_ok {
        if pass {
            println!("  ⚠ the app did not run cleanly, but everything it loaded and spawned");
            println!("    was covered by the declared dependencies, so this looks like an");
            println!("    app-level or environment problem rather than a packaging one.");
            println!("    Not failing on it -- see the app output above.");
        } else {
            println!("  note: the crash above is likely explained by the dependency problems listed.");
        }
    }

    if !pass {
        print_remediation(&missing_pkgs, &missing_probes, boot_ok, binary_name);
    }
    pass
}


/// Prints the copy-pasteable fix for whatever made the verification fail.
fn print_remediation(
    missing_pkgs: &BTreeSet<String>,
    missing_probes: &BTreeSet<String>,
    boot_ok: bool,
    binary_name: &str,
) {
    println!();
    println!("HOW TO FIX");

    if !missing_pkgs.is_empty() {
        println!("  Declare the missing package(s) by appending to the");
        println!("  `before-each-package-command` in your Cargo.toml:");
        println!();
        println!("      robius-packaging-commands before-each-package \\");
        println!("          --binary-name {binary_name} \\");
        println!("          --path-to-binary ./target/release/{binary_name} \\");
        let last = missing_pkgs.len() - 1;
        for (i, pkg) in missing_pkgs.iter().enumerate() {
            let cont = if i == last { "" } else { " \\" };
            println!("          --add-deb-dep {pkg}{cont}");
        }
        println!();
        println!("  Then rebuild and re-verify:");
        println!();
        println!("      cargo packager --release --formats deb");
        println!("      robius-packaging-commands verify-deb --deb ./dist/<your>.deb");
        println!();
        println!("  Note: these are normally detected automatically. Anything listed here");
        println!("  is a gap in auto-detection -- worth reporting so the tool can find it");
        println!("  for every app rather than each app declaring it by hand.");
    }

    if !missing_probes.is_empty() {
        println!("  The app looked for these libraries and did not find them anywhere:");
        for name in missing_probes {
            println!("      {name}");
        }
        println!("  They are not installed in the test environment, so their Debian package");
        println!("  could not be determined automatically. Identify the providing package");
        println!("  (`apt-file search <soname>`) and add it with --add-deb-dep.");
    }

    if !boot_ok {
        println!("  The app did not run successfully with only its declared dependencies.");
        println!("  Check the failure above: if it is not a missing library, it may be an");
        println!("  app-level error (missing resources, no display) rather than a packaging bug.");
    }
}

//------------------------------------------------------------------------------
// strace parsing and small helpers
//------------------------------------------------------------------------------

/// Parses `strace -f` output into the sets we care about. Handles the
/// `<unfinished ...>` / `<... resumed>` line pairs that `-f` produces.
fn parse_strace(log: &str) -> Trace {
    let mut trace = Trace::default();
    // (pid, syscall) -> path of an unfinished call.
    let mut pending: BTreeMap<(String, String), String> = BTreeMap::new();

    for line in log.lines() {
        // Lines look like: `1234  openat(AT_FDCWD, "/path", O_RDONLY) = 3`.
        let (pid, rest) = split_pid(line);
        let rest = rest.trim_start();

        if let Some(resumed_at) = rest.find(" resumed>") {
            let syscall = rest[..resumed_at].trim_start_matches("<... ").to_string();
            if let Some(path) = pending.remove(&(pid.to_string(), syscall.clone())) {
                record(&mut trace, &syscall, &path, ret_of(rest));
            }
            continue;
        }
        let Some(paren) = rest.find('(') else { continue };
        let syscall = &rest[..paren];
        if !matches!(syscall, "openat" | "openat2" | "execve" | "execveat") {
            continue;
        }
        let Some(path) = first_quoted(rest) else { continue };
        if rest.contains("<unfinished") {
            pending.insert((pid.to_string(), syscall.to_string()), path.to_string());
            continue;
        }
        record(&mut trace, syscall, path, ret_of(rest));
    }
    trace
}

/// Records one completed syscall into the trace.
fn record(trace: &mut Trace, syscall: &str, path: &str, ret: Option<&str>) {
    let Some(ret) = ret else { return };
    let success = match syscall {
        "execve" | "execveat" => ret == "0",
        _ => ret.chars().next().is_some_and(|c| c.is_ascii_digit()),
    };
    if success {
        match syscall {
            "execve" | "execveat" => {
                trace.execs.push(path.to_string());
            }
            _ if (path.starts_with("/lib") || path.starts_with("/usr/lib")) && path.contains(".so") => {
                trace.loaded_libs.insert(path.to_string());
            }
            _ if path.starts_with("/etc") || path.starts_with("/usr/share") => {
                trace.data_opens.insert(path.to_string());
            }
            _ => {}
        }
    } else if ret.contains("ENOENT") && path.contains(".so") {
        trace.failed_lib_probes.insert(path.to_string());
    }
}

/// Splits a leading numeric pid (as produced by `strace -f`) off the line.
fn split_pid(line: &str) -> (&str, &str) {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(line.len());
    line.split_at(end)
}

/// The first double-quoted string in the line (strace prints paths quoted).
fn first_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')? + 1;
    let len = s[start..].find('"')?;
    Some(&s[start..start + len])
}

/// The syscall return value: everything after the last `) = `, with any trailing
/// explanation stripped, e.g. `3`, or `-1 ENOENT` from `-1 ENOENT (No such file...)`.
fn ret_of(s: &str) -> Option<&str> {
    let rest = &s[s.rfind(") = ")? + 4..];
    Some(rest.split('(').next().unwrap_or(rest).trim())
}

/// Treats `/usr/X` and `/X` as the same path (usr-merged systems mix both forms).
fn normalize_path(path: &str) -> String {
    path.strip_prefix("/usr").unwrap_or(path).to_string()
}

/// The usr-merge sibling of a path: `/lib/x` <-> `/usr/lib/x`.
fn usr_merge_alias(path: &str) -> String {
    match path.strip_prefix("/usr") {
        Some(stripped) => stripped.to_string(),
        None => format!("/usr{path}"),
    }
}

/// Reads one control field from a .deb via `dpkg-deb -f`.
fn deb_field(deb: &Path, field: &str) -> std::io::Result<String> {
    let output = Command::new("dpkg-deb").arg("-f").arg(deb).arg(field).output()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "dpkg-deb not found; install it with 'sudo apt-get install dpkg'",
            )
        } else {
            e
        })?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("dpkg-deb -f {} {field} failed", deb.display()),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Package names from a Depends field, version constraints and alternatives stripped.
fn parse_depends(depends: &str) -> Vec<String> {
    depends
        .split(',')
        .flat_map(|d| d.split('|'))
        .filter_map(|d| d.trim().split([' ', '(']).next())
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect()
}

/// Finds the app's main binary among the .deb's usr/bin entries.
fn find_app_binary(extract_dir: &Path, binary_name: Option<&str>) -> std::io::Result<PathBuf> {
    let bin_dir = extract_dir.join("usr").join("bin");
    let entries: Vec<PathBuf> = fs::read_dir(&bin_dir)
        .map_err(|_| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the .deb contains no usr/bin directory",
        ))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    match binary_name {
        Some(name) => entries.iter()
            .find(|p| p.file_name().is_some_and(|f| f == name))
            .cloned()
            .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no binary named {name:?} in the .deb's usr/bin"),
            )),
        None => match entries.as_slice() {
            [single] => Ok(single.clone()),
            [] => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no binaries in the .deb's usr/bin")),
            multiple => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "multiple binaries in the .deb's usr/bin ({}); pick one with --binary-name",
                    multiple.iter().filter_map(|p| p.file_name()).map(|f| f.to_string_lossy()).collect::<Vec<_>>().join(", "),
                ),
            )),
        },
    }
}

/// Library stems the binary references: `DT_NEEDED` sonames plus embedded dlopen strings.
///
/// Makepad's optional libs are excluded here regardless of whether this is a Makepad app.
/// We're verifying a built `.deb` and have no Cargo project to detect that from, and
/// excluding one just means we don't *enforce* it, which is the lenient direction -- a
/// false CI failure is worse than a miss in what's already a backstop check.
fn referenced_lib_stems(binary: &Path) -> std::io::Result<BTreeSet<String>> {
    let mut stems: BTreeSet<String> = dt_needed_sonames(binary)?
        .iter()
        .map(|s| lib_stem(s).to_string())
        .collect();
    for soname in find_soname_strings(&fs::read(binary)?).into_keys() {
        let stem = lib_stem(&soname).to_string();
        if !MAKEPAD_OPTIONAL_DLOPEN_STEM_PREFIXES.iter().any(|p| stem.starts_with(p)) {
            stems.insert(stem);
        }
    }
    Ok(stems)
}

/// Runs a command, turning a non-zero exit into an error.
fn run_checked(cmd: &mut Command) -> std::io::Result<()> {
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{cmd:?} exited with {status}"),
        ))
    }
}
