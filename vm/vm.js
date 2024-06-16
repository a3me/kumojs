class VirtualFunction {
    constructor(code) {
        this.code = new Uint8Array(code);
    }
}

class StackFrame {
    constructor(func) {
        this.func = func;
    }
}

class VM {
    constructor(bytecode) {
        // setup virtual functions
        this.functions = [];
        for (const funcCode of bytecode) {
            const func = new VirtualFunction(funcCode);
            this.functions.push(func);
        }

        // init ip and stack 
        this.ip = 0;
        this.stack = [];

        // init call stack with main function
        this.callStack = [new StackFrame(this.functions[0])];
        this.bytecode = this.functions[0].code;
    }

    printStack() {
        console.log(`STACK:${this.stack.slice().join(', ')}`);
    }

    readUInt8() {
        return this.bytecode[++this.ip];
    }

    readUInt16() {
        return this.bytecode[++this.ip] | (this.bytecode[++this.ip] << 8);
    }

    readUInt32() {
        return this.bytecode[++this.ip] |
            (this.bytecode[++this.ip] << 8) |
            (this.bytecode[++this.ip] << 16) |
            (this.bytecode[++this.ip] << 24);
    }

    readFloat64() {
        const dataview = new DataView(new ArrayBuffer(8));
        for (let i = 0; i < 8; i++) {
            dataview.setUint8(i, this.readUInt8());
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
                    this.stack.push(this.readUInt8() === 0x01);
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
                    console.log("OP_NULL");
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
                case 0x07: {
                    this.push(undefined);
                    console.log("OP_UNDEFINED");
                    break;
                }
                case 0x08: {
                    this.callStack.pop();
                    const returnValue = this.pop();
                    if (this.callStack.length === 0) {
                        console.log("OP_RETURN (main)", returnValue);
                        return returnValue;
                    }
                    this.push(returnValue);
                    console.log("OP_RETURN", returnValue);
                    return returnValue;
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