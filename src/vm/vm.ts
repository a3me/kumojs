'use strict';

// The kumo VM: a stack machine that runs the compiler's bytecode.
//
// - Each function's code is a `Uint8Array`. `ip` points at the last byte consumed, and the
//   run loop's `ip++` steps to the next opcode, so reading an operand is `bytecode[++ip]`,
//   a frame that is about to start has `ip = -1`, and a jump lands one byte short of its
//   target.
// - VM code calling VM code runs in the same `execute` loop (a frame is pushed). Only host
//   code calling a VM function (through its host wrapper) starts a nested `run()`.
// - `try` blocks push entries on `handlers`; a throw unwinds to the innermost one.
// - Async functions are `Coroutine`s: a suspended one has left the call stack, taking its
//   operands and handlers with it.
//
// The file is laid out as: constants and markers, host intrinsics, small helpers, hidden
// per-object slots, iteration, coroutines, host wrappers, function/frame data, and the VM.

// ---- values shared with the compiler ----

// function kinds (`FnKind` in the compiler) in the low bits of a function's flags, plus
// the async flag bit
const FN_NORMAL = 0;
const FN_ARROW = 1;
const FN_METHOD = 2;
const FN_CLASS = 3;
const FN_DERIVED_CLASS = 4;
const FN_KIND_MASK = 7;
const FN_FLAG_ASYNC = 8;

// private name kinds (`PrivateKind` in the compiler)
const PRIVATE_FIELD = 0;
const PRIVATE_METHOD = 1;
const PRIVATE_ACCESSOR = 2;

// what a class or object member defines (`DefineMethod`, `DefinePrivateMethod`), also the
// prefix `SetFunctionName` puts on a name: none / `get ` / `set `
const MEMBER_METHOD = 0;
const MEMBER_GETTER = 1;
const MEMBER_SETTER = 2;
// `DefinePrivateMethod` flags: the member kind in the low bits, then "static"
const PRIVATE_DEFINE_KIND_MASK = 3;
const PRIVATE_DEFINE_STATIC = 4;

// `HostGlobal` flags: which global, and `typeof window` (gets undefined, doesn't throw)
const HOST_GLOBAL_IS_GLOBAL_THIS = 0b01; // clear = window
const HOST_GLOBAL_TYPEOF = 0b10;

// ---- markers: unique values that can't be mistaken for a script's own values ----

// a `let` / `const` / `class` binding that a closure may read before it's initialised
const TDZ = {};
// `this` in a derived class constructor before `super(...)` has run
const UNINITIALIZED = {};
// an array literal hole (`[1, , 3]`) on the stack
const HOLE = {};
// returned by `unwindCoroutines` when the throw goes on
const NOTHING = {};
const EMPTY: any[] = [];

// `iterTake` count meaning "all of them" (a rest element)
const ITER_TAKE_ALL = 0xffffffff;

// ---- errors ----

function tdzError() {
    return new ReferenceError('Cannot access a variable before initialization');
}

function thisError() {
    return new ReferenceError(
        "Must call super constructor in derived class before accessing 'this' or returning from derived constructor");
}

function privateError() {
    return new TypeError('Cannot access private member from an object whose class did not declare it');
}

function notIterableError(value: any) {
    return new TypeError(`${typeof value === 'string' ? value : typeof value} is not iterable`);
}

function notObjectError(result: any) {
    return new TypeError(`Iterator result ${String(result)} is not an object`);
}

// ---- host intrinsics, captured once so input scripts can't change how the VM behaves ----

const arraySlice = Array.prototype.slice;
const arrayValues = Array.prototype.values;
const propertyIsEnumerable = Object.prototype.propertyIsEnumerable;
const defineProperty = Object.defineProperty;
const HostBigInt = typeof BigInt === 'function' ? BigInt : undefined; // only for `1n` literals
const HostPromise = Promise;
const promiseResolve = Promise.resolve;
const promiseThen = Promise.prototype.then;
const AsyncFunctionPrototype = Object.getPrototypeOf(async function () {});

// scratch space for decoding f64 literals
const F64 = new Float64Array(1);
const F64_BYTES = new Uint8Array(F64.buffer);
const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

// ---- small helpers ----

function resolved(value: any) {
    return promiseResolve.call(HostPromise, value);
}

function isObject(value: any) {
    return value !== null && (typeof value === 'object' || typeof value === 'function');
}

function toPropertyKey(key: any) {
    return typeof key === 'symbol' ? key : String(key);
}

function dataProperty(value: any) {
    return { value, writable: true, enumerable: true, configurable: true };
}

function methodProperty(value: any) {
    return { value, writable: true, enumerable: false, configurable: true };
}

// whether `new f()` is allowed (the standard check via Reflect.construct's newTarget)
function isConstructor(f: any) {
    if (typeof f !== 'function') return false;
    try {
        Reflect.construct(String, [], f);
        return true;
    } catch (e) {
        return false;
    }
}

// a new object for `new fn()`: it inherits from `fn.prototype` when that is an object
function newInstance(fn: any) {
    const proto = fn.prototype;
    return Object.create(isObject(proto) ? proto : Object.prototype);
}

// `arguments` objects are made by a real function, so they look and behave like the host's
function makeArguments(..._args: any[]) {
    return arguments;
}

// An empty array that already holds any kind of value. V8 tracks what an array holds
// (small ints, doubles, anything) and code optimised for "anything" deoptimises on a fresh
// `[]` (small ints) the first time it changes: for the operand stack, once per new VM.
function genericArray() {
    const array = [undefined];
    array.length = 0;
    return array;
}

// ---- hidden per-object slots ----

// A base constructor that returns another object lets a subclass add its private fields to
// that object. That stamps a hidden slot onto any object, with no property or symbol for
// scripts to see or reach (unlike a normal property), and it's faster than a WeakMap entry.
class ReturnTarget {
    constructor(target: any) {
        return target;
    }
}

function hiddenSlot() {
    return class extends ReturnTarget {
        #value;
        constructor(target: any, value: any) {
            super(target);
            this.#value = value;
        }
        static set(target: any, value: any) {
            new this(target, value);
        }
        // the stored value, or undefined for any object without the slot
        static get(target: any) {
            return isObject(target) && #value in target ? target.#value : undefined;
        }
    };
}

// host wrapper function -> its VirtualFunction
const wrappedFunctions = hiddenSlot();

// a VM method (getter, setter, class member) remembers the object it was defined on, for `super`
function setHomeObject(fn: any, home: any) {
    if (typeof fn === 'function') {
        const vf = wrappedFunctions.get(fn);
        if (vf !== undefined && vf.method) vf.homeObject = home;
    }
}

// Name a VM function after a property key (computed keys), with a `MEMBER_*` prefix.
function nameFunction(fn: any, key: any, prefix: number) {
    const vf = wrappedFunctions.get(fn);
    if (vf === undefined) return;
    let name = typeof key === 'symbol'
        ? (key.description === undefined ? '' : `[${key.description}]`)
        : String(key);
    if (prefix === MEMBER_GETTER) name = 'get ' + name;
    else if (prefix === MEMBER_SETTER) name = 'set ' + name;
    vf.name = name;
    defineProperty(fn, 'name', { value: name, configurable: true });
}

// an exception handler (`try`): where to resume if something throws
interface Handler {
    callDepth: number; // call stack length when the `try` started
    stackLen: number;  // operand stack length to cut back to
    catchIp: number;
}

// a suspended coroutine's handler; its stack position is relative to the frame's base
interface SavedHandler {
    rel: number;
    catchIp: number;
}

// coroutine states
const CO_RUNNING = 0;
const CO_DONE = 1;
const CO_AWAITING = 2; // stopped at an `await`

// how a suspended coroutine is resumed: the awaited value, or the rejection
const RESUME_NEXT = 0;
const RESUME_THROW = 1;

// An async function's suspendable state. While it's suspended at an `await` its frame is
// off the call stack, with its operands and exception handlers saved here.
class Coroutine {
    declare vm: VM;
    declare frame: StackFrame;
    declare state: number;
    // saved operands
    declare stack: any[] | null;
    declare handlers: SavedHandler[] | null;
    // where to continue (next instruction) for each way of resuming
    declare normalIp: number;
    declare throwIp: number;
    // the promise the call returned
    declare promise: any;
    declare resolve: any;
    declare reject: any;

    constructor(vm: VM, frame: StackFrame) {
        this.vm = vm;
        this.frame = frame;
        this.state = CO_RUNNING;
        this.stack = null;
        this.handlers = null;
        this.normalIp = 0;
        this.throwIp = 0;
        this.promise = null;
        this.resolve = null;
        this.reject = null;
    }

    // where to continue when resumed with `mode`
    resumeIp(mode: number) {
        return mode === RESUME_NEXT ? this.normalIp : this.throwIp;
    }
}

// ---- iteration ----

// The first `count` values of an iterable (array patterns), closing the iterator early
// like JS does. Arrays with the normal iterator are read directly.
function iterTake(iterable: any, count: number) {
    if (Array.isArray(iterable) && iterable[Symbol.iterator] === arrayValues) {
        return iterable;
    }
    if (iterable === null || iterable === undefined ||
        typeof iterable[Symbol.iterator] !== 'function') {
        throw notIterableError(iterable);
    }
    const iterator = iterable[Symbol.iterator]();
    const values = [];
    while (count === ITER_TAKE_ALL || values.length < count) {
        const step = iterator.next();
        if (step.done) return values;
        values.push(step.value);
    }
    if (typeof iterator.return === 'function') iterator.return();
    return values;
}

// `for-of` iteration state. Arrays using the built-in iterator are walked by index (the
// same values and order the iterator would give, without creating it).
const ArrayIteratorPrototype = Object.getPrototypeOf([][Symbol.iterator]());
const arrayIteratorNext = ArrayIteratorPrototype.next;
class IterRecord {
    declare iterator: any;
    declare next: any;
    declare array: any;
    declare index: number;
    declare done: boolean;

    constructor(iterator: any, next: any, array: any) {
        this.iterator = iterator;
        this.next = next;
        this.array = array;
        this.index = 0;
        this.done = false;
    }
}
// returned by `iterStep` when the iterator is done
const ITER_DONE = {};

function getIterator(iterable: any) {
    if (Array.isArray(iterable) && iterable[Symbol.iterator] === arrayValues &&
        ArrayIteratorPrototype.next === arrayIteratorNext) {
        return new IterRecord(null, null, iterable);
    }
    const method = iterable === null || iterable === undefined ? undefined : iterable[Symbol.iterator];
    if (typeof method !== 'function') throw notIterableError(iterable);
    const iterator = method.call(iterable);
    if (!isObject(iterator)) throw new TypeError('Result of the Symbol.iterator method is not an object');
    return new IterRecord(iterator, iterator.next, null);
}

// The next value, or ITER_DONE. An iterator that throws or finishes is done (not closed).
function iterStep(record: IterRecord) {
    if (record.array !== null) {
        if (record.index < record.array.length) return record.array[record.index++];
        record.done = true;
        return ITER_DONE;
    }
    try {
        const result = record.next.call(record.iterator);
        if (!isObject(result)) throw notObjectError(result);
        if (result.done) {
            record.done = true;
            return ITER_DONE;
        }
        return result.value;
    } catch (e) {
        record.done = true;
        throw e;
    }
}

// Close an iterator left early: call its `return()`. Leaving by a throw (`quiet`) ignores
// errors from it, so the original error wins.
function iterClose(record: IterRecord, quiet: boolean) {
    if (record.done || record.array !== null) return;
    record.done = true;
    const iterator = record.iterator;
    if (quiet) {
        try {
            const ret = iterator.return;
            if (ret !== undefined && ret !== null) ret.call(iterator);
        } catch (e) {
            // the throw that's leaving the loop wins
        }
        return;
    }
    const ret = iterator.return;
    if (ret === undefined || ret === null) return;
    if (typeof ret !== 'function') throw new TypeError('iterator.return is not a function');
    if (!isObject(ret.call(iterator))) throw notObjectError(undefined);
}

// ---- host wrappers ----

type WrapperMaker = (vm: VM, vf: VirtualFunction, name: string) => Function;

// Host wrappers by kind and parameter count, so `length` is right without being set
// afterwards (which is slow). Normal functions and methods pass their `arguments`;
// arrows have none, but padding missing arguments with `undefined` can't be observed.
// Each is made under a computed key `n` so it gets the function's name from JS itself
// (setting `name` afterwards with defineProperty is slow).
const WRAPPERS: WrapperMaker[][] = [
    // FN_NORMAL
    [
        (vm, vf, n) => ({ [n]: function () { return vm.runFromHost(vf, this, arguments, new.target); } })[n],
        (vm, vf, n) => ({ [n]: function (a: any) { return vm.runFromHost(vf, this, arguments, new.target); } })[n],
        (vm, vf, n) => ({ [n]: function (a: any, b: any) { return vm.runFromHost(vf, this, arguments, new.target); } })[n],
        (vm, vf, n) => ({ [n]: function (a: any, b: any, c: any) { return vm.runFromHost(vf, this, arguments, new.target); } })[n],
    ],
    // FN_ARROW
    [
        (vm, vf, n) => ({ [n]: (...r: any[]) => vm.runFromHost(vf, undefined, r) })[n],
        (vm, vf, n) => ({ [n]: (a: any, ...r: any[]) => vm.runFromHost(vf, undefined, [a, ...r]) })[n],
        (vm, vf, n) => ({ [n]: (a: any, b: any, ...r: any[]) => vm.runFromHost(vf, undefined, [a, b, ...r]) })[n],
        (vm, vf, n) => ({ [n]: (a: any, b: any, c: any, ...r: any[]) => vm.runFromHost(vf, undefined, [a, b, c, ...r]) })[n],
    ],
    // FN_METHOD
    [
        (vm, vf, n) => ({ [n]() { return vm.runFromHost(vf, this, arguments); } })[n],
        (vm, vf, n) => ({ [n](a: any) { return vm.runFromHost(vf, this, arguments); } })[n],
        (vm, vf, n) => ({ [n](a: any, b: any) { return vm.runFromHost(vf, this, arguments); } })[n],
        (vm, vf, n) => ({ [n](a: any, b: any, c: any) { return vm.runFromHost(vf, this, arguments); } })[n],
    ],
];

// Class constructors need `new`; a base class uses the `this` the host's `new` made,
// a derived class gets its `this` from `super(...)`.
function makeClassWrapper(vm: VM, vf: VirtualFunction, name: string) {
    return {
        [name]: function () {
            if (new.target === undefined) {
                throw new TypeError(`Class constructor ${vf.name} cannot be invoked without 'new'`);
            }
            return vm.runFromHost(vf, vf.derived ? UNINITIALIZED : this, arguments, new.target, true);
        },
    }[name];
}

// ---- functions and frames ----

// A class's private name (`#x`), made fresh each time the class body is evaluated. Field
// values live in a WeakMap, so they never appear on the object (no reflection, works on
// frozen objects and doesn't touch proxies). Methods and accessors are shared: an object
// may use them once it's in the brand (the class's WeakSet).
class PrivateName {
    declare kind: number;
    // field values by object (fields only)
    declare values: WeakMap<object, any> | null;
    declare brand: any;
    // a method, or an accessor's getter
    declare method: any;
    declare setter: any;

    constructor(kind: number) {
        this.kind = kind;
        this.values = kind === PRIVATE_FIELD ? new WeakMap() : null;
        this.brand = null;
        this.method = undefined;
        this.setter = undefined;
    }
}

class VirtualFunction {
    // `async`: calling it returns a promise
    declare async: boolean;
    // the function's `length`
    declare arity: number;
    // parameter slots filled from the call's arguments
    declare params: number;
    // arrows use `this` and `arguments` from where they were made
    declare arrow: boolean;
    // methods (and arrows) can't be used with `new`
    declare method: boolean;
    // a class constructor: only callable with `new`
    declare isClass: boolean;
    // a derived class constructor: `this` comes from `super(...)`
    declare derived: boolean;
    // a class's instance field keys (property keys / private names)
    declare fieldKeys: any;
    // a class's method setting up instance field values
    declare initFn: VirtualFunction | null;
    // a class's WeakSet of instances, if it has private methods
    declare brand: any;
    // where `super` lookups start (methods, class members)
    declare homeObject: any;
    // arrows: the enclosing function's frame
    declare argsFrame: StackFrame | null;
    declare name: string;
    // may be a view into the parent's bytecode
    declare code: Uint8Array;
    // boxes (`{ v }`) of the variables this closure uses
    declare captures: any[];
    // host-callable function for this VM function
    declare wrapper: any;

    constructor(arity: number, params: number, flags: number, name: string, code: Uint8Array, captures: any[]) {
        const kind = flags & FN_KIND_MASK;
        this.async = (flags & FN_FLAG_ASYNC) !== 0;
        this.arity = arity;
        this.params = params;
        this.arrow = kind === FN_ARROW;
        this.method = kind === FN_METHOD;
        this.isClass = kind === FN_CLASS || kind === FN_DERIVED_CLASS;
        this.derived = kind === FN_DERIVED_CLASS;
        this.fieldKeys = null;
        this.initFn = null;
        this.brand = null;
        this.homeObject = null;
        this.argsFrame = null;
        this.name = name;
        this.code = code;
        this.captures = captures;
        this.wrapper = null;
    }
}

class StackFrame {
    declare vf: VirtualFunction;
    // index where this frame's operands start on the operand stack
    declare stackBase: number;
    // where to continue when this frame is current again
    declare ip: number;
    // local slots; a captured variable's slot holds a box
    declare locals: any[];
    declare thisValue: any;
    // all call arguments, for `arguments`
    declare args: any[];
    declare argumentsObject: any;
    // the enclosing non-arrow frame: where `this`, `arguments`, `super` and `new.target`
    // come from (itself, except for arrows)
    declare argsFrame: StackFrame;
    declare newTarget: any;
    // running as `new f()`: return `this` unless an object is returned
    declare isConstruct: boolean;
    // an async function's state
    declare coroutine: Coroutine | null;

    constructor(vf: VirtualFunction, stackBase?: number) {
        this.vf = vf;
        this.stackBase = stackBase || 0;
        this.ip = 0;
        this.locals = [];
        this.thisValue = undefined;
        this.args = [];
        this.argumentsObject = null;
        this.argsFrame = this;
        this.newTarget = undefined;
        this.isConstruct = false;
        this.coroutine = null;
    }
}

// ---- the VM ----

class VM {
    declare options: { debug: boolean; debugLog: any };
    declare debug: boolean;
    // the functions of the program; the first is the main one
    declare functions: VirtualFunction[];
    declare ip: number;
    // the operand stack, shared by all frames
    declare stack: any[];
    declare callStack: StackFrame[];
    // exception handlers, innermost last
    declare handlers: Handler[];
    // tagged template strings arrays, by template site
    declare templates: Map<number, string[]>;
    // the code of the running function
    declare bytecode: Uint8Array;
    // decoded string constants, per bytecode buffer (see `readString`), and the buffer
    // and cache currently in use
    declare stringCaches: Map<ArrayBufferLike, string[]>;
    declare stringBuffer: ArrayBufferLike | null;
    declare stringCache: string[] | null;
    // The host's real `window` / `globalThis`, exactly as the environment provides them.
    // Never shim or patch these: input scripts rely on them to detect where they run.
    declare hostGlobals: any[];
    // `this` for plain (non-method) native calls
    declare global: any;

    constructor(bytecode: any[], options?: any) {
        this.options = options || {
            debug: false,
            debugLog: undefined
        };
        this.debug = !!(this.options.debug && this.options.debugLog);

        this.functions = [];
        for (const functionCode of bytecode) {
            const code = functionCode instanceof Uint8Array ? functionCode : new Uint8Array(functionCode);
            this.functions.push(new VirtualFunction(0, 0, FN_NORMAL, 'main', code, []));
        }

        this.ip = 0;
        this.stack = genericArray();
        this.callStack = [new StackFrame(this.functions[0])];
        this.handlers = [];
        this.templates = new Map();
        this.bytecode = this.functions[0].code;
        this.stringCaches = new Map();
        this.stringBuffer = null;
        this.stringCache = null;
        this.hostGlobals = [
            typeof window !== 'undefined' ? window : undefined,
            typeof globalThis !== 'undefined' ? globalThis : undefined,
        ];
        this.global = this.hostGlobals[1] || this.hostGlobals[0];
    }

    // ---- debug output

    log(...data: any[]) {
        if (this.options.debug && this.options.debugLog) {
            this.options.debugLog(...data);
        }
    }

    printStack() {
        this.log(`STACK:${this.stack.slice().join(', ')}`);
    }

    // ---- frames and operands

    // the running function's frame
    get frame() {
        return this.callStack[this.callStack.length - 1];
    }

    push(value: any) {
        this.stack.push(value);
    }

    pop() {
        return this.stack.pop();
    }

    peek() {
        return this.stack[this.stack.length - 1];
    }

    // make the top frame's code current, continuing at `ip`
    continueAt(ip: number) {
        this.bytecode = this.frame.vf.code;
        this.ip = ip;
    }

    // go back to the top frame after a call returned or a coroutine suspended
    resumeCaller() {
        this.continueAt(this.frame.ip);
    }

    // call into `frame`: the current frame remembers where to come back to
    enterFrame(frame: StackFrame) {
        this.frame.ip = this.ip;
        this.callStack.push(frame);
        this.bytecode = frame.vf.code;
        this.ip = -1;
    }

    // drop the handlers of frames deeper than `depth`
    dropHandlersAbove(depth: number) {
        while (this.handlers.length && this.handlers[this.handlers.length - 1].callDepth > depth) {
            this.handlers.pop();
        }
    }

    // ---- reading operands (`ip` ends on the last byte read)

    readBool() {
        return this.bytecode[++this.ip] === 1;
    }

    readUInt8() {
        return this.bytecode[++this.ip];
    }

    readUInt16() {
        const bytes = this.bytecode;
        const ip = this.ip;
        this.ip = ip + 2;
        return bytes[ip + 1] | (bytes[ip + 2] << 8);
    }

    readUInt32() {
        const bytes = this.bytecode;
        const ip = this.ip;
        this.ip = ip + 4;
        return (bytes[ip + 1] | (bytes[ip + 2] << 8) | (bytes[ip + 3] << 16) | (bytes[ip + 4] << 24)) >>> 0;
    }

    // little-endian f64, decoded through a shared scratch buffer (no allocation per read)
    readFloat64() {
        const bytes = this.bytecode;
        const ip = this.ip;
        for (let i = 0; i < 8; i++) {
            F64_BYTES[LITTLE_ENDIAN ? i : 7 - i] = bytes[ip + 1 + i];
        }
        this.ip = ip + 8;
        return F64[0];
    }

    // u32 byte length, then WTF-8 (UTF-8 that may also encode lone surrogates). Each
    // string is decoded once and cached by where it sits in its bytecode buffer.
    readString() {
        const bytes = this.bytecode;
        if (bytes.buffer !== this.stringBuffer) {
            this.stringBuffer = bytes.buffer;
            let cache = this.stringCaches.get(bytes.buffer);
            if (cache === undefined) {
                cache = [];
                this.stringCaches.set(bytes.buffer, cache);
            }
            this.stringCache = cache;
        }
        const at = bytes.byteOffset + this.ip;
        const cached = this.stringCache![at];
        const length = this.readUInt32();
        if (cached !== undefined) {
            this.ip += length;
            return cached;
        }
        const string = this.decodeString(length);
        this.stringCache![at] = string;
        return string;
    }

    decodeString(length: number) {
        const bytes = this.bytecode;
        let i = this.ip + 1;
        const end = i + length;
        let result = '';
        const units = [];
        while (i < end) {
            const b = bytes[i++];
            if (b < 0x80) {
                units.push(b);
            } else if (b < 0xe0) {
                units.push(((b & 0x1f) << 6) | (bytes[i++] & 0x3f));
            } else if (b < 0xf0) {
                units.push(((b & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f));
            } else {
                const cp = (((b & 0x07) << 18) | ((bytes[i++] & 0x3f) << 12) |
                    ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f)) - 0x10000;
                units.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
            }
            if (units.length >= 4096) {
                result += String.fromCharCode.apply(null, units);
                units.length = 0;
            }
        }
        this.ip = end - 1;
        return result + String.fromCharCode.apply(null, units);
    }

    // ---- the run loop

    // Run until the frame on top of the call stack returns. Exceptions are caught by the
    // innermost handler that belongs to this run; others propagate to the caller.
    run() {
        const entryDepth = this.callStack.length;
        for (;;) {
            try {
                return this.execute(entryDepth);
            } catch (e) {
                const handler = this.handlers[this.handlers.length - 1];
                const ownHandler = handler !== undefined && handler.callDepth >= entryDepth;
                // async functions among the frames being unwound
                const promise = this.unwindCoroutines(e, ownHandler ? handler.callDepth : entryDepth - 1, entryDepth);
                if (promise !== NOTHING) {
                    if (promise !== undefined) return promise;
                    continue;
                }
                if (!ownHandler) {
                    throw e;
                }
                this.handlers.pop();
                this.callStack.length = handler.callDepth;
                this.stack.length = handler.stackLen;
                this.continueAt(handler.catchIp);
                this.push(e);
                this.debug && this.log("caught", e);
            }
        }
    }

    // A throw is unwinding the frames from index `keep` up: an async function stops it,
    // rejecting its promise instead and returning that to its caller. Returns NOTHING if
    // the throw goes on; otherwise the promise when it's for `run()` to return, or
    // undefined to keep running.
    unwindCoroutines(e: any, keep: number, entryDepth: number) {
        for (let i = this.callStack.length - 1; i >= keep; i--) {
            const frame = this.callStack[i];
            const co = frame.coroutine;
            if (co === null) continue;
            co.state = CO_DONE;
            co.reject(e);
            this.callStack.length = i;
            this.dropHandlersAbove(i);
            this.stack.length = frame.stackBase;
            if (i < entryDepth) return co.promise;
            this.push(co.promise);
            this.continueAt(this.frame.ip + 1);
            return undefined;
        }
        return NOTHING;
    }

    execute(entryDepth: number) {
        // read once: log arguments (often template strings) are only built when debugging
        const debug = this.debug;
        for (; this.ip < this.bytecode.length; this.ip++) {
            // get current opcode
            const op = this.bytecode[this.ip];

            // switch on opcode and execute operation
            switch (op) {
                // ---- constants and stack
                // Operation::LoadString(_) => 0x01,
                case 0x01: {
                    this.push(this.readString());
                    debug && this.log("OP_LOAD_STRING", this.peek());
                    break;
                }
                // Operation::LoadFloat64(_) => 0x02,
                case 0x02: {
                    this.push(this.readFloat64());
                    debug && this.log("OP_LOAD_FLOAT64", this.peek());
                    break;
                }
                // Operation::Bool(_) => 0x03,
                case 0x03: {
                    this.push(this.readBool());
                    debug && this.log("OP_LOAD_BOOL", this.peek());
                    break;
                }
                // Operation::Null => 0x05,
                case 0x05: {
                    this.push(null);
                    debug && this.log("OP_NULL");
                    break;
                }
                // Operation::Undefined => 0x07,
                case 0x07: {
                    this.push(undefined);
                    debug && this.log("OP_UNDEFINED");
                    break;
                }
                // Operation::UInt8(_) => 0x0b,
                case 0x0b: {
                    const value = this.readUInt8();
                    this.push(value);
                    debug && this.log("OP_LOAD_UINT8", value);
                    break;
                }
                // Operation::UInt16(_) => 0x0c,
                case 0x0c: {
                    const value = this.readUInt16();
                    this.push(value);
                    debug && this.log("OP_LOAD_UINT16", value);
                    break;
                }
                // Operation::UInt32(_) => 0x0d,
                case 0x0d: {
                    const value = this.readUInt32();
                    this.push(value);
                    debug && this.log("OP_LOAD_UINT32", value);
                    break;
                }
                // Operation::UInt64(_) => 0x0e,
                case 0x0e: {
                    // JavaScript cannot represent full UInt64 range accurately
                    const low = this.readUInt32();
                    const high = this.readUInt32();
                    const value = high * 0x100000000 + low;
                    this.push(value);
                    debug && this.log("OP_LOAD_UINT64", value);
                    break;
                }
                // Operation::LoadBigInt(_) => 0x6d,
                case 0x6d: {
                    this.push(HostBigInt!(this.readString()));
                    debug && this.log("OP_LOAD_BIGINT", this.peek());
                    break;
                }
                // Operation::Pop => 0x04,
                case 0x04: {
                    const popped = this.pop();
                    debug && this.log("OP_POP", popped);
                    break;
                }
                // Operation::Dup => 0x36,
                case 0x36: {
                    const value = this.peek();
                    this.push(value);
                    debug && this.log(`OP_DUP`, value);
                    break;
                }
                // Operation::Dup2 => 0x42,
                case 0x42: {
                    const b = this.stack[this.stack.length - 1];
                    const a = this.stack[this.stack.length - 2];
                    this.push(a);
                    this.push(b);
                    debug && this.log("OP_DUP2");
                    break;
                }

                // ---- operators
                // Operation::EqEq => 0x12,
                case 0x12: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a == b;
                    this.push(result);
                    debug && this.log(`OP_EQ_EQ ${a} == ${b} =>`, result);
                    break;
                }
                // Operation::NotEq => 0x13,
                case 0x13: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a != b;
                    this.push(result);
                    debug && this.log(`OP_NOT_EQ ${a} != ${b} =>`, result);
                    break;
                }
                // Operation::EqEqEq => 0x14,
                case 0x14: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a === b;
                    this.push(result);
                    debug && this.log(`OP_EQ_EQ_EQ ${a} === ${b} =>`, result);
                    break;
                }
                // Operation::NotEqEq => 0x15,
                case 0x15: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a !== b;
                    this.push(result);
                    debug && this.log(`OP_NOT_EQ_EQ ${a} !== ${b} =>`, result);
                    break;
                }
                // Operation::Lt => 0x16,
                case 0x16: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a < b;
                    this.push(result);
                    debug && this.log(`OP_LT ${a} < ${b} =>`, result);
                    break;
                }
                // Operation::LtEq => 0x17,
                case 0x17: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a <= b;
                    this.push(result);
                    debug && this.log(`OP_LT_EQ ${a} <= ${b} =>`, result);
                    break;
                }
                // Operation::Gt => 0x18,
                case 0x18: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a > b;
                    this.push(result);
                    debug && this.log(`OP_GT ${a} > ${b} =>`, result);
                    break;
                }
                // Operation::GtEq => 0x19,
                case 0x19: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >= b;
                    this.push(result);
                    debug && this.log(`OP_GT_EQ ${a} >= ${b} =>`, result);
                    break;
                }
                // Operation::LShift => 0x1a,
                case 0x1a: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a << b;
                    this.push(result);
                    debug && this.log(`OP_LSHIFT ${a} << ${b} =>`, result);
                    break;
                }
                // Operation::RShift => 0x1b,
                case 0x1b: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >> b;
                    this.push(result);
                    debug && this.log(`OP_RSHIFT ${a} >> ${b} =>`, result);
                    break;
                }
                // Operation::ZeroFillRShift => 0x1c,
                case 0x1c: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a >>> b;
                    this.push(result);
                    debug && this.log(`OP_ZERO_FILL_RSHIFT ${a} >>> ${b} =>`, result);
                    break;
                }
                // Operation::Add => 0x1d,
                case 0x1d: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a + b;
                    this.push(result);
                    debug && this.log(`OP_ADD ${a} + ${b} =>`, result);
                    break;
                }
                // Operation::Sub => 0x1e,
                case 0x1e: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a - b;
                    this.push(result);
                    debug && this.log(`OP_SUB ${a} - ${b} =>`, result);
                    break;
                }
                // Operation::Mul => 0x1f,
                case 0x1f: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a * b;
                    this.push(result);
                    debug && this.log(`OP_MUL ${a} * ${b} =>`, result);
                    break;
                }
                // Operation::Div => 0x20,
                case 0x20: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a / b;
                    this.push(result);
                    debug && this.log(`OP_DIV ${a} / ${b} =>`, result);
                    break;
                }
                // Operation::Mod => 0x21,
                case 0x21: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a % b;
                    this.push(result);
                    debug && this.log(`OP_MOD ${a} % ${b} =>`, result);
                    break;
                }
                // Operation::BitOr => 0x22,
                case 0x22: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a | b;
                    this.push(result);
                    debug && this.log(`OP_BIT_OR ${a} | ${b} =>`, result);
                    break;
                }
                // Operation::BitXor => 0x23,
                case 0x23: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a ^ b;
                    this.push(result);
                    debug && this.log(`OP_BIT_XOR ${a} ^ ${b} =>`, result);
                    break;
                }
                // Operation::BitAnd => 0x24,
                case 0x24: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a & b;
                    this.push(result);
                    debug && this.log(`OP_BIT_AND ${a} & ${b} =>`, result);
                    break;
                }
                // Operation::Exp => 0x29,
                case 0x29: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a ** b;
                    this.push(result);
                    debug && this.log(`OP_EXP ${a} ** ${b} =>`, result);
                    break;
                }
                // Operation::In => 0x27,
                case 0x27: {
                    const obj = this.pop();
                    const prop = this.pop();
                    const result = prop in obj;
                    this.push(result);
                    debug && this.log(`OP_IN ${prop} in`, obj, '=>', result);
                    break;
                }
                // Operation::InstanceOf => 0x28,
                case 0x28: {
                    const constructor = this.pop();
                    const obj = this.pop();
                    const result = obj instanceof constructor;
                    this.push(result);
                    debug && this.log(`OP_INSTANCE_OF`, obj, 'instanceof', constructor, '=>', result);
                    break;
                }
                // Operation::UnaryMinus => 0x2d,
                case 0x2d: {
                    const a = this.pop();
                    const result = -a;
                    this.push(result);
                    debug && this.log(`OP_UNARY_MINUS -${a} =>`, result);
                    break;
                }
                // Operation::UnaryPlus => 0x2e,
                case 0x2e: {
                    const a = this.pop();
                    const result = +a;
                    this.push(result);
                    debug && this.log(`OP_UNARY_PLUS +${a} =>`, result);
                    break;
                }
                // Operation::LogicalNot => 0x2f,
                case 0x2f: {
                    const a = this.pop();
                    const result = !a;
                    this.push(result);
                    debug && this.log(`OP_LOGICAL_NOT !${a} =>`, result);
                    break;
                }
                // Operation::BitwiseNot => 0x30,
                case 0x30: {
                    const a = this.pop();
                    const result = ~a;
                    this.push(result);
                    debug && this.log(`OP_BITWISE_NOT ~${a} =>`, result);
                    break;
                }
                // Operation::TypeOf => 0x31,
                case 0x31: {
                    const a = this.pop();
                    const result = typeof a;
                    this.push(result);
                    debug && this.log(`OP_TYPEOF typeof ${a} =>`, result);
                    break;
                }
                // Operation::Void => 0x32,
                case 0x32: {
                    this.pop();
                    this.push(undefined);
                    debug && this.log(`OP_VOID => undefined`);
                    break;
                }
                // Operation::Increment => 0x6e,
                case 0x6e: {
                    const v = this.stack[this.stack.length - 1];
                    this.stack[this.stack.length - 1] = typeof v === 'bigint' ? v + 1n : +v + 1;
                    break;
                }
                // Operation::Decrement => 0x6f,
                case 0x6f: {
                    const v = this.stack[this.stack.length - 1];
                    this.stack[this.stack.length - 1] = typeof v === 'bigint' ? v - 1n : +v - 1;
                    break;
                }
                // Operation::ToNumeric => 0x70,
                case 0x70: {
                    const v = this.stack[this.stack.length - 1];
                    if (typeof v !== 'number' && typeof v !== 'bigint') this.stack[this.stack.length - 1] = +v;
                    break;
                }

                // ---- properties
                // Operation::GetProperty => 0x10,
                case 0x10: {
                    const propName = this.pop();
                    const obj = this.pop();
                    const value = obj[propName];
                    this.push(value);
                    debug && this.log(`OP_GET_PROPERTY ${propName} =`, value);
                    break;
                }
                // Operation::SetProperty => 0x11,
                case 0x11: {
                    let value = this.pop();
                    const propName = this.pop();
                    const obj = this.pop();

                    obj[propName] = value;
                    this.push(value); // SetProperty is an expression that returns the value
                    debug && this.log(`OP_SET_PROPERTY ${propName} =`, value);
                    break;
                }
                // Operation::Delete => 0x33,
                case 0x33: {
                    const propName = this.pop();
                    const obj = this.pop();
                    const result = delete obj[propName];
                    this.push(result);
                    debug && this.log(`OP_DELETE delete ${propName} from`, obj, '=>', result);
                    break;
                }
                // Operation::ToPropertyKey => 0x7f,
                case 0x7f: {
                    this.push(toPropertyKey(this.pop()));
                    break;
                }

                // ---- variables, boxes and the temporal dead zone
                // Operation::GetLocal(_) => 0x2b,
                case 0x2b: {
                    const localIndex = this.readUInt8();
                    const frame = this.frame;
                    const value = frame.locals[localIndex];
                    this.push(value);
                    debug && this.log(`OP_GET_LOCAL index=${localIndex} value=`, value);
                    break;
                }
                // Operation::SetLocal(_) => 0x2c,
                case 0x2c: {
                    const localIndex = this.readUInt8();
                    const value = this.peek();  // Don't pop, assignment is an expression
                    const frame = this.frame;
                    frame.locals[localIndex] = value;
                    debug && this.log(`OP_SET_LOCAL index=${localIndex} value=`, value);
                    break;
                }
                // Operation::GetUpvalue => 0x38,
                case 0x38: {
                    const index = this.readUInt8();
                    const value = this.frame.vf.captures[index].v;
                    this.push(value);
                    debug && this.log(`OP_GET_UPVALUE index=${index} value=`, value);
                    break;
                }
                // Operation::SetUpvalue => 0x39,
                case 0x39: {
                    const index = this.readUInt8();
                    const value = this.peek(); // Don't pop, assignment is an expression
                    this.frame.vf.captures[index].v = value;
                    debug && this.log(`OP_SET_UPVALUE index=${index} value=`, value);
                    break;
                }
                // Operation::GetBox(_) => 0x4c,
                case 0x4c: {
                    const slot = this.readUInt8();
                    const value = this.frame.locals[slot].v;
                    this.push(value);
                    debug && this.log(`OP_GET_BOX slot=${slot} value=`, value);
                    break;
                }
                // Operation::SetBox(_) => 0x4d,
                case 0x4d: {
                    const slot = this.readUInt8();
                    const value = this.peek(); // Don't pop, assignment is an expression
                    this.frame.locals[slot].v = value;
                    debug && this.log(`OP_SET_BOX slot=${slot} value=`, value);
                    break;
                }
                // Operation::NewBox(_) => 0x4e,
                case 0x4e: {
                    const slot = this.readUInt8();
                    this.frame.locals[slot] = { v: undefined };
                    debug && this.log(`OP_NEW_BOX slot=${slot}`);
                    break;
                }
                // Operation::BoxLocal(_) => 0x4f,
                case 0x4f: {
                    const slot = this.readUInt8();
                    const locals = this.frame.locals;
                    locals[slot] = { v: locals[slot] };
                    debug && this.log(`OP_BOX_LOCAL slot=${slot}`);
                    break;
                }
                // Operation::RenewBox(_) => 0x50,
                case 0x50: {
                    const slot = this.readUInt8();
                    const locals = this.frame.locals;
                    locals[slot] = { v: locals[slot].v };
                    debug && this.log(`OP_RENEW_BOX slot=${slot}`);
                    break;
                }
                // Operation::NewBoxTdz(_) => 0x86,
                case 0x86: {
                    this.frame.locals[this.readUInt8()] = { v: TDZ };
                    break;
                }
                // Operation::Tdz(_) => 0x87,
                case 0x87: {
                    this.frame.locals[this.readUInt8()] = TDZ;
                    break;
                }
                // Operation::CheckTdz => 0x88,
                case 0x88: {
                    if (this.stack[this.stack.length - 1] === TDZ) throw tdzError();
                    break;
                }
                // Operation::ThrowTdz => 0x89,
                case 0x89: {
                    throw tdzError();
                }
                // Operation::GetSlotIndex(_, _) => 0x84,
                case 0x84: {
                    const value = this.frame.locals[this.readUInt8()];
                    this.push(value[this.readUInt32()]);
                    break;
                }
                // Operation::GetSlotProp(_, _) => 0x85,
                case 0x85: {
                    const value = this.frame.locals[this.readUInt8()];
                    this.push(value[this.readString()]);
                    break;
                }

                // ---- globals, this, arguments
                // Operation::HostGlobal(_) => 0x40,
                case 0x40: {
                    const flags = this.readUInt8();
                    const isGlobalThis = (flags & HOST_GLOBAL_IS_GLOBAL_THIS) !== 0;
                    const value = this.hostGlobals[isGlobalThis ? 1 : 0];
                    if (value === undefined && !(flags & HOST_GLOBAL_TYPEOF)) {
                        throw new ReferenceError(`${isGlobalThis ? 'globalThis' : 'window'} is not defined`);
                    }
                    this.push(value);
                    debug && this.log("OP_HOST_GLOBAL", isGlobalThis ? 'globalThis' : 'window');
                    break;
                }
                // Operation::This => 0x3f,
                case 0x3f: {
                    const value = this.frame.argsFrame.thisValue;
                    if (value === UNINITIALIZED) throw thisError();
                    this.push(value);
                    debug && this.log("OP_THIS", value);
                    break;
                }
                // Operation::Callee => 0x43,
                case 0x43: {
                    const vf = this.frame.vf;
                    this.push(vf.wrapper || vf);
                    debug && this.log("OP_CALLEE");
                    break;
                }
                // Operation::Arguments => 0x44,
                case 0x44: {
                    const frame = this.frame.argsFrame;
                    if (!frame.argumentsObject) {
                        frame.argumentsObject = makeArguments.apply(undefined, frame.args);
                    }
                    this.push(frame.argumentsObject);
                    debug && this.log("OP_ARGUMENTS");
                    break;
                }
                // Operation::NewTarget => 0x67,
                case 0x67: {
                    this.push(this.frame.argsFrame.newTarget);
                    debug && this.log("OP_NEW_TARGET");
                    break;
                }

                // ---- control flow and exceptions
                // Operation::Jump(_) => 0x3a,
                case 0x3a: {
                    const offset = this.readUInt32();
                    this.ip += offset;
                    debug && this.log(`OP_JUMP +${offset}`);
                    break;
                }
                // Operation::JumpIfFalse(_) => 0x3b,
                case 0x3b: {
                    const offset = this.readUInt32();
                    if (!this.peek()) this.ip += offset;
                    debug && this.log(`OP_JUMP_IF_FALSE +${offset}`);
                    break;
                }
                // Operation::JumpIfTrue(_) => 0x3c,
                case 0x3c: {
                    const offset = this.readUInt32();
                    if (this.peek()) this.ip += offset;
                    debug && this.log(`OP_JUMP_IF_TRUE +${offset}`);
                    break;
                }
                // Operation::JumpIfNotNullish(_) => 0x3d,
                case 0x3d: {
                    const offset = this.readUInt32();
                    const value = this.peek();
                    if (value !== null && value !== undefined) this.ip += offset;
                    debug && this.log(`OP_JUMP_IF_NOT_NULLISH +${offset}`);
                    break;
                }
                // Operation::JumpIfNullish => 0x74,
                case 0x74: {
                    const offset = this.readUInt32();
                    const value = this.peek();
                    if (value === null || value === undefined) this.ip += offset;
                    debug && this.log(`OP_JUMP_IF_NULLISH +${offset}`);
                    break;
                }
                // Operation::Loop(_) => 0x41,
                case 0x41: {
                    const offset = this.readUInt32();
                    this.ip -= offset;
                    debug && this.log(`OP_LOOP -${offset}`);
                    break;
                }
                // Operation::Throw => 0x46,
                case 0x46: {
                    throw this.pop();
                }
                // Operation::TryStart(_) => 0x47,
                case 0x47: {
                    const offset = this.readUInt32();
                    this.handlers.push({
                        callDepth: this.callStack.length,
                        stackLen: this.stack.length,
                        catchIp: this.ip + offset + 1,
                    });
                    debug && this.log("OP_TRY_START");
                    break;
                }
                // Operation::TryEnd => 0x48,
                case 0x48: {
                    this.handlers.pop();
                    debug && this.log("OP_TRY_END");
                    break;
                }

                // ---- calls and functions
                // Operation::Function => 0x37 (host-callable) / 0x4b (internal: only called
                // directly from VM code, so no host wrapper is made). The two opcodes share
                // one decoder; the case decides whether to wrap, so no raw opcode value is
                // read (which a renumbered build would get wrong).
                case 0x37: {
                    const vf = this.readFunction();
                    debug && this.log(`OP_FUNCTION name=${vf.name} arity=${vf.arity}`);
                    this.push(this.createHostWrapper(vf));
                    break;
                }
                case 0x4b: {
                    const vf = this.readFunction();
                    debug && this.log(`OP_FUNCTION name=${vf.name} arity=${vf.arity} internal`);
                    this.push(vf);
                    break;
                }
                // Operation::Call => 0x0f,
                case 0x0f: {
                    const args = this.popArgs();
                    const fn = this.pop();
                    // plain natives keep being called with the host global as `this`
                    this.callFunction(fn, undefined, this.global, args);
                    break;
                }
                // Operation::CallMethod => 0x3e,
                case 0x3e: {
                    const args = this.popArgs();
                    const fn = this.pop();
                    const receiver = this.pop();
                    this.callFunction(fn, receiver, receiver, args);
                    break;
                }
                // Operation::New => 0x45,
                case 0x45: {
                    const args = this.popArgs();
                    const fn = this.pop();
                    if (typeof fn !== 'function') {
                        throw new TypeError(`${fn} is not a constructor`);
                    }
                    const vf = wrappedFunctions.get(fn);
                    if (vf !== undefined && (vf.arrow || vf.method || vf.async)) {
                        throw new TypeError(`${vf.name || 'anonymous'} is not a constructor`);
                    }
                    if (vf === undefined) {
                        this.push(new fn(...args));
                        debug && this.log("OP_NEW");
                        break;
                    }
                    // VM constructor: run it in this loop instead of through its wrapper,
                    // creating `this` the way the host's `new` would (a derived class
                    // gets it from `super(...)`)
                    const frame = this.makeFrame(vf, vf.derived ? UNINITIALIZED : newInstance(fn), args);
                    frame.isConstruct = true;
                    frame.newTarget = fn;
                    this.enterFrame(frame);
                    debug && this.log("OP_NEW virtual function");
                    break;
                }
                // Operation::Return => 0x08,
                case 0x08: {
                    const frame = this.callStack.pop()!;
                    let returnValue = this.pop();
                    if (frame.isConstruct && !isObject(returnValue)) {
                        if (returnValue !== undefined && frame.vf.derived) {
                            throw new TypeError('Derived constructors may only return object or undefined');
                        }
                        returnValue = frame.thisValue;
                        if (returnValue === UNINITIALIZED) throw thisError();
                    }
                    if (frame.coroutine !== null) returnValue = this.finishCoroutine(frame.coroutine, returnValue);

                    // drop handlers left by a `return` from inside `try`
                    this.dropHandlersAbove(this.callStack.length);

                    // remove the returning function's operands (its locals are in `frame.locals`)
                    this.stack.length = frame.stackBase;

                    if (this.callStack.length < entryDepth) {
                        debug && this.log("OP_RETURN (main)", returnValue);
                        return returnValue;
                    }

                    this.push(returnValue);

                    this.resumeCaller();
                    debug && this.log("OP_RETURN", returnValue);
                    break;
                }
                // Operation::SpreadArgs => 0x53,
                case 0x53: {
                    const args = this.pop();
                    for (let i = 0; i < args.length; i++) this.push(args[i]);
                    this.push(args.length);
                    debug && this.log("OP_SPREAD_ARGS", args.length);
                    break;
                }
                // Operation::RestArgs(_) => 0x54,
                case 0x54: {
                    const start = this.readUInt8();
                    const rest = arraySlice.call(this.frame.args, start);
                    this.push(rest);
                    debug && this.log("OP_REST_ARGS", rest);
                    break;
                }

                // ---- array, object, template and regex literals
                // Operation::Regex(_, _) => 0x06,
                case 0x06: {
                    const exp = this.readString();
                    const flags = this.readString();
                    this.push(new RegExp(exp, flags));
                    debug && this.log(`OP_REGEX exp=${exp} flags=${flags}`);
                    break;
                }
                // Operation::Array(_) => 0x34,
                case 0x34: {
                    const count = this.readUInt32();
                    const arr = new Array(count);
                    for (let i = count - 1; i >= 0; i--) {
                        const value = this.pop();
                        if (value !== HOLE) arr[i] = value;
                    }
                    this.push(arr);
                    debug && this.log(`OP_ARRAY count=${count}`, arr);
                    break;
                }
                // Operation::Hole => 0x4a,
                case 0x4a: {
                    this.push(HOLE);
                    break;
                }
                // Operation::ArrayAppend => 0x51,
                case 0x51: {
                    const value = this.pop();
                    const arr = this.peek();
                    if (value === HOLE) arr.length++;
                    else arr.push(value);
                    debug && this.log("OP_ARRAY_APPEND", value);
                    break;
                }
                // Operation::ArraySpread => 0x52,
                case 0x52: {
                    const iterable = this.pop();
                    const arr = this.peek();
                    for (const value of iterable) arr.push(value);
                    debug && this.log("OP_ARRAY_SPREAD", arr);
                    break;
                }
                // Operation::Object(_) => 0x35,
                case 0x35: {
                    const count = this.readUInt32();
                    // pairs are assigned in source order, so later duplicate keys win
                    const base = this.stack.length - count * 2;
                    const obj: any = {};
                    for (let i = base; i < this.stack.length; i += 2) {
                        const value = this.stack[i + 1];
                        obj[this.stack[i]] = value;
                        if (typeof value === 'function') setHomeObject(value, obj);
                    }
                    this.stack.length = base;
                    this.push(obj);
                    debug && this.log(`OP_OBJECT count=${count}`, obj);
                    break;
                }
                // Operation::ObjectSpread => 0x5d,
                case 0x5d: {
                    const source = this.pop();
                    const target = this.peek();
                    if (source !== null && source !== undefined) {
                        const from = Object(source);
                        for (const key of Reflect.ownKeys(from)) {
                            if (propertyIsEnumerable.call(from, key)) {
                                defineProperty(target, key, dataProperty(from[key]));
                            }
                        }
                    }
                    debug && this.log("OP_OBJECT_SPREAD");
                    break;
                }
                // Operation::Template(_) => 0x59,
                case 0x59: {
                    const count = this.readUInt32();
                    const parts = this.stack.splice(this.stack.length - count, count);
                    let result = '';
                    for (let i = 0; i < count; i++) {
                        // values use template conversion (ToString), unlike `+`
                        result += (i & 1) ? `${parts[i]}` : parts[i];
                    }
                    this.push(result);
                    debug && this.log("OP_TEMPLATE", result);
                    break;
                }
                // Operation::TemplateObject(_) => 0x5a,
                case 0x5a: {
                    // one frozen strings array per template site, reused on every run
                    const site = this.bytecode.byteOffset + this.ip;
                    const count = this.readUInt32();
                    const cooked = new Array(count);
                    const raw = new Array(count);
                    for (let i = 0; i < count; i++) {
                        cooked[i] = this.readUInt8() ? this.readString() : undefined;
                        raw[i] = this.readString();
                    }
                    let strings = this.templates.get(site);
                    if (!strings) {
                        strings = cooked;
                        defineProperty(strings, 'raw', { value: Object.freeze(raw) });
                        Object.freeze(strings);
                        this.templates.set(site, strings);
                    }
                    this.push(strings);
                    debug && this.log("OP_TEMPLATE_OBJECT", strings);
                    break;
                }
                // Operation::ForInKeys => 0x49,
                case 0x49: {
                    const obj = this.pop();
                    const keys = [];
                    if (obj !== null && obj !== undefined) {
                        for (const key in obj) keys.push(key);
                    }
                    this.push(keys);
                    debug && this.log("OP_FOR_IN_KEYS", keys);
                    break;
                }

                // ---- destructuring
                // Operation::RequireObjectCoercible => 0x57,
                case 0x57: {
                    const value = this.peek();
                    if (value === null || value === undefined) {
                        throw new TypeError(`Cannot destructure '${value}' as it is ${value}.`);
                    }
                    debug && this.log("OP_REQUIRE_OBJECT_COERCIBLE");
                    break;
                }
                // Operation::IterTake(_) => 0x55,
                case 0x55: {
                    const count = this.readUInt32();
                    this.push(iterTake(this.pop(), count));
                    debug && this.log("OP_ITER_TAKE", count);
                    break;
                }
                // Operation::ArraySliceFrom(_) => 0x56,
                case 0x56: {
                    const start = this.readUInt32();
                    this.push(arraySlice.call(this.pop(), start));
                    debug && this.log("OP_ARRAY_SLICE_FROM", start);
                    break;
                }
                // Operation::ObjectRest => 0x58,
                case 0x58: {
                    const keys = this.pop();
                    const source = Object(this.pop());
                    const used = new Set(keys.map((k: any) => typeof k === 'symbol' ? k : String(k)));
                    const rest: any = {};
                    for (const key of Reflect.ownKeys(source)) {
                        if (!used.has(key) && propertyIsEnumerable.call(source, key)) {
                            rest[key] = (source as any)[key];
                        }
                    }
                    this.push(rest);
                    debug && this.log("OP_OBJECT_REST", rest);
                    break;
                }

                // ---- classes
                // Operation::SetFunctionName(_) => 0x5e,
                case 0x5e: {
                    const prefix = this.readUInt8();
                    nameFunction(this.peek(), this.stack[this.stack.length - 2], prefix);
                    debug && this.log("OP_SET_FUNCTION_NAME");
                    break;
                }
                // Operation::MakeClass(_) => 0x5f,
                case 0x5f: {
                    this.opMakeClass();
                    break;
                }
                // Operation::DefineMethod(_) => 0x60,
                case 0x60: {
                    const kind = this.readUInt8();
                    const fn = this.pop();
                    const key = this.pop();
                    const target = this.pop();
                    defineProperty(target, key, kind === MEMBER_METHOD
                        ? methodProperty(fn)
                        : kind === MEMBER_GETTER
                            ? { get: fn, enumerable: false, configurable: true }
                            : { set: fn, enumerable: false, configurable: true });
                    wrappedFunctions.get(fn).homeObject = target;
                    debug && this.log(`OP_DEFINE_METHOD ${String(key)}`);
                    break;
                }
                // Operation::DefineField => 0x5b,
                case 0x5b: {
                    const value = this.pop();
                    const key = this.pop();
                    defineProperty(this.peek(), key, dataProperty(value));
                    setHomeObject(value, this.peek());
                    debug && this.log(`OP_DEFINE_FIELD ${String(key)}`, value);
                    break;
                }
                // Operation::DefineAccessor(_) => 0x5c,
                case 0x5c: {
                    const isSetter = this.readBool();
                    const fn = this.pop();
                    const key = this.pop();
                    // the other half of an existing accessor is kept
                    defineProperty(this.peek(), key, isSetter
                        ? { set: fn, enumerable: true, configurable: true }
                        : { get: fn, enumerable: true, configurable: true });
                    setHomeObject(fn, this.peek());
                    debug && this.log(`OP_DEFINE_ACCESSOR ${String(key)}`);
                    break;
                }
                // Operation::ClassField => 0x61,
                case 0x61: {
                    let key = this.pop();
                    if (!(key instanceof PrivateName)) key = toPropertyKey(key);
                    wrappedFunctions.get(this.pop()).fieldKeys.push(key);
                    debug && this.log(`OP_CLASS_FIELD ${String(key)}`);
                    break;
                }
                // Operation::StaticField => 0x62,
                case 0x62: {
                    const init = this.pop();
                    const key = this.pop();
                    const cls = this.pop();
                    let value;
                    if (init !== undefined) {
                        init.homeObject = cls;
                        value = this.runFromHost(init, cls, []);
                    }
                    if (key instanceof PrivateName) {
                        key.values!.set(cls, value);
                    } else {
                        defineProperty(cls, toPropertyKey(key), dataProperty(value));
                    }
                    debug && this.log(`OP_STATIC_FIELD ${String(key)}`);
                    break;
                }
                // Operation::StaticBlock => 0x63,
                case 0x63: {
                    const fn = this.pop();
                    const cls = this.pop();
                    fn.homeObject = cls;
                    this.runFromHost(fn, cls, []);
                    debug && this.log("OP_STATIC_BLOCK");
                    break;
                }
                // Operation::InitInstance => 0x80,
                case 0x80: {
                    const frame = this.frame;
                    this.initInstance(frame.vf, frame.thisValue);
                    break;
                }
                // Operation::SuperCallFinish => 0x81,
                case 0x81: {
                    const result = this.pop();
                    const frame = this.frame.argsFrame;
                    if (frame.thisValue !== UNINITIALIZED) {
                        throw new ReferenceError('Super constructor may only be called once');
                    }
                    frame.thisValue = result;
                    this.initInstance(frame.vf, result);
                    break;
                }
                // Operation::InitField(_, _) => 0x82,
                case 0x82: {
                    const index = this.readUInt32();
                    const nameIt = this.readBool();
                    const value = this.pop();
                    const frame = this.frame;
                    const key = frame.vf.fieldKeys[index];
                    const obj = frame.thisValue;
                    if (key instanceof PrivateName) {
                        if (key.values!.has(obj)) throw new TypeError('Cannot initialize a private field twice on the same object');
                        key.values!.set(obj, value);
                    } else {
                        if (nameIt) nameFunction(value, key, MEMBER_METHOD);
                        defineProperty(obj, key, dataProperty(value));
                    }
                    break;
                }
                // Operation::ClassInit => 0x83,
                case 0x83: {
                    const fn = this.pop();
                    const classVf = wrappedFunctions.get(this.pop());
                    fn.homeObject = classVf.homeObject;
                    fn.fieldKeys = classVf.fieldKeys;
                    classVf.initFn = fn;
                    break;
                }
                // Operation::SuperCall => 0x64,
                case 0x64: {
                    this.opSuperCall();
                    break;
                }
                // Operation::SuperGet => 0x65,
                case 0x65: {
                    const key = this.pop();
                    const frame = this.frame.argsFrame;
                    if (frame.thisValue === UNINITIALIZED) throw thisError();
                    const home = frame.vf.homeObject;
                    if (home === null) throw new SyntaxError("'super' keyword unexpected here");
                    const value = Reflect.get(Object.getPrototypeOf(home), key, frame.thisValue);
                    this.push(value);
                    debug && this.log(`OP_SUPER_GET ${String(key)}`, value);
                    break;
                }
                // Operation::SuperSet => 0x66,
                case 0x66: {
                    const value = this.pop();
                    const key = this.pop();
                    const frame = this.frame.argsFrame;
                    if (frame.thisValue === UNINITIALIZED) throw thisError();
                    const home = frame.vf.homeObject;
                    if (home === null) throw new SyntaxError("'super' keyword unexpected here");
                    if (!Reflect.set(Object.getPrototypeOf(home), key, value, frame.thisValue)) {
                        throw new TypeError(`Cannot assign to read only property '${String(key)}'`);
                    }
                    this.push(value);
                    debug && this.log(`OP_SUPER_SET ${String(key)}`, value);
                    break;
                }
                // Operation::NewPrivateName(_) => 0x68,
                case 0x68: {
                    this.opNewPrivateName();
                    break;
                }
                // Operation::PrivateGet => 0x69,
                case 0x69: {
                    this.opPrivateGet();
                    break;
                }
                // Operation::PrivateSet => 0x6a,
                case 0x6a: {
                    this.opPrivateSet();
                    break;
                }
                // Operation::PrivateIn => 0x6b,
                case 0x6b: {
                    this.opPrivateIn();
                    break;
                }
                // Operation::DefinePrivateMethod(_) => 0x6c,
                case 0x6c: {
                    this.opDefinePrivateMethod();
                    break;
                }

                // ---- iteration
                // Operation::GetIterator => 0x71,
                case 0x71: {
                    this.push(getIterator(this.pop()));
                    debug && this.log("OP_GET_ITERATOR");
                    break;
                }
                // Operation::IterNext(_, _) => 0x72,
                case 0x72: {
                    const slot = this.readUInt8();
                    const offset = this.readUInt32();
                    const value = iterStep(this.frame.locals[slot]);
                    if (value === ITER_DONE) this.ip += offset;
                    else this.push(value);
                    debug && this.log("OP_ITER_NEXT", value);
                    break;
                }
                // Operation::IterClose(_, _) => 0x73,
                case 0x73: {
                    const slot = this.readUInt8();
                    const quiet = this.readUInt8() === 1;
                    iterClose(this.frame.locals[slot], quiet);
                    debug && this.log("OP_ITER_CLOSE");
                    break;
                }

                // ---- async functions
                // Operation::Await => 0x77,
                case 0x77: {
                    const result = this.opAwait();
                    if (this.callStack.length < entryDepth) return result;
                    this.push(result);
                    this.resumeCaller();
                    break;
                }
                default: {
                    throw new Error(`Unknown opcode 0x${op.toString(16)} at ${this.ip}`);
                }
            }
        }
        this.printStack();
    }

    // Decode a Function instruction's operands into a VirtualFunction (shared by the
    // host-callable and internal opcodes).
    readFunction() {
        const arity = this.readUInt8();
        const params = this.readUInt8();
        const flags = this.readUInt8();
        const name = this.readString();
        const currentFrame = this.frame;
        // captures: count, then (from_local, index): a boxed local slot of this frame, or
        // one of this closure's own captures
        const captureCount = this.readUInt8();
        const captures = new Array(captureCount);
        for (let i = 0; i < captureCount; i++) {
            const fromLocal = this.readUInt8();
            const index = this.readUInt8();
            captures[i] = fromLocal
                ? currentFrame.locals[index]
                : currentFrame.vf.captures[index];
        }
        const codeLen = this.readUInt32();
        const code = this.bytecode.subarray(this.ip + 1, this.ip + 1 + codeLen);
        this.ip += codeLen;

        const vf = new VirtualFunction(arity, params, flags, name, code, captures);
        if (vf.arrow) {
            vf.argsFrame = currentFrame.argsFrame;
        }
        return vf;
    }

    // ---- instructions kept out of `execute` so it stays small for V8 (log lines here read `this.debug`)

    opAwait() {
        // (resolving first: a throwing `then` / `constructor` throws right here)
        const promise = resolved(this.pop());
        const co = this.frame.coroutine!;
        co.throwIp = this.ip + 1;
        co.normalIp = this.ip + 2;
        co.state = CO_AWAITING;
        this.suspendFrame(co);
        this.awaitPromise(co, promise);
        this.debug && this.log("OP_AWAIT");
        return co.promise;
    }

    opMakeClass() {
        const hasSuper = this.readBool();
        const cls = this.pop();
        const proto = cls.prototype;
        if (hasSuper) {
            const parent = this.pop();
            if (parent === null) {
                Object.setPrototypeOf(proto, null);
            } else {
                if (!isConstructor(parent)) {
                    throw new TypeError(`Class extends value ${typeof parent === 'symbol' ? 'Symbol()' : String(parent)} is not a constructor or null`);
                }
                const parentProto = parent.prototype;
                if (parentProto !== null && !isObject(parentProto)) {
                    throw new TypeError(`Class extends value does not have valid prototype property ${String(parentProto)}`);
                }
                Object.setPrototypeOf(cls, parent);
                Object.setPrototypeOf(proto, parentProto);
            }
        }
        defineProperty(cls, 'prototype', { writable: false });
        const classVf = wrappedFunctions.get(cls);
        classVf.homeObject = proto;
        classVf.fieldKeys = [];
        this.push(cls);
        this.debug && this.log("OP_MAKE_CLASS", cls.name);
    }

    opSuperCall() {
        const args = this.popArgs();
        const frame = this.frame.argsFrame;
        const parent = Object.getPrototypeOf(frame.vf.wrapper);
        const parentVf = wrappedFunctions.get(parent);
        if (parentVf !== undefined && parentVf.isClass) {
            // a VM parent class runs in this loop; `SuperCallFinish` takes its result
            const obj = parentVf.derived ? UNINITIALIZED : newInstance(frame.newTarget);
            const parentFrame = this.makeFrame(parentVf, obj, args);
            parentFrame.isConstruct = true;
            parentFrame.newTarget = frame.newTarget;
            this.enterFrame(parentFrame);
            this.debug && this.log("OP_SUPER_CALL virtual class");
            return;
        }
        if (!isConstructor(parent)) {
            throw new TypeError('Super constructor is not a constructor');
        }
        this.push(Reflect.construct(parent, args, frame.newTarget));
        this.debug && this.log("OP_SUPER_CALL");
    }

    opNewPrivateName() {
        this.push(new PrivateName(this.readUInt8()));
        this.debug && this.log("OP_NEW_PRIVATE_NAME");
    }

    opPrivateGet() {
        const name = this.pop();
        const obj = this.pop();
        if (name.kind === PRIVATE_FIELD) {
            const value = name.values.get(obj);
            if (value === undefined && !name.values.has(obj)) throw privateError();
            this.push(value);
        } else {
            if (name.brand === null || !name.brand.has(obj)) throw privateError();
            if (name.kind === PRIVATE_METHOD) {
                this.push(name.method);
            } else if (name.method === undefined) {
                throw new TypeError("Private accessor was defined without a getter");
            } else {
                // runs in this loop; its result is pushed when it returns
                this.callFunction(name.method, obj, obj, []);
            }
        }
        this.debug && this.log("OP_PRIVATE_GET");
    }

    opPrivateSet() {
        const value = this.pop();
        const name = this.pop();
        const obj = this.pop();
        if (name.kind === PRIVATE_FIELD) {
            if (!name.values.has(obj)) throw privateError();
            name.values.set(obj, value);
            this.push(value);
        } else {
            if (name.brand === null || !name.brand.has(obj)) throw privateError();
            if (name.kind === PRIVATE_METHOD) {
                throw new TypeError('Private method is not writable');
            }
            if (name.setter === undefined) {
                throw new TypeError("Private accessor was defined without a setter");
            }
            // [value, setter result]: the compiler pops the result
            this.push(value);
            this.callFunction(name.setter, obj, obj, [value]);
        }
        this.debug && this.log("OP_PRIVATE_SET");
    }

    opPrivateIn() {
        const obj = this.pop();
        const name = this.pop();
        if (!isObject(obj)) {
            throw new TypeError("Cannot use 'in' operator to search for a private name in a non-object");
        }
        this.push(name.kind === PRIVATE_FIELD
            ? name.values.has(obj)
            : name.brand !== null && name.brand.has(obj));
        this.debug && this.log("OP_PRIVATE_IN");
    }

    opDefinePrivateMethod() {
        const flags = this.readUInt8();
        const fn = this.pop();
        const name = this.pop();
        const cls = this.pop();
        // a method, or an accessor's getter / setter
        if ((flags & PRIVATE_DEFINE_KIND_MASK) === MEMBER_SETTER) name.setter = fn;
        else name.method = fn;
        const vf = fn instanceof VirtualFunction ? fn : wrappedFunctions.get(fn);
        if (flags & PRIVATE_DEFINE_STATIC) {
            // static: only the class itself has it
            vf.homeObject = cls;
            if (name.brand === null) name.brand = new WeakSet();
            name.brand.add(cls);
        } else {
            // instances join the class's brand when they're constructed
            const classVf = wrappedFunctions.get(cls);
            vf.homeObject = cls.prototype;
            if (classVf.brand === null) classVf.brand = new WeakSet();
            name.brand = classVf.brand;
        }
        this.debug && this.log("OP_DEFINE_PRIVATE_METHOD");
    }

    // ---- calls, frames and host interop

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
    // A VM function gets a frame in this loop; a host function runs now and its result is
    // pushed.
    callFunction(fn: any, vmThis: any, nativeThis: any, args: any[]) {
        // (internal functions are called as bare VirtualFunctions, others through a wrapper)
        const vf = fn instanceof VirtualFunction ? fn : typeof fn === 'function' ? wrappedFunctions.get(fn) : undefined;
        if (vf !== undefined) {
            if (vf.isClass) {
                throw new TypeError(`Class constructor ${vf.name} cannot be invoked without 'new'`);
            }
            this.enterFrame(this.makeFrame(vf, vmThis, args));
            this.debug && this.log(`OP_CALL virtual function ${vf.name} arity=${vf.arity} args=${args.length}`);
        } else if (typeof fn === 'function') {
            const result = fn.apply(nativeThis, args);
            this.push(result);
            this.debug && this.log("OP_CALL native function", result);
        } else {
            throw new TypeError(`${fn} is not a function`);
        }
    }

    // A new frame for calling `vf`.
    makeFrame(vf: VirtualFunction, thisValue: any, args: any) {
        const frame = new StackFrame(vf, this.stack.length);
        frame.args = args;
        if (vf.arrow) {
            frame.argsFrame = vf.argsFrame!;
        } else {
            frame.thisValue = thisValue;
        }
        // only parameter slots come from arguments; later slots are the function's own
        const paramCount = Math.min(args.length, vf.params);
        for (let i = 0; i < paramCount; i++) {
            frame.locals[i] = args[i];
        }
        if (vf.async) {
            const co = new Coroutine(this, frame);
            co.promise = new HostPromise((resolve, reject) => {
                co.resolve = resolve;
                co.reject = reject;
            });
            frame.coroutine = co;
        }
        return frame;
    }

    // Set up a new instance of a class: its private methods first, then its fields, whose
    // method runs in this loop and gives back the instance. Pushes the instance.
    initInstance(vf: VirtualFunction, obj: any) {
        const brand = vf.brand;
        if (brand !== null) {
            if (brand.has(obj)) throw new TypeError('Cannot initialize private methods twice on the same object');
            brand.add(obj);
        }
        const init = vf.initFn;
        if (init === null) {
            this.push(obj);
            return;
        }
        this.enterFrame(this.makeFrame(init, obj, EMPTY));
    }

    // Run a VM function for a host caller, in a nested `run()`. `construct` runs it as
    // `new` (for class constructors, which set up fields first).
    runFromHost(vf: VirtualFunction, thisValue: any, args: any, newTarget?: any, construct?: boolean) {
        const savedIp = this.ip;
        const savedBytecode = this.bytecode;
        const savedDepth = this.callStack.length;
        const savedStackLen = this.stack.length;

        const frame = this.makeFrame(vf, thisValue, args);
        frame.newTarget = newTarget;
        frame.isConstruct = construct === true;
        this.callStack.push(frame);
        this.continueAt(0);

        try {
            return this.run();
        } finally {
            // also restores state when the function throws
            this.restoreState(savedIp, savedBytecode, savedDepth, savedStackLen);
        }
    }

    // Put the VM back as it was before a nested `run()`.
    restoreState(ip: number, bytecode: Uint8Array, callDepth: number, stackLen: number) {
        this.ip = ip;
        this.bytecode = bytecode;
        this.callStack.length = callDepth;
        this.stack.length = stackLen;
        this.dropHandlersAbove(callDepth);
    }

    // A host function that runs a VM function, so natives, `call`/`apply`/`bind`, `new`
    // and `instanceof` all work on it. Only made for functions that can leave VM code.
    // Arrows and methods get host arrows / methods: not constructible, no `prototype`.
    createHostWrapper(vf: VirtualFunction) {
        let kind = vf.arrow ? FN_ARROW : vf.method ? FN_METHOD : FN_NORMAL;
        // async functions can't be used with `new` (method wrappers can't)
        if (vf.async && kind === FN_NORMAL) kind = FN_METHOD;
        const makers = WRAPPERS[kind];
        let wrapper;
        if (vf.isClass) {
            wrapper = makeClassWrapper(this, vf, vf.name);
            defineProperty(wrapper, 'length', { value: vf.arity });
        } else if (vf.arity < makers.length) {
            wrapper = makers[vf.arity](this, vf, vf.name);
        } else {
            wrapper = makers[0](this, vf, vf.name);
            defineProperty(wrapper, 'length', { value: vf.arity });
        }
        if (vf.async) Object.setPrototypeOf(wrapper, AsyncFunctionPrototype);
        wrappedFunctions.set(wrapper, vf);
        vf.wrapper = wrapper;
        return wrapper;
    }

    // ---- coroutines

    // Take a suspending coroutine's frame (on top) off the call stack, saving its
    // operands and exception handlers.
    suspendFrame(co: Coroutine) {
        const frame = co.frame;
        const depth = this.callStack.length;
        co.stack = this.stack.length > frame.stackBase ? this.stack.slice(frame.stackBase) : EMPTY;
        this.stack.length = frame.stackBase;
        let saved = null;
        while (this.handlers.length && this.handlers[this.handlers.length - 1].callDepth === depth) {
            const h = this.handlers.pop()!;
            (saved || (saved = [])).push({ rel: h.stackLen - frame.stackBase, catchIp: h.catchIp });
        }
        co.handlers = saved;
        this.callStack.pop();
    }

    // Put a suspended coroutine's frame back on top of the call stack, continuing at `ip`.
    restoreFrame(co: Coroutine, ip: number) {
        const frame = co.frame;
        frame.stackBase = this.stack.length;
        const saved = co.stack!;
        for (let i = 0; i < saved.length; i++) this.stack.push(saved[i]);
        co.stack = null;
        const depth = this.callStack.length + 1;
        if (co.handlers !== null) {
            for (let i = co.handlers.length - 1; i >= 0; i--) {
                const h = co.handlers[i];
                this.handlers.push({ callDepth: depth, stackLen: frame.stackBase + h.rel, catchIp: h.catchIp });
            }
            co.handlers = null;
        }
        this.callStack.push(frame);
        this.continueAt(ip);
        co.state = CO_RUNNING;
    }

    // A coroutine's frame returned `value`: resolve its promise, which its caller gets.
    finishCoroutine(co: Coroutine, value: any) {
        co.state = CO_DONE;
        co.resolve(value);
        return co.promise;
    }

    // Resume `co` once an awaited promise settles. (Kept out of `execute`: a closure there
    // capturing `this` makes V8 slow down the whole loop.)
    awaitPromise(co: Coroutine, promise: any) {
        // (the callbacks return nothing: the function's own promise carries the result)
        promiseThen.call(promise,
            v => { this.resumeNested(co, RESUME_NEXT, v); },
            e => { this.resumeNested(co, RESUME_THROW, e); });
    }

    // Resume a suspended coroutine in a nested `run()` with a sent value, returning what
    // it gives back when it next suspends or finishes.
    resumeNested(co: Coroutine, mode: number, value: any) {
        const ip = co.resumeIp(mode);
        const savedIp = this.ip;
        const savedBytecode = this.bytecode;
        const savedDepth = this.callStack.length;
        const savedStackLen = this.stack.length;
        this.restoreFrame(co, ip);
        this.push(value);
        try {
            return this.run();
        } finally {
            this.restoreState(savedIp, savedBytecode, savedDepth, savedStackLen);
        }
    }
}

declare const module: any;
if (typeof module !== 'undefined' && module.exports) {
    module.exports = { VM };
}