//! Compile a script to kumo bytecode: `kumoc <input.js> [output.kumo]` (stdout if no output).
//! `kumoc --disasm <input.js>` prints a readable listing of the bytecode instead.
//! `kumoc --batch <list> <out-dir>` compiles every path listed (one per line) in parallel,
//! writing `<n>.kumo` or the compile error as `<n>.err` for the n-th (test runners).
//! `--opcode-map <map.json>` (before the rest) uses a build's opcode numbering from
//! `tools/build.mjs`. `kumoc --emit-opcodes [out.d.ts]` writes the VM's opcode declarations.
use kumojs::opcode::OpcodeMap;
use kumojs::Compiler;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const USAGE: &str = "usage: kumoc [--opcode-map <map.json>] <input.js> [output.kumo]
       kumoc [--opcode-map <map.json>] --disasm <input.js>
       kumoc [--opcode-map <map.json>] --batch <list> <out-dir>
       kumoc --emit-opcodes [out.d.ts]";

/// Why kumoc stopped: bad arguments (exit 2) or a failure (exit 1).
enum Fail {
    Usage,
    Error(String),
}

impl<E: std::fmt::Display> From<E> for Fail {
    fn from(e: E) -> Self {
        Fail::Error(e.to_string())
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Fail::Error(e)) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), Fail> {
    let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
    let map = match args.as_slice() {
        ["--opcode-map", path, ..] => {
            let map = OpcodeMap::from_json(&std::fs::read_to_string(path)?)?;
            args.drain(..2);
            map
        }
        _ => OpcodeMap::identity(),
    };
    let compiler = Compiler::new().with_opcode_map(map);
    match args.as_slice() {
        ["--emit-opcodes", rest @ ..] if rest.len() <= 1 => {
            let declarations = kumojs::opcode::typescript_declarations();
            write_output(rest.first().copied(), declarations.as_bytes())
        }
        ["--batch", list, out] => batch(&compiler, Path::new(list), Path::new(out)),
        ["--disasm", input] => {
            let bytecode = compiler.compile_file(Path::new(input))?;
            let listing = kumojs::disasm::disassemble_with(&bytecode, compiler.opcode_map())?;
            print!("{listing}");
            Ok(())
        }
        [input] | [input, _] if !input.starts_with("--") => {
            let bytecode = compiler.compile_file(Path::new(input))?;
            write_output(args.get(1).copied(), &bytecode)
        }
        _ => Err(Fail::Usage),
    }
}

/// Write to the file at `path`, or stdout.
fn write_output(path: Option<&str>, bytes: &[u8]) -> Result<(), Fail> {
    match path {
        Some(path) => std::fs::write(path, bytes)?,
        None => std::io::stdout().write_all(bytes)?,
    }
    Ok(())
}

/// Compile every listed file on all cores. Compile errors go in `<n>.err` (they're
/// results); failing to write an output file is reported and fails the run.
fn batch(compiler: &Compiler, list: &Path, out: &Path) -> Result<(), Fail> {
    let list = std::fs::read_to_string(list)?;
    let inputs: Vec<&str> = list.lines().filter(|l| !l.is_empty()).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = AtomicUsize::new(0);
    let write_failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(input) = inputs.get(i) else { break };
                // a parser panic counts as a compile error for that file
                let result =
                    std::panic::catch_unwind(|| compiler.compile_file(Path::new(input)));
                let (path, contents) = match result {
                    Ok(Ok(bytecode)) => (out.join(format!("{i}.kumo")), bytecode),
                    Ok(Err(e)) => (out.join(format!("{i}.err")), e.to_string().into_bytes()),
                    Err(_) => (out.join(format!("{i}.err")), b"compiler panicked".to_vec()),
                };
                if let Err(e) = std::fs::write(&path, contents) {
                    eprintln!("{}: {e}", path.display());
                    write_failed.store(true, Ordering::Relaxed);
                }
            });
        }
    });
    if write_failed.into_inner() {
        return Err(Fail::Error("some outputs could not be written".into()));
    }
    Ok(())
}
