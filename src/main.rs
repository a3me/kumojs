use kumojs::Compiler;
use serde::Serialize;
use std::path::Path;
use warp::Filter;

#[derive(Serialize)]
struct CompileResponse {
    bytecode: Vec<u8>,
    error: String,
}

fn compile() -> CompileResponse {
    let mut compiler = Compiler::new();

    let compilation = compiler.compile_file(&Path::new("src/scripts/example.js"));

    match compilation {
        Ok(bytecode) => CompileResponse {
            bytecode,
            error: "".to_string(),
        },
        Err(e) => CompileResponse {
            bytecode: [].to_vec(),
            error: e.to_string(),
        },
    }
}

#[tokio::main]
async fn main() {
    let compile_route = warp::path("compile").map(|| warp::reply::json(&compile()));

    let vm_static_path = warp::fs::dir("src");

    let routes = compile_route.or(vm_static_path);

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}
