use kumojs::Compiler;
use serde_json;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

fn main() {
    let mut compiler = Compiler::new();

    let compilation = compiler.compile_file(&Path::new("scripts/example.js"));

    match compilation {
        Ok(bytecode) => {
            let bytecode_json =
                serde_json::to_string(&bytecode).expect("failed to serialize bytecode");

            let mut file = File::create(Path::new("vm/bytecode.json"))
                .expect("failed to create bytecode file");

            file.write_all(bytecode_json.as_bytes())
                .expect("failed to write to bytecode file");

            println!("{:?}", bytecode);
        }
        Err(e) => println!("{:?}", e),
    }
}
