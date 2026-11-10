const fs = require('fs');
const { VM } = require('./vm');

// simulate a browser: tests are written against `window`
globalThis.window = globalThis;

function main() {
    const args = process.argv.slice(2);
    if (args.length < 1) {
        console.error("Usage: node test_runner.js <bytecode_file>");
        process.exit(1);
    }

    const bytecodePath = args[0];

    try {
        const bytecodeBuffer = fs.readFileSync(bytecodePath);
        // Convert Buffer to Uint8Array/Array for the VM
        const bytecode = new Uint8Array(bytecodeBuffer);
        // VM expects an array of bytecodes (one for each function)
        const vm = new VM([bytecode], {
            debug: false,
            debugLog: console.error
        });

        vm.run();

        // Execute optional verification script
        if (args.length > 1) {
            const verificationScript = args[1];
            try {
                eval(verificationScript);
            } catch (e) {
                console.error("Error executing verification script:", e);
                process.exit(1);
            }
        }

        // Print the last value on the stack
        if (vm.stack.length > 0) {
            console.log(vm.stack[vm.stack.length - 1]);
        } else {
            console.log("undefined");
        }

    } catch (e) {
        console.error("Error executing VM:", e);
        process.exit(1);
    }
}

main();
