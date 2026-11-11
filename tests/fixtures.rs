//! Differential tests: every `tests/fixtures/**/*.js` file is compiled with kumojs,
//! run in the VM, and run directly in Node. Console output and thrown error type must match.
//! Each fixture is its own test, so they run in parallel across all cores.
//! A fixture whose first line starts with `// skip:` is reported as ignored. One whose
//! first line is `// compile-error: <text>` must fail to compile with <text> in the error
//! (it isn't run).

use kumojs::Compiler;
use libtest_mimic::{Arguments, Failed, Trial};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

fn collect_fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures(&path, out);
        } else if path.extension().is_some_and(|e| e == "js") {
            out.push(path);
        }
    }
}

fn run_fixture(root: &Path, path: &Path) -> Result<(), Failed> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let name = path.strip_prefix(root).unwrap_or(path).display().to_string();
    let compiled = Compiler::new().compile_source(&name, &source);
    if let Some(expected) = source.lines().next().and_then(|l| l.strip_prefix("// compile-error:")) {
        let expected = expected.trim();
        return match compiled {
            Err(e) if e.to_string().contains(expected) => Ok(()),
            Err(e) => Err(format!("expected compile error containing {expected:?}, got: {e}").into()),
            Ok(_) => Err(format!("expected compile error containing {expected:?}, but it compiled").into()),
        };
    }
    let bytecode = compiled.map_err(|e| format!("compile failed: {e}"))?;

    let bytecode_path = std::env::temp_dir().join(format!(
        "kumo_fixture_{}_{}.kumo",
        std::process::id(),
        name.replace(['/', '\\', '.'], "_")
    ));
    std::fs::write(&bytecode_path, &bytecode).map_err(|e| e.to_string())?;

    let result = run_node(root, path, &bytecode_path);
    let _ = std::fs::remove_file(&bytecode_path);
    let (stdout, stderr) = result?;

    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("bad harness output ({e}):\n{stdout}\n{stderr}"))?;
    if report["pass"].as_bool() == Some(true) {
        return Ok(());
    }
    Err(format!(
        "output differs from node\n  kumo: {}\n  node: {}",
        serde_json::to_string_pretty(&report["kumo"]).unwrap(),
        serde_json::to_string_pretty(&report["node"]).unwrap(),
    )
    .into())
}

/// Run the diff harness with a timeout, returning (stdout, stderr).
fn run_node(root: &Path, source: &Path, bytecode: &Path) -> Result<(String, String), Failed> {
    let mut child = Command::new("node")
        .arg(root.join("../harness/diff.js"))
        .arg(source)
        .arg(bytecode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start node: {e}"))?;

    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let stdout_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout_pipe.read_to_string(&mut s);
        s
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr_pipe.read_to_string(&mut s);
        s
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            return Err(format!("timed out after {TIMEOUT:?}").into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("harness crashed:\n{stdout}\n{stderr}").into());
    }
    Ok((stdout, stderr))
}

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    let mut paths = Vec::new();
    collect_fixtures(&root, &mut paths);
    paths.sort();

    let trials = paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .with_extension("")
                .display()
                .to_string();
            let skip = std::fs::read_to_string(&path)
                .is_ok_and(|s| s.trim_start().starts_with("// skip:"));
            let root = root.clone();
            Trial::test(name, move || run_fixture(&root, &path)).with_ignored_flag(skip)
        })
        .collect();

    libtest_mimic::run(&args, trials).exit();
}
