class VirtualFunction {
    constructor(arity, name, code, env) {
        this.arity = arity;
        this.name = name;
        this.code = new Uint8Array(code);
        this.env = env; // Closure environment (parent StackFrame)
    }
}

class StackFrame {
    constructor(func, stackBase, parent) {
        this.func = func;
        this.stackBase = stackBase || 0;  // Index where this frame's operands start (for restoration)
        this.parent = parent || null;     // Parent frame for upvalues
        this.ip = 0;                      // Instruction pointer for this frame
        this.locals = new Map();          // Locals for this frame (heap allocated/captured)
        this.thisValue = undefined;       // `this` inside the frame
        this.id = Math.random();
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
            const func = new VirtualFunction(0, 'main', funcCode, null);
            this.functions.push(func);
        }

        // init ip and stack 
        this.ip = 0;
        this.stack = [];

        // init call stack with main function
        this.callStack = [new StackFrame(this.functions[0])];
        this.bytecode = this.functions[0].code;

        // The host's real `window` / `globalThis`, exactly as the environment provides them.
        // Never shim or patch these: input scripts rely on them to detect where they run.
        this.hostGlobals = [
            typeof window !== 'undefined' ? window : undefined,
            typeof globalThis !== 'undefined' ? globalThis : undefined,
        ];
        // `this` for plain (non-method) native calls
        this.global = this.hostGlobals[1] || this.hostGlobals[0];
    }

    log(...data) {
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
        this.ip++; // Advance past the opcode or previous null terminator
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
        const entryStackSize = this.callStack.length;

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
                    // this.ip++; // skip null terminator - handled by readString for the next read
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
                    const frame = this.callStack.pop();
                    const returnValue = this.pop();

                    // Clean up stack (remove operands of the returning function)
                    // Locals are in frame.locals, so we don't need to clean them from stack
                    this.stack.length = frame.stackBase;

                    if (this.callStack.length < entryStackSize) {
                        this.log("OP_RETURN (main)", returnValue);
                        return returnValue;
                    }

                    this.push(returnValue);

                    // Restore caller state
                    const callerFrame = this.callStack[this.callStack.length - 1];
                    this.bytecode = callerFrame.func.code;
                    this.ip = callerFrame.ip;

                    this.log("OP_RETURN", returnValue);
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
                    const args = this.popArgs();
                    const func = this.pop();
                    // plain natives keep being called with the host global as `this`
                    this.callFunction(func, undefined, this.global, args);
                    break;
                }
                // Operation::GetProperty => 0x10,
                case 0x10: {
                    const propName = this.pop();
                    const obj = this.pop();
                    const value = obj[propName];
                    this.push(value);
                    this.log(`OP_GET_PROPERTY ${propName} =`, value);
                    break;
                }
                // Operation::SetProperty => 0x11,
                case 0x11: {
                    let value = this.pop();
                    const propName = this.pop();
                    const obj = this.pop();

                    if (value instanceof VirtualFunction) {
                        value = this.createHostWrapper(value);
                    }

                    obj[propName] = value;
                    this.push(value); // SetProperty is an expression that returns the value
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
                // Operation::In => 0x27,
                case 0x27: {
                    const obj = this.pop();
                    const prop = this.pop();
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
                // Operation::GetLocal(_) => 0x2b,
                case 0x2b: {
                    const localIndex = this.readUInt8();
                    const frame = this.callStack[this.callStack.length - 1];
                    const value = frame.locals.get(localIndex);
                    this.push(value);
                    this.log(`OP_GET_LOCAL index=${localIndex} value=`, value);
                    break;
                }
                // Operation::SetLocal(_) => 0x2c,
                case 0x2c: {
                    const localIndex = this.readUInt8();
                    const value = this.peek();  // Don't pop, assignment is an expression
                    const frame = this.callStack[this.callStack.length - 1];
                    frame.locals.set(localIndex, value);
                    this.log(`OP_SET_LOCAL index=${localIndex} value=`, value);
                    break;
                }
                // Operation::UnaryMinus => 0x2d,
                case 0x2d: {
                    const a = this.pop();
                    const result = -a;
                    this.push(result);
                    this.log(`OP_UNARY_MINUS -${a} =>`, result);
                    break;
                }
                // Operation::UnaryPlus => 0x2e,
                case 0x2e: {
                    const a = this.pop();
                    const result = +a;
                    this.push(result);
                    this.log(`OP_UNARY_PLUS +${a} =>`, result);
                    break;
                }
                // Operation::LogicalNot => 0x2f,
                case 0x2f: {
                    const a = this.pop();
                    const result = !a;
                    this.push(result);
                    this.log(`OP_LOGICAL_NOT !${a} =>`, result);
                    break;
                }
                // Operation::BitwiseNot => 0x30,
                case 0x30: {
                    const a = this.pop();
                    const result = ~a;
                    this.push(result);
                    this.log(`OP_BITWISE_NOT ~${a} =>`, result);
                    break;
                }
                // Operation::TypeOf => 0x31,
                case 0x31: {
                    const a = this.pop();
                    const result = typeof a;
                    this.push(result);
                    this.log(`OP_TYPEOF typeof ${a} =>`, result);
                    break;
                }
                // Operation::Void => 0x32,
                case 0x32: {
                    this.pop();
                    this.push(undefined);
                    this.log(`OP_VOID => undefined`);
                    break;
                }
                // Operation::Delete => 0x33,
                case 0x33: {
                    const propName = this.pop();
                    const obj = this.pop();
                    const result = delete obj[propName];
                    this.push(result);
                    this.log(`OP_DELETE delete ${propName} from`, obj, '=>', result);
                    break;
                }
                // Operation::Array(_) => 0x34,
                case 0x34: {
                    const count = this.readUInt32();
                    const arr = [];
                    for (let i = 0; i < count; i++) {
                        arr.push(this.pop());
                    }
                    arr.reverse(); // Stack is LIFO, so reverse to get correct order
                    this.push(arr);
                    this.log(`OP_ARRAY count=${count}`, arr);
                    break;
                }
                // Operation::Object(_) => 0x35,
                case 0x35: {
                    const count = this.readUInt32();
                    const obj = {};
                    for (let i = 0; i < count; i++) {
                        const value = this.pop();
                        const key = this.pop();
                        obj[key] = value;
                    }
                    this.push(obj);
                    this.log(`OP_OBJECT count=${count}`, obj);
                    break;
                }
                // Operation::Dup => 0x36,
                case 0x36: {
                    const value = this.peek();
                    this.push(value);
                    this.log(`OP_DUP`, value);
                    break;
                }
                // Operation::Function => 0x37,
                case 0x37: {
                    const arity = this.readUInt8();
                    const name = this.readString();
                    const codeLen = this.readUInt32();
                    const code = [];
                    for (let i = 0; i < codeLen; i++) {
                        code.push(this.readUInt8());
                    }

                    // Capture current environment
                    const currentFrame = this.callStack[this.callStack.length - 1];
                    const func = new VirtualFunction(arity, name, code, currentFrame);
                    this.push(func);
                    this.log(`OP_FUNCTION name=${name} arity=${arity}`);
                    break;
                }
                // Operation::GetUpvalue => 0x38,
                case 0x38: {
                    const depth = this.readUInt8();
                    const index = this.readUInt8();

                    let frame = this.callStack[this.callStack.length - 1];
                    for (let i = 0; i < depth; i++) {
                        frame = frame.parent;
                        if (!frame) {
                            throw new Error(`Invalid upvalue depth: ${depth}`);
                        }
                    }

                    const value = frame.locals.get(index);
                    this.push(value);
                    this.log(`OP_GET_UPVALUE depth=${depth} index=${index} value=`, value);
                    break;
                }
                // Operation::SetUpvalue => 0x39,
                case 0x39: {
                    const depth = this.readUInt8();
                    const index = this.readUInt8();
                    const value = this.peek(); // Don't pop, assignment is an expression

                    let frame = this.callStack[this.callStack.length - 1];
                    for (let i = 0; i < depth; i++) {
                        frame = frame.parent;
                        if (!frame) {
                            throw new Error(`Invalid upvalue depth: ${depth}`);
                        }
                    }

                    frame.locals.set(index, value);
                    this.log(`OP_SET_UPVALUE depth=${depth} index=${index} value=`, value);
                    break;
                }
                // Operation::Jump(_) => 0x3a,
                case 0x3a: {
                    const offset = this.readUInt32();
                    this.ip += offset;
                    this.log(`OP_JUMP +${offset}`);
                    break;
                }
                // Operation::JumpIfFalse(_) => 0x3b,
                case 0x3b: {
                    const offset = this.readUInt32();
                    if (!this.peek()) this.ip += offset;
                    this.log(`OP_JUMP_IF_FALSE +${offset}`);
                    break;
                }
                // Operation::JumpIfTrue(_) => 0x3c,
                case 0x3c: {
                    const offset = this.readUInt32();
                    if (this.peek()) this.ip += offset;
                    this.log(`OP_JUMP_IF_TRUE +${offset}`);
                    break;
                }
                // Operation::JumpIfNotNullish(_) => 0x3d,
                case 0x3d: {
                    const offset = this.readUInt32();
                    const value = this.peek();
                    if (value !== null && value !== undefined) this.ip += offset;
                    this.log(`OP_JUMP_IF_NOT_NULLISH +${offset}`);
                    break;
                }
                // Operation::CallMethod => 0x3e,
                case 0x3e: {
                    const args = this.popArgs();
                    const func = this.pop();
                    const receiver = this.pop();
                    this.callFunction(func, receiver, receiver, args);
                    break;
                }
                // Operation::This => 0x3f,
                case 0x3f: {
                    const frame = this.callStack[this.callStack.length - 1];
                    this.push(frame.thisValue);
                    this.log("OP_THIS", frame.thisValue);
                    break;
                }
                // Operation::HostGlobal(_) => 0x40,
                case 0x40: {
                    const flags = this.readUInt8();
                    const which = flags & 0b01; // 0 = window, 1 = globalThis
                    const value = this.hostGlobals[which];
                    // HOST_GLOBAL_TYPEOF: `typeof window` gets undefined instead of throwing
                    if (value === undefined && !(flags & 0b10)) {
                        throw new ReferenceError(`${which ? 'globalThis' : 'window'} is not defined`);
                    }
                    this.push(value);
                    this.log("OP_HOST_GLOBAL", which);
                    break;
                }
                default: {
                    throw new Error(`Unknown opcode 0x${op.toString(16)} at ${this.ip}`);
                }
            }
        }
        this.printStack();
    }

    // Pop the arg count and then that many args, returned in call order.
    popArgs() {
        const argCount = this.pop();
        const args = new Array(argCount);
        for (let i = argCount - 1; i >= 0; i--) {
            args[i] = this.pop();
        }
        return args;
    }

    // `vmThis` is `this` inside a VM function; `nativeThis` is `this` for a host function.
    callFunction(func, vmThis, nativeThis, args) {
        let targetFunc = func;
        if (func && func.virtualFunction instanceof VirtualFunction) {
            targetFunc = func.virtualFunction;
        }

        if (targetFunc instanceof VirtualFunction) {
            const currentFrame = this.callStack[this.callStack.length - 1];
            currentFrame.ip = this.ip; // save current IP

            // parent is the function's definition environment (closure)
            const newFrame = new StackFrame(targetFunc, this.stack.length, targetFunc.env);
            newFrame.thisValue = vmThis;
            for (let i = 0; i < args.length; i++) {
                newFrame.locals.set(i, args[i]);
            }
            this.callStack.push(newFrame);

            this.bytecode = targetFunc.code;
            this.ip = -1;
            this.log(`OP_CALL virtual function ${targetFunc.name} arity=${targetFunc.arity} args=${args.length}`);
        } else if (typeof func === 'function') {
            const result = func.apply(nativeThis, args);
            this.push(result);
            this.log("OP_CALL native function", result);
        } else {
            throw new TypeError(`${func} is not a function`);
        }
    }

    createHostWrapper(func) {
        const vm = this;
        const wrapper = function (...args) {
            // Save current state
            const savedIp = vm.ip;
            const savedBytecode = vm.bytecode;

            // Create new stack frame
            const newStackBase = vm.stack.length;
            const newFrame = new StackFrame(func, newStackBase, func.env);
            newFrame.thisValue = this;
            for (let i = 0; i < args.length; i++) {
                newFrame.locals.set(i, args[i]);
            }
            vm.callStack.push(newFrame);

            // Setup VM state
            vm.bytecode = func.code;
            vm.ip = 0;

            // Run VM
            const result = vm.run();

            // Restore state
            vm.ip = savedIp;
            vm.bytecode = savedBytecode;

            return result;
        };

        wrapper.virtualFunction = func;
        return wrapper;
    }
}

if (typeof module !== 'undefined' && module.exports) {
    module.exports = { VM };
}