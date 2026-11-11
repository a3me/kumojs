# kumojs

kumojs is an javascript compiler written in rust, and an accompanying virtual machine / interpreter written in javascript.

things to note:

- input is parsed in strict mode (as a module), so features like `with` are not supported.

- generators (`function*`, `yield`) and `for await` are not supported, but `async` / `await` is.

- no `import` / `export`, dynamic `import()`, top-level `await`, `using` declarations or JSX.

- variables must be declared before use to avoid ambiguity between virtual and host propeties, this has impacts in behaviour between real javascript environments and kumojs' interpreter, beware! 

    e.g.
    ```js
    window.console.log(x); // x is undefined, not ok, compiler will error
    ```

    ```js
    var x = 1;
    window.console.log(x); // x is 1, this is fine
    ```

- similarly to the last point, all host globals must be accessed via the window object directly 
    
    e.g.
    ```js
    console.log("🇬🇧"); // incorrect
    ```

    ```js
    window.console.log("🇵🇱"); // correct
    ```

# development / testing

the VM source is `src/vm/vm.ts`; `cargo build` strips its types (needs node 23.2+) into the generated `src/vm/vm.js`. type-check with `npx -p typescript tsc -p .`

playground on http://127.0.0.1:3030/ (edit and run code, pick from `web/examples/`, any `tests/fixtures/` test or a `bench/cases/` benchmark; compares with native)
```sh
cargo run
```

compile a script into bytecode
```sh
cargo run --release --bin kumoc -- in.js out.kumo
```

run benchmarks
```sh
node bench/run.js [filter]
```

kumo disassembly
```sj
kumoc --disasm in.js
```

snapshot tests in `tests/snapshots` 
```sh
KUMO_UPDATE_SNAPSHOTS=1 cargo test --test snapshots # to accept changes
```

test262 javascript support tests
```sh
node tests/test262/run.js [filter] [--failures] [--update] # results in tests/test262/summary.txt
```
