class VM {

    constructor(bytecode) {
        this.ip = 0;
        this.bytecode = bytecode;
        this.stack = [];
    }

    printStack() {
        console.log(`STACK:${this.stack.slice().join(', ')}`);
    }

    readByte() {
        return this.bytecode[++this.ip];
    }

    readFloat64() {
        const buffer = new ArrayBuffer(8);
        const dataview = new DataView(buffer);
        for (let i = 0; i < 8; i++) {
            dataview.setUint8(i, this.readByte());
        }
        return dataview.getFloat64(0, true);
    }

    readString() {
        const stringBytes = [];
        while (this.ip < this.bytecode.length && this.bytecode[this.ip] !== 0x00) {
            stringBytes.push(this.bytecode[this.ip]);
            this.ip++;
        }
        return String.fromCharCode.apply(null, stringBytes);
    }

    push(value) {
        this.stack.push(value);
    }

    pop() {
        return this.stack.pop();
    }

    peek() {
        return this.stack[this.stack.length - 1];
    }

    run() {
        console.log('bytecode', this.bytecode);

        for (; this.ip < this.bytecode.length; this.ip++) {
            // get current opcode
            const op = this.bytecode[this.ip];
            // switch on opcode and execute operation
            switch (op) {
                case 0x01: {
                    this.push(this.readString());
                    console.log("OP_LOAD_STRING", this.peek());
                    break;
                }
                case 0x02: {
                    const number = this.readFloat64();
                    this.stack.push(number);
                    console.log("OP_LOAD_FLOAT64", this.peek());
                    break;
                }
                case 0x03: {
                    this.stack.push(this.readByte() === 0x01);
                    console.log("OP_LOAD_BOOL", this.peek());
                    break;
                }
                case 0x04: {
                    const popped = this.pop();
                    console.log("OP_POP", popped);
                    break;
                }
                case 0x05: {
                    this.push(null);
                    console.log("OP_NULL", this.peek());
                    break;
                }
                case 0x06: {
                    const exp = this.readString();
                    this.ip++; // skip null terminator
                    const flags = this.readString();
                    this.push(new RegExp(exp, flags));
                    console.log(`OP_REGEX exp=${exp} flags=${flags}`);
                    break;
                }
                default: {
                    console.log("Unknown opcode: " + op);
                    return;
                }
            }
        }
        this.printStack();
    }
}

fetch("bytecode.json")
    .then(response => response.json())
    .then(bytecode => {
        const vm = new VM(bytecode);
        vm.run();
    });
