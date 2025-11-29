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
    constructor(bytecode, options) {

        this.options = options || {
            debug: false,
            debugLog: undefined
        };

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

    log(...data) {
        console.log(...data);
        if (this.options.debug && this.options.debugLog) {
            this.options.debugLog(...data);
        }
    }

    printStack() {
        this.log(`STACK:${this.stack.slice().join(', ')}`);
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
        for (; this.ip < this.bytecode.length; this.ip++) {
            // get current opcode
            const op = this.bytecode[this.ip];

            // switch on opcode and execute operation
            switch (op) {
                // Operation::LoadString(_) => 0x01,
                case 0x01: {
                    this.push(this.readString());
                    this.log("OP_LOAD_STRING", this.peek());
                    break;
                }
                // Operation::LoadFloat64(_) => 0x02,
                case 0x02: {
                    const number = this.readFloat64();
                    this.stack.push(number);
                    this.log("OP_LOAD_FLOAT64", this.peek());
                    break;
                }
                // Operation::Bool(_) => 0x03,
                case 0x03: {
                    this.stack.push(this.readUInt8() === 0x01);
                    this.log("OP_LOAD_BOOL", this.peek());
                    break;
                }
                // Operation::Pop => 0x04,
                case 0x04: {
                    const popped = this.pop();
                    this.log("OP_POP", popped);
                    break;
                }
                // Operation::Null => 0x05,
                case 0x05: {
                    this.push(null);
                    this.log("OP_NULL");
                    break;
                }
                // Operation::Regex(_, _) => 0x06,
                case 0x06: {
                    const exp = this.readString();
                    this.ip++; // skip null terminator
                    const flags = this.readString();
                    this.push(new RegExp(exp, flags));
                    this.log(`OP_REGEX exp=${exp} flags=${flags}`);
                    break;
                }
                // Operation::Undefined => 0x07,
                case 0x07: {
                    this.push(undefined);
                    this.log("OP_UNDEFINED");
                    break;
                }
                // Operation::Return => 0x08,
                case 0x08: {
                    this.callStack.pop();
                    const returnValue = this.pop();
                    if (this.callStack.length === 0) {
                        this.log("OP_RETURN (main)", returnValue);
                        return returnValue;
                    }
                    this.push(returnValue);
                    this.log("OP_RETURN", returnValue);
                    return returnValue;
                }
                // Operation:: StoreVar(_) => 0x09,
                case 0x09: {
                    const varName = this.readString();
                    const value = this.pop();
                    this[varName] = value;
                    this.log(`OP_STORE_VAR ${varName} =`, value);
                    break;
                }
                // Operation::LoadVar(_) => 0x0a,
                case 0x0a: {
                    const varName = this.readString();
                    const value = this[varName];
                    this.push(value);
                    this.log(`OP_LOAD_VAR ${varName} =`, value);
                    break;
                }
                // Operation::UInt8(_) => 0x0b,
                case 0x0b: {
                    const value = this.readUInt8();
                    this.push(value);
                    this.log("OP_LOAD_UINT8", value);
                    break;
                }
                // Operation::UInt16(_) => 0x0c,
                case 0x0c: {
                    const value = this.readUInt16();
                    this.push(value);
                    this.log("OP_LOAD_UINT16", value);
                    break;
                }
                // Operation::UInt32(_) => 0x0d,
                case 0x0d: {
                    const value = this.readUInt32();
                    this.push(value);
                    this.log("OP_LOAD_UINT32", value);
                    break;
                }
                // Operation::UInt64(_) => 0x0e,
                case 0x0e: {
                    // JavaScript cannot represent full UInt64 range accurately
                    const low = this.readUInt32();
                    const high = this.readUInt32();
                    const value = high * 0x100000000 + low;
                    this.push(value);
                    this.log("OP_LOAD_UINT64", value);
                    break;
                }
                // Operation::Call => 0x0f,
                case 0x0f: {
                    const funcIndex = this.readUInt16();
                    const func = this.functions[funcIndex];
                    this.callStack.push(new StackFrame(func));
                    this.bytecode = func.code;
                    this.ip = -1; // will be incremented to 0 at the top of the loop
                    this.log("OP_CALL function index", funcIndex);
                    break;
                }
                // Operation::GetProperty => 0x10,
                case 0x10: {
                    const propName = this.readString();
                    const obj = this.pop();
                    const value = obj[propName];
                    this.push(value);
                    this.log(`OP_GET_PROPERTY ${propName} =`, value);
                    break;
                }
                // Operation::SetProperty => 0x11,
                case 0x11: {
                    const propName = this.readString();
                    const value = this.pop();
                    const obj = this.pop();
                    obj[propName] = value;
                    this.log(`OP_SET_PROPERTY ${propName} =`, value);
                    break;
                }
                // Operation::EqEq => 0x12,
                case 0x12: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a == b;
                    this.push(result);
                    this.log(`OP_EQ_EQ ${a} == ${b} =>`, result);
                    break;
                }
                // Operation::NotEq => 0x13,
                case 0x13: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a != b;
                    this.push(result);
                    this.log(`OP_NOT_EQ ${a} != ${b} =>`, result);
                    break;
                }
                // Operation::EqEqEq => 0x14,
                case 0x14: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a === b;
                    this.push(result);
                    this.log(`OP_EQ_EQ_EQ ${a} === ${b} =>`, result);
                    break;
                }
                // Operation::NotEqEq => 0x15,
                case 0x15: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a !== b;
                    this.push(result);
                    this.log(`OP_NOT_EQ_EQ ${a} !== ${b} =>`, result);
                    break;
                }
                // Operation::Lt => 0x16,
                case 0x16: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a < b;
                    this.push(result);
                    this.log(`OP_LT ${a} < ${b} =>`, result);
                    break;
                }
                // Operation::LtEq => 0x17,
                case 0x17: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a <= b;
                    this.push(result);
                    this.log(`OP_LT_EQ ${a} <= ${b} =>`, result);
                    break;
                }
                // Operation::Gt => 0x18,
                case 0x18: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a > b;
                    this.push(result);
                    this.log(`OP_GT ${a} > ${b} =>`, result);
                    break;
                }
                // Operation::GtEq => 0x19,
                case 0x19: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >= b;
                    this.push(result);
                    this.log(`OP_GT_EQ ${a} >= ${b} =>`, result);
                    break;
                }
                // Operation::LShift => 0x1a,
                case 0x1a: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a << b;
                    this.push(result);
                    this.log(`OP_LSHIFT ${a} << ${b} =>`, result);
                    break;
                }
                // Operation::RShift => 0x1b,
                case 0x1b: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >> b;
                    this.push(result);
                    this.log(`OP_RSHIFT ${a} >> ${b} =>`, result);
                    break;
                }
                // Operation::ZeroFillRShift => 0x1c,
                case 0x1c: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >>> b;
                    this.push(result);
                    this.log(`OP_ZERO_FILL_RSHIFT ${a} >>> ${b} =>`, result);
                    break;
                }
                // Operation::Add => 0x1d,
                case 0x1d: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a + b;
                    this.push(result);
                    this.log(`OP_ADD ${a} + ${b} =>`, result);
                    break;
                }
                // Operation::Sub => 0x1e,
                case 0x1e: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a - b;
                    this.push(result);
                    this.log(`OP_SUB ${a} - ${b} =>`, result);
                    break;
                }
                // Operation::Mul => 0x1f,
                case 0x1f: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a * b;
                    this.push(result);
                    this.log(`OP_MUL ${a} * ${b} =>`, result);
                    break;
                }
                // Operation::Div => 0x20,
                case 0x20: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a / b;
                    this.push(result);
                    this.log(`OP_DIV ${a} / ${b} =>`, result);
                    break;
                }
                // Operation::Mod => 0x21,
                case 0x21: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a % b;
                    this.push(result);
                    this.log(`OP_MOD ${a} % ${b} =>`, result);
                    break;
                }
                // Operation::BitOr => 0x22,
                case 0x22: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a | b;
                    this.push(result);
                    this.log(`OP_BIT_OR ${a} | ${b} =>`, result);
                    break;
                }
                // Operation::BitXor => 0x23,
                case 0x23: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a ^ b;
                    this.push(result);
                    this.log(`OP_BIT_XOR ${a} ^ ${b} =>`, result);
                    break;
                }
                // Operation::BitAnd => 0x24,
                case 0x24: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a & b;
                    this.push(result);
                    this.log(`OP_BIT_AND ${a} & ${b} =>`, result);
                    break;
                }
                // Operation::LogicalOr => 0x25,
                case 0x25: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a || b;
                    this.push(result);
                    this.log(`OP_LOGICAL_OR ${a} || ${b} =>`, result);
                    break;
                }
                // Operation::LogicalAnd => 0x26,
                case 0x26: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a && b;
                    this.push(result);
                    this.log(`OP_LOGICAL_AND ${a} && ${b} =>`, result);
                    break;
                }
                // Operation::In => 0x27,
                case 0x27: {
                    const prop = this.pop();
                    const obj = this.pop();
                    const result = prop in obj;
                    this.push(result);
                    this.log(`OP_IN ${prop} in`, obj, '=>', result);
                    break;
                }
                // Operation::InstanceOf => 0x28,
                case 0x28: {
                    const constructor = this.pop();
                    const obj = this.pop();
                    const result = obj instanceof constructor;
                    this.push(result);
                    this.log(`OP_INSTANCE_OF`, obj, 'instanceof', constructor, '=>', result);
                    break;
                }
                // Operation::Exp => 0x29,
                case 0x29: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = Math.pow(a, b);
                    this.push(result);
                    this.log(`OP_EXP ${a} ** ${b} =>`, result);
                    break;
                }
                // Operation::NullishCoalescing => 0x2a,
                case 0x2a: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = (a !== null && a !== undefined) ? a : b;
                    this.push(result);
                    this.log(`OP_NULLISH_COALESCING ${a} ?? ${b} =>`, result);
                    break;
                }
                default: {
                    this.log("Unknown opcode: " + op);
                    return;
                }
            }
        }
        this.printStack();
    }
}