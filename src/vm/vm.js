'use strict';

// function kinds (`FnKind` in the compiler)
const FN_NORMAL = 0;
const FN_ARROW = 1;
const FN_METHOD = 2;
const FN_CLASS = 3;
const FN_DERIVED_CLASS = 4;

// a `let` / `const` / `class` binding that a closure may read before it's initialised
const TDZ = {};
const tdzError = () => new ReferenceError('Cannot access a variable before initialization');

// `this` in a derived class constructor before `super(...)` has run
const UNINITIALIZED = {};
const thisError = () => new ReferenceError(
    "Must call super constructor in derived class before accessing 'this' or returning from derived constructor");

// host wrapper -> its VM function, stored as a JS private field stamped onto the wrapper
// (a base constructor returning another object lets a subclass add its private fields to
// that object). Scripts can't see or reach it: no property or symbol appears on the
// function, unlike a normal property. Faster to add than a WeakMap entry.
const ReturnTarget = class {
    constructor(target) {
        return target;
    }
};
const WRAPPED = class extends ReturnTarget {
    #vf;
    constructor(wrapper, vf) {
        super(wrapper);
        this.#vf = vf;
    }
    static set(wrapper, vf) {
        new WRAPPED(wrapper, vf);
    }
    // the VM function behind a host wrapper, or undefined for any other value
    static get(value) {
        return typeof value === 'function' && #vf in value ? value.#vf : undefined;
    }
};

// a VM method (getter, setter, class member) remembers the object it was defined on, for `super`
const setHomeObject = function (fn, home) {
    if (typeof fn === 'function') {
        const vf = WRAPPED.get(fn);
        if (vf !== undefined && vf.method) vf.homeObject = home;
    }
};

// Name a VM function after a property key (computed keys), prefixed by 1 `get ` / 2 `set `.
const nameFunction = function (fn, key, prefix) {
    const vf = WRAPPED.get(fn);
    if (vf === undefined) return;
    let name = typeof key === 'symbol'
        ? (key.description === undefined ? '' : `[${key.description}]`)
        : String(key);
    if (prefix) name = (prefix === 1 ? 'get ' : 'set ') + name;
    vf.name = name;
    defineProperty(fn, 'name', { value: name, configurable: true });
};

const dataProperty = function (value) {
    return { value, writable: true, enumerable: true, configurable: true };
};

// whether `new f()` is allowed (the standard check via Reflect.construct's newTarget)
const isConstructor = function (f) {
    if (typeof f !== 'function') return false;
    try {
        Reflect.construct(String, [], f);
        return true;
    } catch (e) {
        return false;
    }
};

// captured once, like the other built-ins the VM relies on (only used for `1n` literals)
const HostBigInt = typeof BigInt === 'function' ? BigInt : undefined;

const toPropertyKey = key => typeof key === 'symbol' ? key : String(key);

// captured once, so input scripts can't change how destructuring behaves
const arraySlice = Array.prototype.slice;
const arrayValues = Array.prototype.values;
const propertyIsEnumerable = Object.prototype.propertyIsEnumerable;
const defineProperty = Object.defineProperty;
const ITER_TAKE_ALL = 0xffffffff;

// The first `count` values of an iterable (array patterns), closing the iterator early
// like JS does. Arrays with the normal iterator are read directly.
const iterTake = function (iterable, count) {
    if (Array.isArray(iterable) && iterable[Symbol.iterator] === arrayValues) {
        return iterable;
    }
    if (iterable === null || iterable === undefined ||
        typeof iterable[Symbol.iterator] !== 'function') {
        throw new TypeError(`${typeof iterable === 'string' ? iterable : typeof iterable} is not iterable`);
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
};

const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');

// `for-of` iteration state. Arrays using the built-in iterator are walked by index (the
// same values and order the iterator would give, without creating it).
const ArrayIteratorPrototype = Object.getPrototypeOf([][Symbol.iterator]());
const arrayIteratorNext = ArrayIteratorPrototype.next;
class IterRecord {
    constructor(iterator, next, array) {
        this.iterator = iterator;
        this.next = next;
        this.array = array;
        this.index = 0;
        this.done = false;
        this.sync = false; // an async iteration over a sync iterator (values are awaited)
        // a VM generator resumed inline by `for-of` leaves its next value (or ITER_DONE)
        // here; a `yield*` result object goes in `pendingResult`
        this.pending = NO_VALUE;
        this.pendingResult = null;
    }
}
// `IterRecord.pending` when there's nothing waiting
const NO_VALUE = {};
// returned by `iterStep` when the iterator is done
const ITER_DONE = {};

const getIterator = function (iterable) {
    if (Array.isArray(iterable) && iterable[Symbol.iterator] === arrayValues &&
        ArrayIteratorPrototype.next === arrayIteratorNext) {
        return new IterRecord(null, null, iterable);
    }
    const method = iterable === null || iterable === undefined ? undefined : iterable[Symbol.iterator];
    if (typeof method !== 'function') {
        throw new TypeError(`${typeof iterable === 'string' ? iterable : typeof iterable} is not iterable`);
    }
    const iterator = method.call(iterable);
    if (!isObject(iterator)) throw new TypeError('Result of the Symbol.iterator method is not an object');
    return new IterRecord(iterator, iterator.next, null);
};

// The next value, or ITER_DONE. An iterator that throws or finishes is done (not closed).
const iterStep = function (record) {
    if (record.array !== null) {
        if (record.index < record.array.length) return record.array[record.index++];
        record.done = true;
        return ITER_DONE;
    }
    let result;
    try {
        result = record.next.call(record.iterator);
        if (!isObject(result)) throw new TypeError(`Iterator result ${String(result)} is not an object`);
        if (result.done) {
            record.done = true;
            return ITER_DONE;
        }
        return result.value;
    } catch (e) {
        record.done = true;
        throw e;
    }
};

// Close an iterator left early: call its `return()`. Leaving by a throw (`quiet`) ignores
// errors from it, so the original error wins.
const iterClose = function (record, quiet) {
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
    if (!isObject(ret.call(iterator))) throw new TypeError('Iterator result is not an object');
};

// Host intrinsics for generators and async functions (read once, never patched)
const GeneratorFunctionPrototype = Object.getPrototypeOf(function* () {});
const AsyncFunctionPrototype = Object.getPrototypeOf(async function () {});
const AsyncGeneratorFunctionPrototype = Object.getPrototypeOf(async function* () {});
const IteratorPrototype = Object.getPrototypeOf(GeneratorFunctionPrototype.prototype);
const AsyncIteratorPrototype = Object.getPrototypeOf(AsyncGeneratorFunctionPrototype.prototype);
const HostPromise = Promise;
const promiseResolve = Promise.resolve;
const promiseThen = Promise.prototype.then;
const resolved = value => promiseResolve.call(HostPromise, value);

// how a suspended coroutine is resumed: next(v) / throw(e) / return(v)
const RESUME_NEXT = 0;
const RESUME_THROW = 1;
const RESUME_RETURN = 2;

// coroutine states
const CO_START = 0;      // generator made, body not started
const CO_SUSPENDED = 1;  // stopped at a `yield`
const CO_RUNNING = 2;
const CO_DONE = 3;
const CO_AWAITING = 4;   // stopped at an `await`
const CO_AWAITING_RETURN = 5; // async generator: awaiting a `return(v)` value after finishing

// A generator / async function's suspendable state. While it's suspended its frame is off
// the call stack, with its operands and exception handlers saved here.
class Coroutine {
    constructor(vm, frame, func) {
        this.vm = vm;
        this.frame = frame;
        this.generator = func.generator;
        this.async = func.async;
        this.state = CO_RUNNING;
        this.stack = null;    // saved operands
        this.handlers = null; // saved handlers: { rel (stack offset from base), catchIp }
        // where to continue (next instruction) for each way of resuming
        this.normalIp = 0;
        this.throwIp = 0;
        this.returnIp = 0;
        // async functions: the promise the call returned
        this.promise = null;
        this.resolve = null;
        this.reject = null;
        // async generators: pending next / throw / return requests
        this.queue = null;
        // a `for-of` record resuming this generator inline: gets the next value directly
        this.forOf = null;
    }
}

// a hidden per-object slot: a JS private field stamped onto any object (see WRAPPED)
const hiddenSlot = function () {
    return class extends ReturnTarget {
        #value;
        constructor(target, value) {
            super(target);
            this.#value = value;
        }
        static set(target, value) {
            new this(target, value);
        }
        static get(target) {
            return isObject(target) && #value in target ? target.#value : undefined;
        }
    };
};
// generator object -> its coroutine
const COROUTINES = hiddenSlot();

// the generator objects' `next` / `throw` / `return` (VM code calling them resumes inline)
const syncCoroutine = function (gen, method) {
    const co = COROUTINES.get(gen);
    if (co === undefined || co.async) {
        throw new TypeError(`${method} method called on incompatible receiver ${String(gen)}`);
    }
    return co;
};
const genNext = ({ next(v) { return syncCoroutine(this, 'next').vm.generatorResume(COROUTINES.get(this), RESUME_NEXT, v, false); } }).next;
const genReturn = ({ return(v) { return syncCoroutine(this, 'return').vm.generatorResume(COROUTINES.get(this), RESUME_RETURN, v, false); } }).return;
const genThrow = ({ throw(e) { return syncCoroutine(this, 'throw').vm.generatorResume(COROUTINES.get(this), RESUME_THROW, e, false); } }).throw;
const methodProperty = value => ({ value, writable: true, enumerable: false, configurable: true });
const KumoGeneratorPrototype = Object.create(IteratorPrototype, {
    next: methodProperty(genNext),
    return: methodProperty(genReturn),
    throw: methodProperty(genThrow),
    [Symbol.toStringTag]: { value: 'Generator', configurable: true },
});

// async generator requests are queued and run one at a time
const asyncGenRequest = function (gen, mode, value, method) {
    return new HostPromise((resolve, reject) => {
        const co = COROUTINES.get(gen);
        if (co === undefined || !co.async || !co.generator) {
            reject(new TypeError(`${method} method called on incompatible receiver ${String(gen)}`));
            return;
        }
        co.queue.push({ mode, value, resolve, reject });
        co.vm.drainAsyncGen(co);
    });
};
const KumoAsyncGeneratorPrototype = Object.create(AsyncIteratorPrototype, {
    next: methodProperty(({ next(v) { return asyncGenRequest(this, RESUME_NEXT, v, 'next'); } }).next),
    return: methodProperty(({ return(v) { return asyncGenRequest(this, RESUME_RETURN, v, 'return'); } }).return),
    throw: methodProperty(({ throw(e) { return asyncGenRequest(this, RESUME_THROW, e, 'throw'); } }).throw),
    [Symbol.toStringTag]: { value: 'AsyncGenerator', configurable: true },
});

// returned by `generatorResume` when it resumed the generator in the running loop
const RESUMED_INLINE = {};
// returned by `delegateCall` / `asyncIterClose` when there's nothing to call
const NOTHING = {};
const EMPTY = [];

const iterResult = (value, done) => ({ value, done });

// async-from-sync iteration: a sync result whose value is awaited
const asyncFromSync = function (result) {
    const done = !!result.done;
    return promiseThen.call(resolved(result.value), v => iterResult(v, done));
};

// a sync iterator record's next result (for async use, and `yield*`)
const syncNextResult = function (record, value) {
    if (record.array !== null) {
        const v = iterStep(record);
        return v === ITER_DONE ? iterResult(undefined, true) : iterResult(v, false);
    }
    const r = record.next.call(record.iterator, value);
    if (!isObject(r)) throw new TypeError(`Iterator result ${String(r)} is not an object`);
    return r;
};

// `for await`: the promise (or value) of the next result
const asyncIterNext = function (record) {
    try {
        if (record.sync) return asyncFromSync(syncNextResult(record, undefined));
        return record.next.call(record.iterator);
    } catch (e) {
        record.done = true;
        throw e;
    }
};

// `for await` closing: what to await from `return()`, or NOTHING. Leaving by a throw
// (`quiet`) ignores errors, including a rejection.
const asyncIterClose = function (record, quiet) {
    if (record.done) return NOTHING;
    record.done = true;
    let result;
    try {
        if (record.sync) {
            // async-from-sync iterators always have `return`
            const ret = record.array !== null ? undefined : record.iterator.return;
            if (ret === undefined || ret === null) {
                result = resolved(iterResult(undefined, true));
            } else {
                const r = ret.call(record.iterator);
                if (!isObject(r)) throw new TypeError('Iterator result is not an object');
                result = asyncFromSync(r);
            }
        } else {
            const ret = record.iterator.return;
            if (ret === undefined || ret === null) return NOTHING;
            result = ret.call(record.iterator);
        }
    } catch (e) {
        if (quiet) return NOTHING;
        throw e;
    }
    return quiet ? promiseThen.call(resolved(result), undefined, () => undefined) : result;
};

// scratch space for decoding f64 literals
const F64 = new Float64Array(1);
const F64_BYTES = new Uint8Array(F64.buffer);
const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

// marks an array literal hole (`[1, , 3]`) on the stack
const HOLE = {};

// `arguments` objects are made by a real function, so they look and behave like the host's
const makeArguments = function () {
    return arguments;
};

// Host wrappers by kind and parameter count, so `length` is right without being set
// afterwards (which is slow). Normal functions and methods pass their `arguments`;
// arrows have none, but padding missing arguments with `undefined` can't be observed.
// Each is made under a computed key `n` so it gets the function's name from JS itself
// (setting `name` afterwards with defineProperty is slow).
const WRAPPERS = [
    [
        (vm, f, n) => ({ [n]: function () { return vm.runFromHost(f, this, arguments, new.target); } })[n],
        (vm, f, n) => ({ [n]: function (a) { return vm.runFromHost(f, this, arguments, new.target); } })[n],
        (vm, f, n) => ({ [n]: function (a, b) { return vm.runFromHost(f, this, arguments, new.target); } })[n],
        (vm, f, n) => ({ [n]: function (a, b, c) { return vm.runFromHost(f, this, arguments, new.target); } })[n],
    ],
    [
        (vm, f, n) => ({ [n]: (...r) => vm.runFromHost(f, undefined, r) })[n],
        (vm, f, n) => ({ [n]: (a, ...r) => vm.runFromHost(f, undefined, [a, ...r]) })[n],
        (vm, f, n) => ({ [n]: (a, b, ...r) => vm.runFromHost(f, undefined, [a, b, ...r]) })[n],
        (vm, f, n) => ({ [n]: (a, b, c, ...r) => vm.runFromHost(f, undefined, [a, b, c, ...r]) })[n],
    ],
    [
        (vm, f, n) => ({ [n]() { return vm.runFromHost(f, this, arguments); } })[n],
        (vm, f, n) => ({ [n](a) { return vm.runFromHost(f, this, arguments); } })[n],
        (vm, f, n) => ({ [n](a, b) { return vm.runFromHost(f, this, arguments); } })[n],
        (vm, f, n) => ({ [n](a, b, c) { return vm.runFromHost(f, this, arguments); } })[n],
    ],
];

// Class constructors need `new`; a base class uses the `this` the host's `new` made,
// a derived class gets its `this` from `super(...)`.
const makeClassWrapper = (vm, f, n) => ({
    [n]: function () {
        if (new.target === undefined) {
            throw new TypeError(`Class constructor ${f.name} cannot be invoked without 'new'`);
        }
        return vm.runFromHost(f, f.derived ? UNINITIALIZED : this, arguments, new.target, true);
    },
})[n];

// private name kinds (`PrivateKind` in the compiler)
const PRIVATE_FIELD = 0;
const PRIVATE_METHOD = 1;
const PRIVATE_ACCESSOR = 2;

// A class's private name (`#x`), made fresh each time the class body is evaluated. Field
// values live in a WeakMap, so they never appear on the object (no reflection, works on
// frozen objects and doesn't touch proxies). Methods and accessors are shared: an object
// may use them once it's in the brand (the class's WeakSet).
class PrivateName {
    constructor(kind) {
        this.kind = kind;
        this.values = kind === PRIVATE_FIELD ? new WeakMap() : null;
        this.brand = null;
        this.method = undefined; // a method, or an accessor's getter
        this.setter = undefined;
    }
}

const privateError = () => new TypeError('Cannot access private member from an object whose class did not declare it');

class VirtualFunction {
    constructor(arity, params, flags, name, code, captures) {
        const kind = flags & 7;
        this.generator = (flags & 8) !== 0; // `function*`: calling it makes a generator
        this.async = (flags & 16) !== 0;    // `async`: calling it returns a promise
        this.genPrototype = null; // an unwrapped generator function's `prototype`
        this.arity = arity;   // the function's `length`
        this.params = params; // parameter slots filled from the call's arguments
        this.arrow = kind === FN_ARROW; // arrows use `this` and `arguments` from where they were made
        this.method = kind === FN_METHOD; // methods (and arrows) can't be used with `new`
        this.isClass = kind === FN_CLASS || kind === FN_DERIVED_CLASS; // only callable with `new`
        this.derived = kind === FN_DERIVED_CLASS; // `this` comes from `super(...)`
        this.fieldKeys = null; // a class's instance field keys (property keys / private names)
        this.initFn = null;    // a class's method setting up instance field values
        this.brand = null;    // a class's WeakSet of instances, if it has private methods
        this.homeObject = null; // where `super` lookups start (methods, class members)
        this.argsFrame = null;  // arrows: the enclosing function's frame
        this.name = name;
        this.code = code;  // Uint8Array (may be a view into the parent's bytecode)
        this.captures = captures; // boxes ({ v }) of the variables this closure uses
        this.wrapper = null; // host-callable function for this VM function
    }
}

// An empty array that already holds any kind of value. V8 tracks what an array holds
// (small ints, doubles, anything) and code optimised for "anything" deoptimises on a fresh
// `[]` (small ints) the first time it changes: for the operand stack, once per new VM.
const genericArray = function () {
    const array = [undefined];
    array.length = 0;
    return array;
};

class StackFrame {
    constructor(func, stackBase) {
        this.func = func;
        this.stackBase = stackBase || 0;  // Index where this frame's operands start (for restoration)
        this.ip = 0;                      // Instruction pointer for this frame
        this.locals = [];                 // local slots; a captured variable's slot holds a box
        this.thisValue = undefined;       // `this` inside the frame
        this.args = [];                   // all call arguments, for `arguments`
        this.argumentsObject = null;
        // the enclosing non-arrow frame: where `this`, `arguments`, `super` and
        // `new.target` come from (itself, except for arrows)
        this.argsFrame = this;
        this.newTarget = undefined;
        this.isConstruct = false;         // running as `new f()`: return `this` unless an object is returned
        this.coroutine = null;            // a generator / async function's state
    }
}

class VM {
    constructor(bytecode, options) {

        this.options = options || {
            debug: false,
            debugLog: undefined
        };
        this.debug = !!(this.options.debug && this.options.debugLog);

        // setup virtual functions
        this.functions = [];
        for (const funcCode of bytecode) {
            const code = funcCode instanceof Uint8Array ? funcCode : new Uint8Array(funcCode);
            const func = new VirtualFunction(0, 0, FN_NORMAL, 'main', code, []);
            this.functions.push(func);
        }

        // init ip and stack 
        this.ip = 0;
        this.stack = genericArray();

        // init call stack with main function
        this.callStack = [new StackFrame(this.functions[0])];
        // exception handlers: { callDepth, stackLen, catchIp }, innermost last
        this.handlers = [];
        // tagged template strings arrays, by template site
        this.templates = new Map();
        this.bytecode = this.functions[0].code;
        // decoded string constants, per bytecode buffer (see `readString`)
        this.stringCaches = new Map();
        this.stringBuffer = null;
        this.stringCache = null;

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
        const cached = this.stringCache[at];
        const length = this.readUInt32();
        if (cached !== undefined) {
            this.ip += length;
            return cached;
        }
        const string = this.decodeString(length);
        this.stringCache[at] = string;
        return string;
    }

    decodeString(length) {
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

    push(value) {
        this.stack.push(value);
    }

    pop() {
        return this.stack.pop();
    }

    peek() {
        return this.stack[this.stack.length - 1];
    }

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
                // generators / async functions among the frames being unwound
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
                this.bytecode = this.callStack[handler.callDepth - 1].func.code;
                this.ip = handler.catchIp;
                this.push(e);
                this.debug && this.log("caught", e);
            }
        }
    }

    // A throw is unwinding the frames from index `keep` up: generators among them are
    // finished, and an async function stops the throw, rejecting its promise instead and
    // returning that to its caller. Returns NOTHING if the throw goes on; otherwise the
    // promise when it's for `run()` to return, or undefined to keep running.
    unwindCoroutines(e, keep, entryDepth) {
        let boundary = -1;
        for (let i = this.callStack.length - 1; i >= keep; i--) {
            const co = this.callStack[i].coroutine;
            if (co === null) continue;
            co.state = CO_DONE;
            if (co.async && !co.generator) {
                boundary = i;
                break;
            }
        }
        if (boundary < 0) return NOTHING;
        const frame = this.callStack[boundary];
        const co = frame.coroutine;
        co.reject(e);
        this.callStack.length = boundary;
        while (this.handlers.length &&
            this.handlers[this.handlers.length - 1].callDepth > boundary) {
            this.handlers.pop();
        }
        this.stack.length = frame.stackBase;
        if (boundary < entryDepth) return co.promise;
        this.push(co.promise);
        const caller = this.callStack[boundary - 1];
        this.bytecode = caller.func.code;
        this.ip = caller.ip + 1;
        return undefined;
    }

    execute(entryStackSize) {
        // read once: log arguments (often template strings) are only built when debugging
        const debug = this.debug;
        for (; this.ip < this.bytecode.length; this.ip++) {
            // get current opcode
            const op = this.bytecode[this.ip];

            // switch on opcode and execute operation
            switch (op) {
                // Operation::LoadString(_) => 0x01,
                case 0x01: {
                    this.push(this.readString());
                    debug && this.log("OP_LOAD_STRING", this.peek());
                    break;
                }
                // Operation::LoadFloat64(_) => 0x02,
                case 0x02: {
                    const number = this.readFloat64();
                    this.stack.push(number);
                    debug && this.log("OP_LOAD_FLOAT64", this.peek());
                    break;
                }
                // Operation::Bool(_) => 0x03,
                case 0x03: {
                    this.stack.push(this.readUInt8() === 0x01);
                    debug && this.log("OP_LOAD_BOOL", this.peek());
                    break;
                }
                // Operation::Pop => 0x04,
                case 0x04: {
                    const popped = this.pop();
                    debug && this.log("OP_POP", popped);
                    break;
                }
                // Operation::Null => 0x05,
                case 0x05: {
                    this.push(null);
                    debug && this.log("OP_NULL");
                    break;
                }
                // Operation::Regex(_, _) => 0x06,
                case 0x06: {
                    const exp = this.readString();
                    // this.ip++; // skip null terminator - handled by readString for the next read
                    const flags = this.readString();
                    this.push(new RegExp(exp, flags));
                    debug && this.log(`OP_REGEX exp=${exp} flags=${flags}`);
                    break;
                }
                // Operation::Undefined => 0x07,
                case 0x07: {
                    this.push(undefined);
                    debug && this.log("OP_UNDEFINED");
                    break;
                }
                // Operation::Return => 0x08,
                case 0x08: {
                    const frame = this.callStack.pop();
                    let returnValue = this.pop();
                    if (frame.isConstruct && (returnValue === null ||
                        (typeof returnValue !== 'object' && typeof returnValue !== 'function'))) {
                        if (returnValue !== undefined && frame.func.derived) {
                            throw new TypeError('Derived constructors may only return object or undefined');
                        }
                        returnValue = frame.thisValue;
                        if (returnValue === UNINITIALIZED) throw thisError();
                    }
                    if (frame.coroutine !== null) returnValue = this.finishCoroutine(frame.coroutine, returnValue);

                    // drop handlers left by a `return` from inside `try`
                    while (this.handlers.length &&
                        this.handlers[this.handlers.length - 1].callDepth > this.callStack.length) {
                        this.handlers.pop();
                    }

                    // Clean up stack (remove operands of the returning function)
                    // Locals are in frame.locals, so we don't need to clean them from stack
                    this.stack.length = frame.stackBase;

                    if (this.callStack.length < entryStackSize) {
                        debug && this.log("OP_RETURN (main)", returnValue);
                        return returnValue;
                    }

                    // (NOTHING: a generator finished into a `for-of`'s record)
                    if (returnValue !== NOTHING) this.push(returnValue);

                    // Restore caller state
                    const callerFrame = this.callStack[this.callStack.length - 1];
                    this.bytecode = callerFrame.func.code;
                    this.ip = callerFrame.ip;

                    debug && this.log("OP_RETURN", returnValue);
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
                // Operation::Exp => 0x29,
                case 0x29: {
                    const b = this.pop();
                    const a = this.pop();
                    const result = a ** b;
                    this.push(result);
                    debug && this.log(`OP_EXP ${a} ** ${b} =>`, result);
                    break;
                }
                // Operation::GetLocal(_) => 0x2b,
                case 0x2b: {
                    const localIndex = this.readUInt8();
                    const frame = this.callStack[this.callStack.length - 1];
                    const value = frame.locals[localIndex];
                    this.push(value);
                    debug && this.log(`OP_GET_LOCAL index=${localIndex} value=`, value);
                    break;
                }
                // Operation::SetLocal(_) => 0x2c,
                case 0x2c: {
                    const localIndex = this.readUInt8();
                    const value = this.peek();  // Don't pop, assignment is an expression
                    const frame = this.callStack[this.callStack.length - 1];
                    frame.locals[localIndex] = value;
                    debug && this.log(`OP_SET_LOCAL index=${localIndex} value=`, value);
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
                // Operation::Delete => 0x33,
                case 0x33: {
                    const propName = this.pop();
                    const obj = this.pop();
                    const result = delete obj[propName];
                    this.push(result);
                    debug && this.log(`OP_DELETE delete ${propName} from`, obj, '=>', result);
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
                // Operation::Object(_) => 0x35,
                case 0x35: {
                    const count = this.readUInt32();
                    // pairs are assigned in source order, so later duplicate keys win
                    const base = this.stack.length - count * 2;
                    const obj = {};
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
                // Operation::Dup => 0x36,
                case 0x36: {
                    const value = this.peek();
                    this.push(value);
                    debug && this.log(`OP_DUP`, value);
                    break;
                }
                // Operation::Function => 0x37 (host-callable), 0x4b (internal: only called
                // directly from VM code, so no host wrapper is made)
                case 0x37:
                case 0x4b: {
                    const arity = this.readUInt8();
                    const params = this.readUInt8();
                    const kind = this.readUInt8();
                    const name = this.readString();
                    const currentFrame = this.callStack[this.callStack.length - 1];
                    // captures: count, then (from_local, index): a boxed local slot of this
                    // frame, or one of this closure's own captures
                    const captureCount = this.readUInt8();
                    const captures = new Array(captureCount);
                    for (let i = 0; i < captureCount; i++) {
                        const fromLocal = this.readUInt8();
                        const index = this.readUInt8();
                        captures[i] = fromLocal
                            ? currentFrame.locals[index]
                            : currentFrame.func.captures[index];
                    }
                    const codeLen = this.readUInt32();
                    const code = this.bytecode.subarray(this.ip + 1, this.ip + 1 + codeLen);
                    this.ip += codeLen;

                    const func = new VirtualFunction(arity, params, kind, name, code, captures);
                    if (func.arrow) {
                        func.argsFrame = currentFrame.argsFrame;
                    }
                    this.push(op === 0x37 ? this.createHostWrapper(func) : func);
                    debug && this.log(`OP_FUNCTION name=${name} arity=${arity}`);
                    break;
                }
                // Operation::GetUpvalue => 0x38,
                case 0x38: {
                    const index = this.readUInt8();
                    const value = this.callStack[this.callStack.length - 1].func.captures[index].v;
                    this.push(value);
                    debug && this.log(`OP_GET_UPVALUE index=${index} value=`, value);
                    break;
                }
                // Operation::SetUpvalue => 0x39,
                case 0x39: {
                    const index = this.readUInt8();
                    const value = this.peek(); // Don't pop, assignment is an expression
                    this.callStack[this.callStack.length - 1].func.captures[index].v = value;
                    debug && this.log(`OP_SET_UPVALUE index=${index} value=`, value);
                    break;
                }
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
                    const value = this.callStack[this.callStack.length - 1].argsFrame.thisValue;
                    if (value === UNINITIALIZED) throw thisError();
                    this.push(value);
                    debug && this.log("OP_THIS", value);
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
                    debug && this.log("OP_HOST_GLOBAL", which);
                    break;
                }
                // Operation::Loop(_) => 0x41,
                case 0x41: {
                    const offset = this.readUInt32();
                    this.ip -= offset;
                    debug && this.log(`OP_LOOP -${offset}`);
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
                // Operation::Callee => 0x43,
                case 0x43: {
                    const func = this.callStack[this.callStack.length - 1].func;
                    this.push(func.wrapper || func);
                    debug && this.log("OP_CALLEE");
                    break;
                }
                // Operation::Arguments => 0x44,
                case 0x44: {
                    const frame = this.callStack[this.callStack.length - 1].argsFrame;
                    if (!frame.argumentsObject) {
                        frame.argumentsObject = makeArguments.apply(undefined, frame.args);
                    }
                    this.push(frame.argumentsObject);
                    debug && this.log("OP_ARGUMENTS");
                    break;
                }
                // Operation::New => 0x45,
                case 0x45: {
                    const args = this.popArgs();
                    const func = this.pop();
                    if (typeof func !== 'function') {
                        throw new TypeError(`${func} is not a constructor`);
                    }
                    const target = WRAPPED.get(func);
                    if (target !== undefined && (target.arrow || target.method || target.generator || target.async)) {
                        throw new TypeError(`${target.name || 'anonymous'} is not a constructor`);
                    }
                    if (target !== undefined) {
                        // VM constructor: run it in this loop instead of through its wrapper,
                        // creating `this` the way the host's `new` would (a derived class
                        // gets it from `super(...)`)
                        let obj = UNINITIALIZED;
                        if (!target.derived) {
                            const proto = func.prototype;
                            obj = Object.create(proto !== null &&
                                (typeof proto === 'object' || typeof proto === 'function')
                                ? proto : Object.prototype);
                        }
                        this.callStack[this.callStack.length - 1].ip = this.ip;
                        const frame = this.makeFrame(target, obj, args);
                        frame.isConstruct = true;
                        frame.newTarget = func;
                        this.callStack.push(frame);
                        this.bytecode = target.code;
                        this.ip = -1;
                        debug && this.log("OP_NEW virtual function");
                        break;
                    }
                    this.push(new func(...args));
                    debug && this.log("OP_NEW");
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
                // Operation::Hole => 0x4a,
                case 0x4a: {
                    this.push(HOLE);
                    break;
                }
                // Operation::GetBox(_) => 0x4c,
                case 0x4c: {
                    const slot = this.readUInt8();
                    const value = this.callStack[this.callStack.length - 1].locals[slot].v;
                    this.push(value);
                    debug && this.log(`OP_GET_BOX slot=${slot} value=`, value);
                    break;
                }
                // Operation::SetBox(_) => 0x4d,
                case 0x4d: {
                    const slot = this.readUInt8();
                    const value = this.peek(); // Don't pop, assignment is an expression
                    this.callStack[this.callStack.length - 1].locals[slot].v = value;
                    debug && this.log(`OP_SET_BOX slot=${slot} value=`, value);
                    break;
                }
                // Operation::NewBox(_) => 0x4e,
                case 0x4e: {
                    const slot = this.readUInt8();
                    this.callStack[this.callStack.length - 1].locals[slot] = { v: undefined };
                    debug && this.log(`OP_NEW_BOX slot=${slot}`);
                    break;
                }
                // Operation::BoxLocal(_) => 0x4f,
                case 0x4f: {
                    const slot = this.readUInt8();
                    const locals = this.callStack[this.callStack.length - 1].locals;
                    locals[slot] = { v: locals[slot] };
                    debug && this.log(`OP_BOX_LOCAL slot=${slot}`);
                    break;
                }
                // Operation::RenewBox(_) => 0x50,
                case 0x50: {
                    const slot = this.readUInt8();
                    const locals = this.callStack[this.callStack.length - 1].locals;
                    locals[slot] = { v: locals[slot].v };
                    debug && this.log(`OP_RENEW_BOX slot=${slot}`);
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
                    const rest = arraySlice.call(this.callStack[this.callStack.length - 1].args, start);
                    this.push(rest);
                    debug && this.log("OP_REST_ARGS", rest);
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
                // Operation::RequireObjectCoercible => 0x57,
                case 0x57: {
                    const value = this.peek();
                    if (value === null || value === undefined) {
                        throw new TypeError(`Cannot destructure '${value}' as it is ${value}.`);
                    }
                    debug && this.log("OP_REQUIRE_OBJECT_COERCIBLE");
                    break;
                }
                // Operation::ObjectRest => 0x58,
                case 0x58: {
                    const keys = this.pop();
                    const source = Object(this.pop());
                    const used = new Set(keys.map(k => typeof k === 'symbol' ? k : String(k)));
                    const rest = {};
                    for (const key of Reflect.ownKeys(source)) {
                        if (!used.has(key) && propertyIsEnumerable.call(source, key)) {
                            rest[key] = source[key];
                        }
                    }
                    this.push(rest);
                    debug && this.log("OP_OBJECT_REST", rest);
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
                    const isSetter = this.readUInt8() === 1;
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
                // Operation::ObjectSpread => 0x5d,
                case 0x5d: {
                    const source = this.pop();
                    const target = this.peek();
                    if (source !== null && source !== undefined) {
                        const from = Object(source);
                        for (const key of Reflect.ownKeys(from)) {
                            if (propertyIsEnumerable.call(from, key)) {
                                defineProperty(target, key, {
                                    value: from[key], writable: true, enumerable: true, configurable: true
                                });
                            }
                        }
                    }
                    debug && this.log("OP_OBJECT_SPREAD");
                    break;
                }
                // Operation::SetFunctionName(_) => 0x5e,
                case 0x5e: {
                    const prefix = this.readUInt8();
                    nameFunction(this.peek(), this.stack[this.stack.length - 2], prefix);
                    debug && this.log("OP_SET_FUNCTION_NAME");
                    break;
                }
                // Operation::MakeClass(_) => 0x5f,
                case 0x5f: {
                    const hasSuper = this.readUInt8() === 1;
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
                            if (parentProto !== null && typeof parentProto !== 'object' &&
                                typeof parentProto !== 'function') {
                                throw new TypeError(`Class extends value does not have valid prototype property ${String(parentProto)}`);
                            }
                            Object.setPrototypeOf(cls, parent);
                            Object.setPrototypeOf(proto, parentProto);
                        }
                    }
                    defineProperty(cls, 'prototype', { writable: false });
                    const classFn = WRAPPED.get(cls);
                    classFn.homeObject = proto;
                    classFn.fieldKeys = [];
                    this.push(cls);
                    debug && this.log("OP_MAKE_CLASS", cls.name);
                    break;
                }
                // Operation::DefineMethod(_) => 0x60,
                case 0x60: {
                    const kind = this.readUInt8();
                    const fn = this.pop();
                    const key = this.pop();
                    const target = this.pop();
                    defineProperty(target, key, kind === 0
                        ? { value: fn, writable: true, enumerable: false, configurable: true }
                        : kind === 1
                            ? { get: fn, enumerable: false, configurable: true }
                            : { set: fn, enumerable: false, configurable: true });
                    WRAPPED.get(fn).homeObject = target;
                    debug && this.log(`OP_DEFINE_METHOD ${String(key)}`);
                    break;
                }
                // Operation::ClassField => 0x61,
                case 0x61: {
                    let key = this.pop();
                    if (!(key instanceof PrivateName)) key = toPropertyKey(key);
                    WRAPPED.get(this.pop()).fieldKeys.push(key);
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
                        key.values.set(cls, value);
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
                // Operation::SuperCall => 0x64,
                case 0x64: {
                    const args = this.popArgs();
                    const frame = this.callStack[this.callStack.length - 1].argsFrame;
                    const parent = Object.getPrototypeOf(frame.func.wrapper);
                    const parentFn = WRAPPED.get(parent);
                    if (parentFn !== undefined && parentFn.isClass) {
                        // a VM parent class runs in this loop; `SuperCallFinish` takes its result
                        let obj = UNINITIALIZED;
                        if (!parentFn.derived) {
                            const proto = frame.newTarget.prototype;
                            obj = Object.create(isObject(proto) ? proto : Object.prototype);
                        }
                        this.callStack[this.callStack.length - 1].ip = this.ip;
                        const parentFrame = this.makeFrame(parentFn, obj, args);
                        parentFrame.isConstruct = true;
                        parentFrame.newTarget = frame.newTarget;
                        this.callStack.push(parentFrame);
                        this.bytecode = parentFn.code;
                        this.ip = -1;
                        debug && this.log("OP_SUPER_CALL virtual class");
                        break;
                    }
                    if (!isConstructor(parent)) {
                        throw new TypeError('Super constructor is not a constructor');
                    }
                    this.push(Reflect.construct(parent, args, frame.newTarget));
                    debug && this.log("OP_SUPER_CALL");
                    break;
                }
                // Operation::SuperGet => 0x65,
                case 0x65: {
                    const key = this.pop();
                    const frame = this.callStack[this.callStack.length - 1].argsFrame;
                    if (frame.thisValue === UNINITIALIZED) throw thisError();
                    const home = frame.func.homeObject;
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
                    const frame = this.callStack[this.callStack.length - 1].argsFrame;
                    if (frame.thisValue === UNINITIALIZED) throw thisError();
                    const home = frame.func.homeObject;
                    if (home === null) throw new SyntaxError("'super' keyword unexpected here");
                    if (!Reflect.set(Object.getPrototypeOf(home), key, value, frame.thisValue)) {
                        throw new TypeError(`Cannot assign to read only property '${String(key)}'`);
                    }
                    this.push(value);
                    debug && this.log(`OP_SUPER_SET ${String(key)}`, value);
                    break;
                }
                // Operation::NewTarget => 0x67,
                case 0x67: {
                    this.push(this.callStack[this.callStack.length - 1].argsFrame.newTarget);
                    debug && this.log("OP_NEW_TARGET");
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
                // Operation::LoadBigInt(_) => 0x6d,
                case 0x6d: {
                    this.push(HostBigInt(this.readString()));
                    debug && this.log("OP_LOAD_BIGINT", this.peek());
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
                // Operation::JumpIfNullish => 0x74,
                case 0x74: {
                    const offset = this.readUInt32();
                    const value = this.peek();
                    if (value === null || value === undefined) this.ip += offset;
                    debug && this.log(`OP_JUMP_IF_NULLISH +${offset}`);
                    break;
                }
                // Operation::InitialYield => 0x75,
                case 0x75: {
                    const result = this.opInitialYield();
                    if (this.callStack.length < entryStackSize) return result;
                    this.push(result);
                    const caller = this.callStack[this.callStack.length - 1];
                    this.bytecode = caller.func.code;
                    this.ip = caller.ip;
                    break;
                }
                // Operation::Yield(_) => 0x76,
                case 0x76: {
                    const result = this.opYield();
                    if (this.callStack.length < entryStackSize) return result;
                    if (result !== NOTHING) this.push(result);
                    const caller = this.callStack[this.callStack.length - 1];
                    this.bytecode = caller.func.code;
                    this.ip = caller.ip;
                    break;
                }
                // Operation::Await => 0x77,
                case 0x77: {
                    const result = this.opAwait();
                    if (this.callStack.length < entryStackSize) return result;
                    this.push(result);
                    const caller = this.callStack[this.callStack.length - 1];
                    this.bytecode = caller.func.code;
                    this.ip = caller.ip;
                    break;
                }
                // Operation::GetAsyncIterator => 0x78,
                case 0x78: {
                    this.opGetAsyncIterator();
                    break;
                }
                // Operation::DelegateCall(_, _, _) => 0x79,
                case 0x79: {
                    this.opDelegateCall();
                    break;
                }
                // Operation::DelegateResult(_, _) => 0x7a,
                case 0x7a: {
                    this.opDelegateResult();
                    break;
                }
                // Operation::AsyncIterNext(_) => 0x7b,
                case 0x7b: {
                    this.opAsyncIterNext();
                    break;
                }
                // Operation::IterResult(_, _) => 0x7c,
                case 0x7c: {
                    this.opIterResult();
                    break;
                }
                // Operation::AsyncIterClose(_, _, _) => 0x7d,
                case 0x7d: {
                    this.opAsyncIterClose();
                    break;
                }
                // Operation::IterCloseCheck(_) => 0x7e,
                case 0x7e: {
                    this.opIterCloseCheck();
                    break;
                }
                // Operation::NewBoxTdz(_) => 0x86,
                case 0x86: {
                    this.callStack[this.callStack.length - 1].locals[this.readUInt8()] = { v: TDZ };
                    break;
                }
                // Operation::Tdz(_) => 0x87,
                case 0x87: {
                    this.callStack[this.callStack.length - 1].locals[this.readUInt8()] = TDZ;
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
                    const value = this.callStack[this.callStack.length - 1].locals[this.readUInt8()];
                    this.push(value[this.readUInt32()]);
                    break;
                }
                // Operation::GetSlotProp(_, _) => 0x85,
                case 0x85: {
                    const value = this.callStack[this.callStack.length - 1].locals[this.readUInt8()];
                    this.push(value[this.readString()]);
                    break;
                }
                // Operation::InitInstance => 0x80,
                case 0x80: {
                    const frame = this.callStack[this.callStack.length - 1];
                    this.initInstance(frame.func, frame.thisValue);
                    break;
                }
                // Operation::SuperCallFinish => 0x81,
                case 0x81: {
                    const result = this.pop();
                    const frame = this.callStack[this.callStack.length - 1].argsFrame;
                    if (frame.thisValue !== UNINITIALIZED) {
                        throw new ReferenceError('Super constructor may only be called once');
                    }
                    frame.thisValue = result;
                    this.initInstance(frame.func, result);
                    break;
                }
                // Operation::InitField(_, _) => 0x82,
                case 0x82: {
                    const index = this.readUInt32();
                    const nameIt = this.readUInt8() === 1;
                    const value = this.pop();
                    const frame = this.callStack[this.callStack.length - 1];
                    const key = frame.func.fieldKeys[index];
                    const obj = frame.thisValue;
                    if (key instanceof PrivateName) {
                        if (key.values.has(obj)) throw new TypeError('Cannot initialize a private field twice on the same object');
                        key.values.set(obj, value);
                    } else {
                        if (nameIt) nameFunction(value, key, 0);
                        defineProperty(obj, key, dataProperty(value));
                    }
                    break;
                }
                // Operation::ClassInit => 0x83,
                case 0x83: {
                    const fn = this.pop();
                    const classFn = WRAPPED.get(this.pop());
                    fn.homeObject = classFn.homeObject;
                    fn.fieldKeys = classFn.fieldKeys;
                    classFn.initFn = fn;
                    break;
                }
                // Operation::ToPropertyKey => 0x7f,
                case 0x7f: {
                    this.push(toPropertyKey(this.pop()));
                    break;
                }
                // Operation::GetIterator => 0x71,
                case 0x71: {
                    this.push(getIterator(this.pop()));
                    debug && this.log("OP_GET_ITERATOR");
                    break;
                }
                // Operation::IterNext(_, _) => 0x72,
                case 0x72: {
                    const start = this.ip;
                    const slot = this.readUInt8();
                    const offset = this.readUInt32();
                    const record = this.callStack[this.callStack.length - 1].locals[slot];
                    let value;
                    if (record.pending !== NO_VALUE) {
                        value = record.pending;
                        record.pending = NO_VALUE;
                    } else if (record.pendingResult !== null) {
                        const result = record.pendingResult;
                        record.pendingResult = null;
                        value = this.unwrapResult(record, result);
                    } else if (record.next === genNext && this.resumeForOf(record, start)) {
                        // the generator runs now; this instruction runs again when it yields
                        break;
                    } else {
                        value = iterStep(record);
                    }
                    if (value === ITER_DONE) this.ip += offset;
                    else this.push(value);
                    debug && this.log("OP_ITER_NEXT", value);
                    break;
                }
                // Operation::IterClose(_, _) => 0x73,
                case 0x73: {
                    const slot = this.readUInt8();
                    const quiet = this.readUInt8() === 1;
                    iterClose(this.callStack[this.callStack.length - 1].locals[slot], quiet);
                    debug && this.log("OP_ITER_CLOSE");
                    break;
                }
                // Operation::DefinePrivateMethod(_) => 0x6c,
                case 0x6c: {
                    this.opDefinePrivateMethod();
                    break;
                }
                default: {
                    throw new Error(`Unknown opcode 0x${op.toString(16)} at ${this.ip}`);
                }
            }
        }
        this.printStack();
    }

    // ---- rarely used instructions, kept out of `execute` so it stays small for V8

    opInitialYield() {
        const frame = this.callStack[this.callStack.length - 1];
        const func = frame.func;
        const co = new Coroutine(this, frame, func);
        frame.coroutine = co;
        co.normalIp = this.ip + 1;
        co.state = CO_START;
        if (func.async) co.queue = [];
        const fallback = func.async ? KumoAsyncGeneratorPrototype : KumoGeneratorPrototype;
        let proto;
        if (func.wrapper !== null) {
            proto = func.wrapper.prototype;
        } else {
            if (func.genPrototype === null) func.genPrototype = Object.create(fallback);
            proto = func.genPrototype;
        }
        const gen = Object.create(isObject(proto) ? proto : fallback);
        COROUTINES.set(gen, co);
        this.suspendFrame(co);
        this.debug && this.log("OP_INITIAL_YIELD");
        return gen;
    }

    opYield() {
        const raw = this.readUInt8() === 1;
        const normal = this.readUInt32();
        const thrown = this.readUInt32();
        const returned = this.readUInt32();
        const end = this.ip + 1;
        const value = this.pop();
        const co = this.callStack[this.callStack.length - 1].coroutine;
        co.normalIp = end + normal;
        co.throwIp = end + thrown;
        co.returnIp = end + returned;
        co.state = CO_SUSPENDED;
        this.suspendFrame(co);
        let result;
        if (co.async) {
            co.queue.shift().resolve(iterResult(value, false));
        } else if (co.forOf !== null) {
            const record = co.forOf;
            co.forOf = null;
            if (raw) record.pendingResult = value;
            else record.pending = value;
            result = NOTHING;
        } else {
            result = raw ? value : iterResult(value, false);
        }
        this.debug && this.log("OP_YIELD", value);
        return result;
    }

    opAwait() {
        // (resolving first: a throwing `then` / `constructor` throws right here)
        const promise = resolved(this.pop());
        const co = this.callStack[this.callStack.length - 1].coroutine;
        co.throwIp = this.ip + 1;
        co.normalIp = this.ip + 2;
        co.state = CO_AWAITING;
        this.suspendFrame(co);
        this.awaitPromise(co, promise);
        this.debug && this.log("OP_AWAIT");
        const result = co.generator ? undefined : co.promise;
        return result;
    }

    opGetAsyncIterator() {
        const iterable = this.pop();
        const method = iterable === null || iterable === undefined
            ? undefined : iterable[Symbol.asyncIterator];
        let record;
        if (method === undefined || method === null) {
            record = getIterator(iterable);
            record.sync = true;
        } else {
            if (typeof method !== 'function') throw new TypeError('Symbol.asyncIterator is not a function');
            const iterator = method.call(iterable);
            if (!isObject(iterator)) throw new TypeError('Result of the Symbol.asyncIterator method is not an object');
            record = new IterRecord(iterator, iterator.next, null);
        }
        this.push(record);
        this.debug && this.log("OP_GET_ASYNC_ITERATOR");
    }

    opDelegateCall() {
        const locals = this.callStack[this.callStack.length - 1].locals;
        const record = locals[this.readUInt8()];
        const mode = locals[this.readUInt8()];
        const offset = this.readUInt32();
        const received = this.pop();
        const result = this.delegateCall(record, mode, received);
        if (result === NOTHING) {
            this.push(received);
            this.ip += offset;
        } else {
            this.push(result);
        }
        this.debug && this.log("OP_DELEGATE_CALL", mode);
    }

    opDelegateResult() {
        const modeSlot = this.readUInt8();
        const isAsync = this.readUInt8() === 1;
        const doneOffset = this.readUInt32();
        const returnOffset = this.readUInt32();
        const end = this.ip + 1;
        const result = this.pop();
        if (!isObject(result)) throw new TypeError(`Iterator result ${String(result)} is not an object`);
        if (result.done) {
            this.push(result.value);
            const mode = this.callStack[this.callStack.length - 1].locals[modeSlot];
            this.ip = end + (mode === RESUME_RETURN ? returnOffset : doneOffset) - 1;
        } else {
            this.push(isAsync ? result.value : result);
        }
        this.debug && this.log("OP_DELEGATE_RESULT");
    }

    opAsyncIterNext() {
        const record = this.callStack[this.callStack.length - 1].locals[this.readUInt8()];
        this.push(asyncIterNext(record));
        this.debug && this.log("OP_ASYNC_ITER_NEXT");
    }

    opIterResult() {
        const record = this.callStack[this.callStack.length - 1].locals[this.readUInt8()];
        const offset = this.readUInt32();
        const result = this.pop();
        if (!isObject(result)) {
            record.done = true;
            throw new TypeError(`Iterator result ${String(result)} is not an object`);
        }
        if (result.done) {
            record.done = true;
            this.ip += offset;
        } else {
            this.push(result.value);
        }
        this.debug && this.log("OP_ITER_RESULT");
    }

    opAsyncIterClose() {
        const record = this.callStack[this.callStack.length - 1].locals[this.readUInt8()];
        const quiet = this.readUInt8() === 1;
        const offset = this.readUInt32();
        const result = asyncIterClose(record, quiet);
        if (result === NOTHING) this.ip += offset;
        else this.push(result);
        this.debug && this.log("OP_ASYNC_ITER_CLOSE");
    }

    opIterCloseCheck() {
        const quiet = this.readUInt8() === 1;
        const result = this.pop();
        if (!quiet && !isObject(result)) throw new TypeError('Iterator result is not an object');
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
        if (obj === null || (typeof obj !== 'object' && typeof obj !== 'function')) {
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
        const kind = flags & 3;
        // a method, or an accessor's getter (1) / setter (2)
        if (kind === 2) name.setter = fn;
        else name.method = fn;
        const vf = fn instanceof VirtualFunction ? fn : WRAPPED.get(fn);
        if (flags & 4) {
            // static: only the class itself has it
            vf.homeObject = cls;
            if (name.brand === null) name.brand = new WeakSet();
            name.brand.add(cls);
        } else {
            // instances join the class's brand when they're constructed
            const classFn = WRAPPED.get(cls);
            vf.homeObject = cls.prototype;
            if (classFn.brand === null) classFn.brand = new WeakSet();
            name.brand = classFn.brand;
        }
        this.debug && this.log("OP_DEFINE_PRIVATE_METHOD");
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
        if (typeof func === 'function') {
            const vf = WRAPPED.get(func);
            if (vf !== undefined) targetFunc = vf;
        }

        if (targetFunc instanceof VirtualFunction) {
            if (targetFunc.isClass) {
                throw new TypeError(`Class constructor ${targetFunc.name} cannot be invoked without 'new'`);
            }
            const currentFrame = this.callStack[this.callStack.length - 1];
            currentFrame.ip = this.ip; // save current IP

            // parent is the function's definition environment (closure)
            this.callStack.push(this.makeFrame(targetFunc, vmThis, args));

            this.bytecode = targetFunc.code;
            this.ip = -1;
            this.debug && this.log(`OP_CALL virtual function ${targetFunc.name} arity=${targetFunc.arity} args=${args.length}`);
        } else if (typeof func === 'function') {
            // VM code driving a VM generator resumes it right here, without a nested run
            if (func === genNext || func === genThrow || func === genReturn) {
                const co = COROUTINES.get(nativeThis);
                if (co !== undefined && co.vm === this && !co.async) {
                    this.callStack[this.callStack.length - 1].ip = this.ip;
                    const mode = func === genNext ? RESUME_NEXT : func === genThrow ? RESUME_THROW : RESUME_RETURN;
                    const result = this.generatorResume(co, mode, args[0], true);
                    if (result !== RESUMED_INLINE) this.push(result);
                    return;
                }
            }
            const result = func.apply(nativeThis, args);
            this.push(result);
            this.debug && this.log("OP_CALL native function", result);
        } else {
            throw new TypeError(`${func} is not a function`);
        }
    }

    // A new frame for calling `func`.
    makeFrame(func, thisValue, args) {
        const frame = new StackFrame(func, this.stack.length);
        frame.args = args;
        if (func.arrow) {
            frame.argsFrame = func.argsFrame;
        } else {
            frame.thisValue = thisValue;
        }
        // only parameter slots come from arguments; later slots are the function's own
        const paramCount = Math.min(args.length, func.params);
        for (let i = 0; i < paramCount; i++) {
            frame.locals[i] = args[i];
        }
        if (func.async && !func.generator) {
            const co = new Coroutine(this, frame, func);
            co.promise = new HostPromise((resolve, reject) => {
                co.resolve = resolve;
                co.reject = reject;
            });
            frame.coroutine = co;
        }
        return frame;
    }

    // Take a suspending coroutine's frame (on top) off the call stack, saving its
    // operands and exception handlers.
    suspendFrame(co) {
        const frame = co.frame;
        const depth = this.callStack.length;
        co.stack = this.stack.length > frame.stackBase ? this.stack.slice(frame.stackBase) : EMPTY;
        this.stack.length = frame.stackBase;
        let saved = null;
        while (this.handlers.length && this.handlers[this.handlers.length - 1].callDepth === depth) {
            const h = this.handlers.pop();
            (saved || (saved = [])).push({ rel: h.stackLen - frame.stackBase, catchIp: h.catchIp });
        }
        co.handlers = saved;
        this.callStack.pop();
    }

    // Put a suspended coroutine's frame back on top of the call stack, continuing at `ip`.
    restoreFrame(co, ip) {
        const frame = co.frame;
        frame.stackBase = this.stack.length;
        const saved = co.stack;
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
        this.bytecode = frame.func.code;
        this.ip = ip;
        co.state = CO_RUNNING;
    }

    // Resume a suspended coroutine in a nested `run()` with a sent value, returning what
    // it gives back when it next suspends or finishes.
    resumeNested(co, mode, value) {
        const ip = mode === RESUME_NEXT ? co.normalIp : mode === RESUME_THROW ? co.throwIp : co.returnIp;
        const savedIp = this.ip;
        const savedBytecode = this.bytecode;
        const savedDepth = this.callStack.length;
        const savedStackLen = this.stack.length;
        this.restoreFrame(co, ip);
        this.push(value);
        try {
            return this.run();
        } finally {
            this.ip = savedIp;
            this.bytecode = savedBytecode;
            this.callStack.length = savedDepth;
            this.stack.length = savedStackLen;
            while (this.handlers.length &&
                this.handlers[this.handlers.length - 1].callDepth > savedDepth) {
                this.handlers.pop();
            }
        }
    }

    // A sync generator's next / throw / return. `inline` (VM code calling it) resumes the
    // generator in the running loop, its result arriving when it next suspends.
    generatorResume(co, mode, value, inline) {
        if (co.state === CO_RUNNING) throw new TypeError('Generator is already running');
        if (co.state === CO_START && mode !== RESUME_NEXT) co.state = CO_DONE;
        if (co.state === CO_DONE) {
            if (mode === RESUME_THROW) throw value;
            return iterResult(mode === RESUME_RETURN ? value : undefined, true);
        }
        if (!inline) return this.resumeNested(co, mode, value);
        const ip = mode === RESUME_NEXT ? co.normalIp : mode === RESUME_THROW ? co.throwIp : co.returnIp;
        // the loop steps past the current instruction first
        this.restoreFrame(co, ip - 1);
        this.push(value);
        return RESUMED_INLINE;
    }

    // `for-of` over a VM generator of this VM: resume it in this loop, re-running the
    // `IterNext` at `start` once it yields. False if it can't be resumed here.
    resumeForOf(record, start) {
        const co = COROUTINES.get(record.iterator);
        if (co === undefined || co.vm !== this || co.async ||
            (co.state !== CO_SUSPENDED && co.state !== CO_START)) {
            return false;
        }
        this.callStack[this.callStack.length - 1].ip = start - 1;
        co.forOf = record;
        this.restoreFrame(co, co.normalIp - 1);
        this.push(undefined);
        return true;
    }

    // a `yield*` result object passed through to `for-of`: its value, or ITER_DONE
    unwrapResult(record, result) {
        try {
            if (!isObject(result)) throw new TypeError(`Iterator result ${String(result)} is not an object`);
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

    // A coroutine's frame returned `value`: what its caller (or resumer) gets.
    finishCoroutine(co, value) {
        co.state = CO_DONE;
        if (!co.async) {
            if (co.forOf === null) return iterResult(value, true);
            co.forOf.pending = ITER_DONE;
            co.forOf.done = true;
            co.forOf = null;
            return NOTHING;
        }
        if (!co.generator) {
            co.resolve(value);
            return co.promise;
        }
        co.queue.shift().resolve(iterResult(value, true));
        return undefined;
    }

    // Resume `co` once an awaited promise settles. (Kept out of `execute`: a closure there
    // capturing `this` makes V8 slow down the whole loop.)
    awaitPromise(co, promise) {
        promiseThen.call(promise,
            v => this.resumeAwait(co, RESUME_NEXT, v),
            e => this.resumeAwait(co, RESUME_THROW, e));
    }

    // An awaited value settled.
    resumeAwait(co, mode, value) {
        if (!co.generator) {
            this.resumeNested(co, mode, value);
            return;
        }
        this.runAsyncGen(co, mode, value);
        this.drainAsyncGen(co);
    }

    // Run an async generator until it yields, awaits or finishes; a throw out of it
    // rejects the request it was working on.
    runAsyncGen(co, mode, value) {
        try {
            this.resumeNested(co, mode, value);
        } catch (e) {
            co.state = CO_DONE;
            co.queue.shift().reject(e);
        }
    }

    // Work through an async generator's queued requests while it isn't busy.
    drainAsyncGen(co) {
        const queue = co.queue;
        while (queue.length > 0) {
            const state = co.state;
            if (state === CO_RUNNING || state === CO_AWAITING || state === CO_AWAITING_RETURN) return;
            const request = queue[0];
            if (state === CO_START && request.mode !== RESUME_NEXT) co.state = CO_DONE;
            if (co.state === CO_DONE) {
                if (request.mode === RESUME_NEXT) {
                    queue.shift();
                    request.resolve(iterResult(undefined, true));
                } else if (request.mode === RESUME_THROW) {
                    queue.shift();
                    request.reject(request.value);
                } else {
                    // `return(v)` on a finished generator awaits v
                    co.state = CO_AWAITING_RETURN;
                    const settle = (ok, v) => {
                        co.state = CO_DONE;
                        queue.shift();
                        if (ok) request.resolve(iterResult(v, true));
                        else request.reject(v);
                        this.drainAsyncGen(co);
                    };
                    try {
                        promiseThen.call(resolved(request.value), v => settle(true, v), e => settle(false, e));
                    } catch (e) {
                        settle(false, e);
                    }
                    return;
                }
                continue;
            }
            this.runAsyncGen(co, request.mode, request.value);
        }
    }

    // One step of `yield*`: call the inner iterator's next / throw / return. NOTHING when
    // a return finds no `return` method (the generator then returns the value itself).
    delegateCall(record, mode, received) {
        if (record.array !== null) {
            if (mode === RESUME_NEXT) {
                const result = syncNextResult(record, received);
                return record.sync ? asyncFromSync(result) : result;
            }
            if (mode === RESUME_THROW) throw new TypeError("The iterator does not provide a 'throw' method");
            return NOTHING;
        }
        const iterator = record.iterator;
        let result;
        if (mode === RESUME_NEXT) {
            result = record.next.call(iterator, received);
        } else {
            const method = mode === RESUME_THROW ? iterator.throw : iterator.return;
            if (method === undefined || method === null) {
                if (mode === RESUME_RETURN) return NOTHING;
                iterClose(record, false);
                throw new TypeError("The iterator does not provide a 'throw' method");
            }
            result = method.call(iterator, received);
        }
        if (record.sync) {
            if (!isObject(result)) throw new TypeError(`Iterator result ${String(result)} is not an object`);
            return asyncFromSync(result);
        }
        return result;
    }

    // Set up a new instance of a class: its private methods first, then its fields, whose
    // method runs in this loop and gives back the instance. Pushes the instance.
    initInstance(func, obj) {
        const brand = func.brand;
        if (brand !== null) {
            if (brand.has(obj)) throw new TypeError('Cannot initialize private methods twice on the same object');
            brand.add(obj);
        }
        const init = func.initFn;
        if (init === null) {
            this.push(obj);
            return;
        }
        this.callStack[this.callStack.length - 1].ip = this.ip;
        this.callStack.push(this.makeFrame(init, obj, EMPTY));
        this.bytecode = init.code;
        this.ip = -1;
    }

    // Run a VM function for a host caller, in a nested `run()`. `construct` runs it as
    // `new` (for class constructors, which set up fields first).
    runFromHost(func, thisValue, args, newTarget, construct) {
        const savedIp = this.ip;
        const savedBytecode = this.bytecode;
        const savedDepth = this.callStack.length;
        const savedStackLen = this.stack.length;

        const frame = this.makeFrame(func, thisValue, args);
        frame.newTarget = newTarget;
        frame.isConstruct = construct === true;
        this.callStack.push(frame);
        this.bytecode = func.code;
        this.ip = 0;

        try {
            return this.run();
        } finally {
            // also restores state when the function throws
            this.ip = savedIp;
            this.bytecode = savedBytecode;
            this.callStack.length = savedDepth;
            this.stack.length = savedStackLen;
            while (this.handlers.length &&
                this.handlers[this.handlers.length - 1].callDepth > savedDepth) {
                this.handlers.pop();
            }
        }
    }

    // A host function that runs a VM function, so natives, `call`/`apply`/`bind`, `new`
    // and `instanceof` all work on it. Only made for functions that can leave VM code.
    // Arrows and methods get host arrows / methods: not constructible, no `prototype`.
    createHostWrapper(func) {
        let kind = func.arrow ? FN_ARROW : func.method ? FN_METHOD : FN_NORMAL;
        const coroutine = func.generator || func.async;
        // generator / async functions can't be used with `new` (method wrappers can't)
        if (coroutine && kind === FN_NORMAL) kind = FN_METHOD;
        const makers = WRAPPERS[kind];
        let wrapper;
        if (func.isClass) {
            wrapper = makeClassWrapper(this, func, func.name);
            defineProperty(wrapper, 'length', { value: func.arity });
        } else if (func.arity < makers.length) {
            wrapper = makers[func.arity](this, func, func.name);
        } else {
            wrapper = makers[0](this, func, func.name);
            defineProperty(wrapper, 'length', { value: func.arity });
        }

        if (coroutine) {
            Object.setPrototypeOf(wrapper, func.generator
                ? (func.async ? AsyncGeneratorFunctionPrototype : GeneratorFunctionPrototype)
                : AsyncFunctionPrototype);
            if (func.generator) {
                defineProperty(wrapper, 'prototype', {
                    value: Object.create(func.async ? KumoAsyncGeneratorPrototype : KumoGeneratorPrototype),
                    writable: true,
                });
            }
        }
        WRAPPED.set(wrapper, func);
        func.wrapper = wrapper;
        return wrapper;
    }
}

if (typeof module !== 'undefined' && module.exports) {
    module.exports = { VM };
}