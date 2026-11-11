//! Dev playground: `cargo run`, then open http://127.0.0.1:3030/
use kumojs::{disasm, Compiler};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use warp::Filter;

#[derive(Deserialize)]
struct CompileRequest {
    source: String,
}

#[derive(Serialize)]
struct CompileResponse {
    bytecode: Vec<u8>,
    disasm: String,
    error: String,
}

fn compile(source: &str) -> CompileResponse {
    match Compiler::new().compile_source("playground.js", source) {
        Ok(bytecode) => CompileResponse {
            disasm: disasm::disassemble(&bytecode).unwrap_or_else(|e| e),
            bytecode,
            error: String::new(),
        },
        Err(e) => CompileResponse {
            bytecode: Vec::new(),
            disasm: String::new(),
            error: e.to_string(),
        },
    }
}

/// Paths of the `.js` files under `dir` (relative to it, `/`-separated), sorted.
fn list_scripts(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &format!("{prefix}{name}/"), out);
            } else if name.ends_with(".js") {
                out.push(format!("{prefix}{name}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

/// The script folders the playground lists and serves.
#[derive(Clone)]
struct ScriptDirs {
    examples: PathBuf,
    fixtures: PathBuf,
    benchmarks: PathBuf,
}

#[derive(Serialize)]
struct Scripts {
    examples: Vec<String>,
    fixtures: Vec<String>,
    benchmarks: Vec<String>,
}

impl ScriptDirs {
    fn list(&self) -> Scripts {
        Scripts {
            examples: list_scripts(&self.examples),
            fixtures: list_scripts(&self.fixtures),
            benchmarks: list_scripts(&self.benchmarks),
        }
    }
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let web = root.join("web");
    let dirs = ScriptDirs {
        examples: web.join("examples"),
        fixtures: root.join("tests/fixtures"),
        benchmarks: root.join("bench/cases"),
    };

    let compile_route = warp::path("compile")
        .and(warp::post())
        .and(warp::body::content_length_limit(1 << 20))
        .and(warp::body::json())
        .map(|request: CompileRequest| warp::reply::json(&compile(&request.source)));

    let list_dirs = dirs.clone();
    let scripts_list = warp::path!("scripts")
        .and(warp::get())
        .map(move || warp::reply::json(&list_dirs.list()));

    // the VM is generated from src/vm/vm.ts by build.rs
    let vm_js = warp::path!("vm.js").and(warp::fs::file(root.join("src/vm/vm.js")));

    let routes = compile_route
        .or(scripts_list)
        .or(vm_js)
        .or(warp::path("examples").and(warp::fs::dir(dirs.examples)))
        .or(warp::path("fixtures").and(warp::fs::dir(dirs.fixtures)))
        .or(warp::path("bench").and(warp::fs::dir(dirs.benchmarks)))
        .or(warp::fs::dir(web));

    println!("kumojs playground: http://127.0.0.1:3030/");
    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}
