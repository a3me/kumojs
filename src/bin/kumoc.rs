//! Compile a script to kumo bytecode: `kumoc <input.js> [output.kumo]` (stdout if no output).
//! `kumoc --disasm <input.js>` prints a readable listing of the bytecode instead.
//! `kumoc --embed <input.js> [output.js] [--seed N]` writes a standalone JS file: the VM with
//! the program embedded (opcodes shuffled to a random seed unless `--seed` pins one).
//! `kumoc --batch <list> <out-dir>` compiles every path listed (one per line) in parallel,
//! writing `<n>.kumo` or the compile error as `<n>.err` for the n-th (test runners).
use kumojs::Compiler;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--batch") {
        return match (args.get(1), args.get(2)) {
            (Some(list), Some(out)) => batch(Path::new(list), Path::new(out)),
            _ => {
                eprintln!("usage: kumoc --batch <list> <out-dir>");
                ExitCode::from(2)
            }
        };
    }
    if args.first().is_some_and(|a| a == "--embed") {
        return embed(&args[1..]);
    }
    let disasm = args.first().is_some_and(|a| a == "--disasm");
    if disasm {
        args.remove(0);
    }
    let Some(input) = args.first() else {
        eprintln!("usage: kumoc [--disasm] <input.js> [output.kumo]");
        return ExitCode::from(2);
    };
    let bytecode = match Compiler::new().compile_file(Path::new(input)) {
        Ok(bytecode) => bytecode,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if disasm {
        return match kumojs::disasm::disassemble(&bytecode) {
            Ok(listing) => {
                print!("{listing}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }
    let written = match args.get(1) {
        Some(output) => std::fs::write(output, &bytecode),
        None => std::io::stdout().write_all(&bytecode),
    };
    if let Err(e) = written {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `--embed [--seed N] <input.js> [output.js]`: write a standalone VM+program JS file.
fn embed(args: &[String]) -> ExitCode {
    let mut seed: Option<u64> = None;
    let mut positional: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--seed" {
            match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => seed = Some(n),
                None => {
                    eprintln!("usage: kumoc --embed <input.js> [output.js] [--seed N]");
                    return ExitCode::from(2);
                }
            }
        } else {
            positional.push(arg);
        }
    }
    let Some(input) = positional.first() else {
        eprintln!("usage: kumoc --embed <input.js> [output.js] [--seed N]");
        return ExitCode::from(2);
    };
    // a random seed by default, so each build renumbers the opcodes differently
    let seed = seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    });
    let js = match kumojs::embed::compile_embedded_file(Path::new(input), seed) {
        Ok(js) => js,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let written = match positional.get(1) {
        Some(output) => std::fs::write(output, &js),
        None => std::io::stdout().write_all(js.as_bytes()),
    };
    if let Err(e) = written {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn batch(list: &Path, out: &Path) -> ExitCode {
    let list = match std::fs::read_to_string(list) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let inputs: Vec<&str> = list.lines().filter(|l| !l.is_empty()).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(input) = inputs.get(i) else { break };
                // a parser panic counts as a compile error for that file
                let result = std::panic::catch_unwind(|| Compiler::new().compile_file(Path::new(input)));
                let _ = match result {
                    Ok(Ok(bytecode)) => std::fs::write(out.join(format!("{i}.kumo")), bytecode),
                    Ok(Err(e)) => std::fs::write(out.join(format!("{i}.err")), e.to_string()),
                    Err(_) => std::fs::write(out.join(format!("{i}.err")), "compiler panicked"),
                };
            });
        }
    });
    ExitCode::SUCCESS
}
