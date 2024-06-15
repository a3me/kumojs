class VM {

    constructor(bytecode) {
        this.bytecode = bytecode;
        this.stack = [];
    }

    run() {
        for (let offset = 0; offset < this.bytecode.length; offset++) {
            let op = this.bytecode[offset];
            switch (op) {
                case 0x01:
                    // load string bytes (null terminated)
                    const stringBytes = [];
                    while (this.bytecode[offset] !== 0x00) {
                        stringBytes.push(this.bytecode[offset]);
                        offset++;
                    }
                    this.stack.push(String.fromCharCode.apply(null, stringBytes));
                    break;
                case 0x02:
                    let number = this.stack.pop();
                    console.log(number);
                    break;
                default:
                    console.log("Unknown opcode: " + op);
            }
        }
    }
}

fetch("bytecode.json")
    .then(response => response.json())
    .then(bytecode => {
        const vm = new VM(bytecode);
        vm.run();
    });
