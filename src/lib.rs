use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use swc_common::sync::Lrc;
pub mod disasm;
pub mod embed;
pub mod opcode;

use std::cell::Cell;
use swc_common::{BytePos, SourceFile, SourceMap, Span, Spanned};
use swc_ecma_ast::{
    ArrayLit, ArrayPat, ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget,
    AssignTargetPat, BinExpr, BinaryOp, BlockStmt, CallExpr, Callee, Class, ClassMember, ClassProp,
    CondExpr, Constructor, Decl, DoWhileStmt, Expr, ExprOrSpread, FnDecl, ForHead, ForInStmt, ForOfStmt,
    ForStmt, Function, Ident, IfStmt, LabeledStmt, Lit, MemberExpr, MemberProp, MetaPropKind,
    MethodKind, Module, ModuleItem, NewExpr, ObjectLit, ObjectPat, ObjectPatProp, OptChainBase,
    OptChainExpr,
    ParamOrTsParamProp, Pat, PrivateName, PrivateProp, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, StaticBlock, Stmt,
    SuperProp, SwitchStmt, TaggedTpl, Tpl, TryStmt, UnaryExpr, UnaryOp, UpdateExpr, UpdateOp, YieldExpr,
    VarDecl, VarDeclKind, VarDeclOrExpr, VarDeclarator, WhileStmt,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};
use thiserror::Error;

pub struct Compiler<'a> {
    bytecode: Vec<u8>,
    /// names currently in scope in this function, innermost last
    locals: Vec<Local>,
    /// slots used so far in this function (slots are never reused, so closures stay valid)
    slot_count: usize,
    /// block nesting depth within this function (0 = function body)
    current_scope_depth: usize,
    /// enclosing loops / switches / labeled statements, innermost last
    breakables: Vec<Breakable>,
    /// enclosing protected `try` regions, innermost last
    try_stack: Vec<TryRegion>,
    /// labels waiting to be attached to the next loop or switch
    pending_labels: Vec<String>,
    /// scratch slot for short-lived values (never live across nested evaluation)
    temp_slot: Option<u8>,
    enclosing: Option<&'a Compiler<'a>>,
    /// names used anywhere inside functions nested in this one; locals with these names
    /// are kept in boxes so closures can share them (see `captured_names`)
    captured_names: HashSet<String>,
    /// variables this function captures, in capture-index order: `(from_local, index)`,
    /// where `from_local` means a boxed local slot of the enclosing function and otherwise
    /// one of the enclosing function's own captures
    upvalues: RefCell<Vec<(bool, u8)>>,
    /// compiling an arrow function: `this` and `arguments` come from the enclosing function
    is_arrow: bool,
    /// compiling a generator / async function (`yield` / `await` suspend it)
    coroutine: Coroutine,
    /// where this function's closure is made in the enclosing function's source (0 for
    /// hoisted function declarations: they exist from the start of their scope)
    created_at: BytePos,
    /// `created_at` for the next closure compiled in this function
    next_anchor: Option<BytePos>,
    /// declaring a `switch` body's names (their uses always check the dead zone)
    in_switch: bool,
    /// compiling a hoisted function declaration (its closure exists from the scope's start)
    hoisting_fn: bool,
    /// compiling a class body: where its constructor / initialiser closures are made
    class_anchor: Option<BytePos>,
    /// how emitted opcode bytes are renumbered (identity unless a seed shuffles them);
    /// shared by every nested function so one script's bytecode is internally consistent
    opcode_map: Lrc<opcode::OpcodeMap>,
}

/// Whether a function can suspend: generators (`yield`), async functions (`await`) or both.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Coroutine {
    None,
    Generator,
    Async,
    AsyncGenerator,
}

impl Coroutine {
    fn of(is_generator: bool, is_async: bool) -> Self {
        match (is_generator, is_async) {
            (false, false) => Coroutine::None,
            (true, false) => Coroutine::Generator,
            (false, true) => Coroutine::Async,
            (true, true) => Coroutine::AsyncGenerator,
        }
    }
    fn is_generator(self) -> bool {
        matches!(self, Coroutine::Generator | Coroutine::AsyncGenerator)
    }
    fn is_async(self) -> bool {
        matches!(self, Coroutine::Async | Coroutine::AsyncGenerator)
    }
}

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to load file: {0}")]
    LoadFileError(String),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("too many locals, max number of locals supported is 256")]
    TooManyLocals,
    /// `location` is filled in (as " at file:line:col") once the span is resolved against the source map
    #[error("unsupported {what}{location}")]
    Unsupported {
        what: String,
        span: Span,
        location: String,
    },
    /// an early error the parser doesn't check (e.g. redeclaring a `let`)
    #[error("syntax error: {what}{location}")]
    SyntaxError {
        what: String,
        span: Span,
        location: String,
    },
}

type CompileResult<T = ()> = Result<T, CompileError>;

fn unsupported<T>(what: impl Into<String>, span: Span) -> CompileResult<T> {
    Err(CompileError::Unsupported {
        what: what.into(),
        span,
        location: String::new(),
    })
}

fn format_location(cm: &SourceMap, span: Span) -> String {
    let loc = cm.lookup_char_pos(span.lo);
    format!(" at {}:{}:{}", loc.file.name, loc.line, loc.col.0 + 1)
}

struct Local {
    name: String,
    depth: usize,
    slot: u8,
    /// used by a nested function: the slot holds a box shared with closures
    captured: bool,
    /// `const` (or a named function expression's own name): assigning to it throws
    read_only: bool,
    /// a class's hidden slot for a private name (`#x`), holding the VM's private name object
    private: Option<PrivateKind>,
    /// `let` / `const` / `class`: reading or writing it before its declaration runs throws
    tdz: Option<Tdz>,
}

/// When a lexical binding may be used before it's initialised (its temporal dead zone).
/// Uses in the same function are decided at compile time: before the declaration they
/// always throw, after it they never do. Only closures that can exist before it (hoisted
/// functions, or ones written earlier) check at run time, and only then does the binding
/// start out holding the VM's "uninitialised" value. `switch` case declarations can be
/// jumped over, so every use of them checks.
struct Tdz {
    /// where the declaration ends: it's initialised once this point runs
    decl_end: BytePos,
    /// every use checks at run time (`switch` cases)
    always: bool,
    /// where the binding's box is made (`NewBox`), to turn into `NewBoxTdz` if needed
    box_at: Option<usize>,
    /// a closure checks it at run time, so it must start uninitialised
    needed: Cell<bool>,
}

/// How a use of a variable deals with the temporal dead zone.
#[derive(PartialEq)]
enum TdzUse {
    /// can't be uninitialised here
    Free,
    /// always before the declaration: throws
    Throw,
    /// check at run time
    Check,
}

/// What a private name (`#x`) declares. Known at compile time because private names are
/// always declared in an enclosing class body.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PrivateKind {
    Field = 0,
    Method = 1,
    /// a getter and / or setter
    Accessor = 2,
}

/// How a member expression's key is read and written.
#[derive(Clone, Copy)]
enum Access {
    /// `obj.x` / `obj[x]`: the key is on the stack
    Property,
    /// `obj.#x`: the private name object is on the stack
    Private(PrivateKind),
}

/// Where a variable lives, from the point of view of the function using it.
enum VarRef {
    /// a plain local slot
    Local(u8),
    /// a local slot holding a box (the variable is captured by a nested function)
    Box(u8),
    /// one of the running closure's captured boxes
    Upvalue(u8),
}

#[derive(PartialEq)]
enum BreakableKind {
    Loop,
    Switch,
    /// a labeled non-loop statement: only `break label` targets it
    Labeled,
}

struct Breakable {
    kind: BreakableKind,
    labels: Vec<String>,
    /// forward jumps to patch to the end of the statement
    breaks: Vec<usize>,
    /// forward jumps to patch to the loop's continue point
    continues: Vec<usize>,
    /// `try_stack` length when the statement started
    try_depth: usize,
    /// a `for-of` loop: the try region right above `try_depth` closes its iterator, which
    /// `continue` must leave in place
    iter_region: bool,
}

/// A protected `try` (or `catch`, when there's a `finally`) region. Leaving it early with
/// `break` / `continue` / `return` must drop its handler and run its `finally`.
struct TryRegion {
    finally: Option<BlockStmt>,
    /// a `for-of` body instead: the slot of the iterator to close (`return()`) on early
    /// exit, and whether it's a `for await` (closing awaits)
    close_iterator: Option<(u8, bool)>,
    /// scope at the `try` statement, so the inlined `finally` resolves names correctly
    locals_len: usize,
    scope_depth: usize,
}

/// Undeclared names that resolve to the host environment. Everything else must be declared.
#[derive(Clone, Copy)]
enum HostGlobal {
    Window = 0,
    GlobalThis = 1,
}

impl HostGlobal {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "window" => Some(HostGlobal::Window),
            "globalThis" => Some(HostGlobal::GlobalThis),
            _ => None,
        }
    }
}

/// `HostGlobal` operand flag: push `undefined` instead of throwing when missing (for `typeof`)
const HOST_GLOBAL_TYPEOF: u8 = 0b10;

#[derive(Debug)]
enum Operation {
    Pop,
    Return,
    LoadFloat64(f64),
    #[allow(dead_code)]
    UInt8(u8),
    #[allow(dead_code)]
    UInt16(u16),
    #[allow(dead_code)]
    UInt32(u32),
    UInt64(u64),
    /// "kumo" (WTF-8 bytes, so lone surrogates survive)
    LoadString(Vec<u8>),
    /// true
    Bool(bool),
    /// null
    Null,
    /// `undefined`
    Undefined,
    /// /exp/flags
    Regex(String, String),
    /// x()
    Call,
    /// x.a
    GetProperty,
    /// x.a = b
    SetProperty,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `===`
    EqEqEq,
    /// `!==`
    NotEqEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `<<`
    LShift,
    /// `>>`
    RShift,
    /// `>>>`
    ZeroFillRShift,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `&`
    BitAnd,
    /// `in`
    In,
    /// `instanceof`
    InstanceOf,
    /// `**`
    Exp,
    /// Get local variable at stack offset
    GetLocal(u8),
    /// Set local variable at stack offset
    SetLocal(u8),
    /// `-`
    UnaryMinus,
    /// `+`
    UnaryPlus,
    /// `!`
    LogicalNot,
    /// `~`
    BitwiseNot,
    /// `typeof`
    TypeOf,
    /// `void`
    Void,
    /// `delete`
    Delete,
    /// Create array with n elements
    Array(u32),
    /// Create object with n properties
    Object(u32),
    /// Duplicate the top value on the stack
    Dup,
    /// Create a closure (see `FnInfo`)
    Function(FnInfo),
    /// Get a captured variable of the running closure: capture index
    GetUpvalue(u8),
    /// Set a captured variable of the running closure: capture index
    SetUpvalue(u8),
    /// obj.f(): stack is [obj, f, ...args, argc]; calls f with obj as `this`
    CallMethod,
    /// `this`
    This,
    /// host `window` / `globalThis`: operand is a `HostGlobal` id, optionally | `HOST_GLOBAL_TYPEOF`
    HostGlobal(u8),
    /// Jump forward by offset
    Jump(u32),
    /// Jump forward by offset if top of stack is falsy (does not pop)
    JumpIfFalse(u32),
    /// Jump forward by offset if top of stack is truthy (does not pop)
    JumpIfTrue(u32),
    /// Jump forward by offset if top of stack is not null/undefined (does not pop)
    JumpIfNotNullish(u32),
    /// Jump backward by offset
    Loop(u32),
    /// Duplicate the top two values: [a, b] -> [a, b, a, b]
    Dup2,
    /// The currently running function (for named function expressions)
    Callee,
    /// `arguments`
    Arguments,
    /// `new f(...args)`: stack is [f, ...args, argc]
    New,
    /// `throw value`
    Throw,
    /// Install an exception handler at a forward offset; the thrown value is pushed there
    TryStart(u32),
    /// Remove the innermost exception handler
    TryEnd,
    /// Replace an object with an array of its `for-in` keys
    ForInKeys,
    /// An array literal hole (`[1, , 3]`)
    Hole,
    /// Read a boxed (captured) local
    GetBox(u8),
    /// Write a boxed (captured) local, leaving the value on the stack
    SetBox(u8),
    /// Put a fresh box holding `undefined` in a slot (a captured variable's scope starts)
    NewBox(u8),
    /// Box the value already in a slot (captured parameters)
    BoxLocal(u8),
    /// Replace a slot's box with a fresh one holding the same value (per-iteration `let`)
    RenewBox(u8),
    /// [arr, value] -> [arr] with value appended (a `Hole` just extends the length)
    ArrayAppend,
    /// [arr, iterable] -> [arr] with every iterated value appended (`...x`)
    ArraySpread,
    /// [args array] -> [...args, argc] for a call with spread arguments
    SpreadArgs,
    /// Push an array of the call's arguments from this index on (rest parameter)
    RestArgs(u8),
    /// Replace an iterable with an array of its first n values (`ITER_TAKE_ALL`: every
    /// value), closing the iterator if it isn't done; arrays are used as they are
    IterTake(u32),
    /// Replace an array with its elements from an index on (array pattern rest)
    ArraySliceFrom(u32),
    /// Throw a `TypeError` if the top of the stack is `null` / `undefined` (does not pop)
    RequireObjectCoercible,
    /// [obj, keys] -> [copy of obj's own enumerable properties not in keys]
    ObjectRest,
    /// Join n parts (strings and values, alternating, string first) as a template does
    Template(u32),
    /// Push the strings array for a tagged template: (cooked, raw) per piece, cooked
    /// `None` for an invalid escape. The same array is reused each time this site runs.
    TemplateObject(Vec<(Option<Vec<u8>>, Vec<u8>)>),
    /// [obj, key, value] -> [obj] with an own data property (object literal)
    DefineField,
    /// [obj, key, fn] -> [obj] with fn as the property's getter (0) or setter (1)
    DefineAccessor(u8),
    /// [obj, source] -> [obj] with source's own enumerable properties copied (`...source`)
    ObjectSpread,
    /// [key, fn] (not popped): name fn after a computed key, prefixed by 1 `get ` / 2 `set `
    SetFunctionName(u8),
    /// [superclass if 1, constructor] -> [class]: link prototypes, lock `prototype`
    MakeClass(u8),
    /// [target, key, fn] -> []: a non-enumerable method (0), getter (1) or setter (2)
    DefineMethod(u8),
    /// [class, key] -> []: an instance field's key (in order); `ClassInit` sets up values
    ClassField,
    /// [class, key, init or undefined] -> []: define a static field now
    StaticField,
    /// [class, fn] -> []: run a static block with `this` as the class
    StaticBlock,
    /// [...args, argc] -> [this]: call the parent constructor (`super(...)`)
    SuperCall,
    /// [key] -> [value]: `super[key]`
    SuperGet,
    /// [key, value] -> [value]: `super[key] = value`
    SuperSet,
    /// `new.target`
    NewTarget,
    /// Push a new private name for a class body (a `PrivateKind`)
    NewPrivateName(u8),
    /// [obj, name] -> [value]: `obj.#x` (a private getter is called)
    PrivateGet,
    /// [obj, name, value] -> [value]: `obj.#x = value`. A private setter is called with
    /// the value left under its result: [value, result], so the compiler pops after it.
    PrivateSet,
    /// [name, obj] -> [bool]: `#x in obj`
    PrivateIn,
    /// [class, name, fn] -> []: a private method (0), getter (1) or setter (2), `| 4` static
    DefinePrivateMethod(u8),
    /// a BigInt literal, as its decimal digits
    LoadBigInt(String),
    /// `x + 1` after converting x to a number (a BigInt steps by `1n`)
    Increment,
    /// `x - 1` after converting x to a number (a BigInt steps by `1n`)
    Decrement,
    /// Jump forward by offset if top of stack is null/undefined (does not pop)
    JumpIfNullish(u32),
    /// A generator's start (after its parameters): suspend, giving the caller the
    /// generator object
    InitialYield,
    /// [value] -> suspend, giving the caller `{ value, done: false }` (or the value itself
    /// when raw = 1). Three u32 offsets from the end of the instruction say where to resume
    /// for `next(v)`, `throw(e)` and `return(v)`; the sent value is pushed.
    Yield(u8),
    /// [value] -> suspend until the value settles, then push the result. A rejection
    /// resumes at the instruction after (a `Throw`), a fulfilment after that.
    Await,
    /// [iterable] -> [async iterator record] (`for await`, async `yield*`)
    GetAsyncIterator,
    /// `yield*`: [received] -> [inner result]: call the iterator record's next / throw /
    /// return (by the mode slot) with it. Without a `return` method a return jumps
    /// forward with the received value.
    DelegateCall(u8, u8, u32),
    /// `yield*`: [inner result] -> [result to yield] (async: its value); when done, its
    /// value jumps to the first offset (or the second, for a return)
    DelegateResult(u8, u8),
    /// `for await`: [iterator record slot] -> [promise of the next result]
    AsyncIterNext(u8),
    /// [result] -> [value], or jump forward by the offset when it's done (marking the
    /// record in the slot done)
    IterResult(u8, u32),
    /// `for await` closing: call the record's `return()` and push its result, or jump
    /// forward when there's nothing to await; mode 1 (leaving by a throw) ignores errors
    AsyncIterClose(u8, u8, u32),
    /// [awaited return() result] -> []: must be an object (mode 0)
    IterCloseCheck(u8),
    /// [key] -> [property key]: a string or symbol, converted the way JS does
    ToPropertyKey,
    /// Put a fresh box holding the "uninitialised" value in a slot (see `Tdz`)
    NewBoxTdz(u8),
    /// Put the "uninitialised" value in a slot (a `switch` case's `let`)
    Tdz(u8),
    /// Throw a `ReferenceError` if the top of the stack is the "uninitialised" value
    CheckTdz,
    /// Throw a `ReferenceError`: a variable used before its declaration
    ThrowTdz,
    /// [] -> [slot value[n]] (array destructuring)
    GetSlotIndex(u8, u32),
    /// [] -> [slot value.name] (object destructuring)
    GetSlotProp(u8, Vec<u8>),
    /// [] -> [this]: set up a new instance's private methods and fields (start of a base
    /// class constructor); the fields' method runs in this loop and gives back `this`
    InitInstance,
    /// [parent constructor's result] -> [this]: bind `this` after `super(...)`, then set up
    /// the instance like `InitInstance`
    SuperCallFinish,
    /// [value] -> []: define instance field n (its key is from the class) on `this`; 1 names
    /// an anonymous function value after a computed key
    InitField(u32, u8),
    /// [class, fn] -> []: the class's instance field method
    ClassInit,
    /// [iterable] -> [iterator record] (`for-of`)
    GetIterator,
    /// Step the iterator record in a slot: push its next value, or jump forward by the
    /// offset when it's done
    IterNext(u8, u32),
    /// Close the iterator record in a slot (call its `return()`) unless it's done; mode 1
    /// (leaving by a throw) ignores errors from `return()`
    IterClose(u8, u8),
    /// convert to a number, keeping a BigInt as it is (the old value of `x++`)
    ToNumeric,
}

/// `IterTake` count meaning "take every value".
const ITER_TAKE_ALL: u32 = u32::MAX;

/// A closure to create. An `internal` function is only ever called directly from VM code,
/// so it gets no host wrapper (see `fn_escapes`).
#[derive(Debug)]
struct FnInfo {
    /// the function's `length`: parameters before the first default or rest
    length: u8,
    /// parameter slots filled from the call's arguments
    params: u8,
    kind: FnKind,
    coroutine: Coroutine,
    internal: bool,
    name: String,
    /// `(from_local, index)` pairs, see `Compiler::upvalues`
    captures: Vec<(bool, u8)>,
    code: Vec<u8>,
}

/// Arrows take `this` and `arguments` from where they're created; arrows and methods
/// (object literal methods, getters, setters) can't be used with `new`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FnKind {
    Normal = 0,
    Arrow = 1,
    Method = 2,
    /// class constructors: only callable with `new`
    BaseClass = 3,
    /// a derived class's `this` comes from its `super(...)` call
    DerivedClass = 4,
}

/// A function or arrow function's body.
enum FnBody<'f> {
    Block(&'f [Stmt]),
    /// an arrow function's expression body
    Expr(&'f Expr),
    /// a class field initialiser: returns the value, named after the field if anonymous
    Field(&'f Expr, Option<String>),
    /// a derived class's implicit `constructor(...args) { super(...args); }`
    DefaultDerived,
    /// a class's instance field initialisers, run in order with `this` as the new instance
    InstanceFields(&'f [FieldInit<'f>]),
}

/// An instance field for `FnBody::InstanceFields`: its value (`None` for `x;`), and the
/// name an anonymous function value gets (`None`: from its computed key, at run time).
struct FieldInit<'f> {
    value: Option<&'f Expr>,
    name: Option<String>,
}

#[rustfmt::skip]
opcode::opcodes! {
    LoadString(..)            => 0x01, [Str];
    LoadFloat64(..)           => 0x02, [F64];
    Bool(..)                  => 0x03, [U8];
    Pop                       => 0x04, [];
    Null                      => 0x05, [];
    Regex(..)                 => 0x06, [Str, Str];
    Undefined                 => 0x07, [];
    Return                    => 0x08, [];
    UInt8(..)                 => 0x0b, [U8];
    UInt16(..)                => 0x0c, [U16];
    UInt32(..)                => 0x0d, [U32];
    UInt64(..)                => 0x0e, [U32, U32];
    Call                      => 0x0f, [];
    GetProperty               => 0x10, [];
    SetProperty               => 0x11, [];
    EqEq                      => 0x12, [];
    NotEq                     => 0x13, [];
    EqEqEq                    => 0x14, [];
    NotEqEq                   => 0x15, [];
    Lt                        => 0x16, [];
    LtEq                      => 0x17, [];
    Gt                        => 0x18, [];
    GtEq                      => 0x19, [];
    LShift                    => 0x1a, [];
    RShift                    => 0x1b, [];
    ZeroFillRShift            => 0x1c, [];
    Add                       => 0x1d, [];
    Sub                       => 0x1e, [];
    Mul                       => 0x1f, [];
    Div                       => 0x20, [];
    Mod                       => 0x21, [];
    BitOr                     => 0x22, [];
    BitXor                    => 0x23, [];
    BitAnd                    => 0x24, [];
    In                        => 0x27, [];
    InstanceOf                => 0x28, [];
    Exp                       => 0x29, [];
    GetLocal(..)              => 0x2b, [U8];
    SetLocal(..)              => 0x2c, [U8];
    UnaryMinus                => 0x2d, [];
    UnaryPlus                 => 0x2e, [];
    LogicalNot                => 0x2f, [];
    BitwiseNot                => 0x30, [];
    TypeOf                    => 0x31, [];
    Void                      => 0x32, [];
    Delete                    => 0x33, [];
    Array(..)                 => 0x34, [U32];
    Object(..)                => 0x35, [U32];
    Dup                       => 0x36, [];
    Function(info) if info.internal => 0x4b, [];
    Function(..)              => 0x37, [];
    GetUpvalue(..)            => 0x38, [U8];
    SetUpvalue(..)            => 0x39, [U8];
    Jump(..)                  => 0x3a, [Jump];
    JumpIfFalse(..)           => 0x3b, [Jump];
    JumpIfTrue(..)            => 0x3c, [Jump];
    JumpIfNotNullish(..)      => 0x3d, [Jump];
    CallMethod                => 0x3e, [];
    This                      => 0x3f, [];
    HostGlobal(..)            => 0x40, [U8];
    Loop(..)                  => 0x41, [Back];
    Dup2                      => 0x42, [];
    Callee                    => 0x43, [];
    Arguments                 => 0x44, [];
    New                       => 0x45, [];
    Throw                     => 0x46, [];
    TryStart(..)              => 0x47, [Jump];
    TryEnd                    => 0x48, [];
    ForInKeys                 => 0x49, [];
    Hole                      => 0x4a, [];
    GetBox(..)                => 0x4c, [U8];
    SetBox(..)                => 0x4d, [U8];
    NewBox(..)                => 0x4e, [U8];
    BoxLocal(..)              => 0x4f, [U8];
    RenewBox(..)              => 0x50, [U8];
    ArrayAppend               => 0x51, [];
    ArraySpread               => 0x52, [];
    SpreadArgs                => 0x53, [];
    RestArgs(..)              => 0x54, [U8];
    IterTake(..)              => 0x55, [U32];
    ArraySliceFrom(..)        => 0x56, [U32];
    RequireObjectCoercible    => 0x57, [];
    ObjectRest                => 0x58, [];
    Template(..)              => 0x59, [U32];
    TemplateObject(..)        => 0x5a, [];
    DefineField               => 0x5b, [];
    DefineAccessor(..)        => 0x5c, [U8];
    ObjectSpread              => 0x5d, [];
    SetFunctionName(..)       => 0x5e, [U8];
    MakeClass(..)             => 0x5f, [U8];
    DefineMethod(..)          => 0x60, [U8];
    ClassField                => 0x61, [];
    StaticField               => 0x62, [];
    StaticBlock               => 0x63, [];
    SuperCall                 => 0x64, [];
    SuperGet                  => 0x65, [];
    SuperSet                  => 0x66, [];
    NewTarget                 => 0x67, [];
    NewPrivateName(..)        => 0x68, [U8];
    PrivateGet                => 0x69, [];
    PrivateSet                => 0x6a, [];
    PrivateIn                 => 0x6b, [];
    DefinePrivateMethod(..)   => 0x6c, [U8];
    LoadBigInt(..)            => 0x6d, [Str];
    Increment                 => 0x6e, [];
    Decrement                 => 0x6f, [];
    ToNumeric                 => 0x70, [];
    GetIterator               => 0x71, [];
    IterNext(..)              => 0x72, [U8, Jump];
    IterClose(..)             => 0x73, [U8, U8];
    JumpIfNullish(..)         => 0x74, [Jump];
    InitialYield              => 0x75, [];
    Yield(..)                 => 0x76, [U8, JumpFromEnd, JumpFromEnd, JumpFromEnd];
    Await                     => 0x77, [];
    GetAsyncIterator          => 0x78, [];
    DelegateCall(..)          => 0x79, [U8, U8, Jump];
    DelegateResult(..)        => 0x7a, [U8, U8, JumpFromEnd, JumpFromEnd];
    AsyncIterNext(..)         => 0x7b, [U8];
    IterResult(..)            => 0x7c, [U8, Jump];
    AsyncIterClose(..)        => 0x7d, [U8, U8, Jump];
    IterCloseCheck(..)        => 0x7e, [U8];
    ToPropertyKey             => 0x7f, [];
    InitInstance              => 0x80, [];
    SuperCallFinish           => 0x81, [];
    InitField(..)             => 0x82, [U32, U8];
    ClassInit                 => 0x83, [];
    GetSlotIndex(..)          => 0x84, [U8, U32];
    GetSlotProp(..)           => 0x85, [U8, Str];
    NewBoxTdz(..)             => 0x86, [U8];
    Tdz(..)                   => 0x87, [U8];
    CheckTdz                  => 0x88, [];
    ThrowTdz                  => 0x89, [];
}
fn load_str(s: &str) -> Operation {
    Operation::LoadString(s.as_bytes().to_vec())
}

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Compiler {
            bytecode: Vec::new(),
            locals: Vec::new(),
            slot_count: 0,
            current_scope_depth: 0,
            breakables: Vec::new(),
            try_stack: Vec::new(),
            pending_labels: Vec::new(),
            temp_slot: None,
            enclosing: None,
            captured_names: HashSet::new(),
            upvalues: RefCell::new(Vec::new()),
            is_arrow: false,
            coroutine: Coroutine::None,
            created_at: BytePos(0),
            next_anchor: None,
            in_switch: false,
            hoisting_fn: false,
            class_anchor: None,
            opcode_map: Lrc::new(opcode::OpcodeMap::identity()),
        }
    }

    fn new_enclosing(enclosing: &'a Compiler<'a>) -> Compiler<'a> {
        Compiler {
            enclosing: Some(enclosing),
            // nested functions must use the same numbering as the script they're part of
            opcode_map: enclosing.opcode_map.clone(),
            ..Compiler::new()
        }
    }

    pub fn compile_file(&mut self, path: &Path) -> CompileResult<Vec<u8>> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm
            .load_file(path)
            .map_err(|e| CompileError::LoadFileError(e.to_string()))?;
        self.parse_and_compile(&cm, &fm)
    }

    /// Compile JS source held in memory. `name` is used in error locations.
    pub fn compile_source(&mut self, name: &str, source: &str) -> CompileResult<Vec<u8>> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            swc_common::FileName::Custom(name.to_string()).into(),
            source.to_string(),
        );
        self.parse_and_compile(&cm, &fm)
    }

    /// Set the opcode numbering for this compilation (and its nested functions). Seed 0 is
    /// a shuffle too; use `compile_source` / `compile_file` for the canonical numbering.
    pub fn with_opcode_map(mut self, map: opcode::OpcodeMap) -> Self {
        self.opcode_map = Lrc::new(map);
        self
    }

    /// The `OpcodeMap` this compiler emits under (so a matching VM can be generated).
    pub fn opcode_map(&self) -> &opcode::OpcodeMap {
        &self.opcode_map
    }

    fn parse_and_compile(&mut self, cm: &SourceMap, fm: &SourceFile) -> CompileResult<Vec<u8>> {
        let lexer = Lexer::new(
            Syntax::Es(Default::default()),
            Default::default(),
            StringInput::from(fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);

        // parsed as a module, so input is always strict mode
        let parse_error = |e: swc_ecma_parser::error::Error| {
            CompileError::ParseError(format!(
                "{}{}",
                e.kind().msg(),
                format_location(cm, e.span())
            ))
        };
        let module = parser.parse_module().map_err(parse_error)?;
        if let Some(e) = parser.take_errors().into_iter().next() {
            return Err(parse_error(e));
        }

        self.compile(&module).map_err(|e| match e {
            CompileError::Unsupported { what, span, .. } => CompileError::Unsupported {
                what,
                span,
                location: format_location(cm, span),
            },
            CompileError::SyntaxError { what, span, .. } => CompileError::SyntaxError {
                what,
                span,
                location: format_location(cm, span),
            },
            e => e,
        })
    }

    pub fn compile(&mut self, module: &Module) -> CompileResult<Vec<u8>> {
        let mut stmts = Vec::with_capacity(module.body.len());
        for item in &module.body {
            match item {
                ModuleItem::Stmt(stmt) => stmts.push(stmt),
                ModuleItem::ModuleDecl(decl) => return unsupported("import/export", decl.span()),
            }
        }
        // the module body behaves like a function body
        self.captured_names = captured_names(&[], &FnBody::Block(&[]), &stmts);
        self.compile_body(&stmts, true, true)?;
        self.patch_tdz(0);
        Ok(self.bytecode.clone())
    }

    // ---------------------------------------------------------------------------------
    // statements
    // ---------------------------------------------------------------------------------

    /// Compile a list of statements forming a function body or block.
    /// `leave_last` keeps the last expression statement's value on the stack (module body).
    fn compile_body(
        &mut self,
        stmts: &[&Stmt],
        hoist_vars: bool,
        leave_last: bool,
    ) -> CompileResult {
        self.hoist_declarations(stmts, hoist_vars)?;
        let last = stmts.len().saturating_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            self.compile_stmt(stmt, leave_last && i == last)?;
        }
        Ok(())
    }

    /// Declarations are hoisted before any code runs: `var`s (function bodies only, found
    /// anywhere in the body), then `let`/`const` and function names of this block, then
    /// function values, so code can refer to anything in scope regardless of order.
    fn hoist_declarations(&mut self, stmts: &[&Stmt], hoist_vars: bool) -> CompileResult {
        if hoist_vars {
            let mut names = Vec::new();
            let mut collector = VarCollector { names: &mut names };
            for stmt in stmts {
                stmt.visit_with(&mut collector);
            }
            for name in names {
                self.declare_if_absent(name)?;
            }
        }

        self.check_redeclarations(stmts, hoist_vars)?;
        for stmt in stmts {
            match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => {
                    self.declare_lexical(var_decl, None)?;
                }
                Stmt::Decl(Decl::Fn(fn_decl)) => {
                    self.declare_if_absent(fn_decl.ident.sym.to_string())?;
                }
                Stmt::Decl(Decl::Class(class_decl)) => {
                    self.declare_variable(class_decl.ident.sym.to_string())?;
                    self.mark_tdz(class_decl.class.span.hi);
                }
                _ => {}
            }
        }

        for stmt in stmts {
            if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
                // `stmts` is everything that can see this name, so it's enough to check
                let internal = !fn_escapes(&fn_decl.ident.sym, stmts);
                self.compile_fn_decl(fn_decl, internal)?;
            }
        }
        Ok(())
    }

    /// Early errors for names declared twice in one block: a `let` / `const` / `class` may
    /// share its name with nothing else there (including parameters and `var`s at a
    /// function's top level), and block-level functions can't repeat either.
    fn check_redeclarations(&self, stmts: &[&Stmt], function_top: bool) -> CompileResult {
        let mut seen: Vec<(&str, bool)> = Vec::new(); // (name, lexical)
        for stmt in stmts {
            let names: Vec<(&Ident, bool)> = match stmt {
                Stmt::Decl(Decl::Var(var_decl)) if var_decl.kind != VarDeclKind::Var => var_decl
                    .decls
                    .iter()
                    .flat_map(|d| pat_idents(&d.name))
                    .map(|i| (i, true))
                    .collect(),
                Stmt::Decl(Decl::Class(class_decl)) => vec![(&class_decl.ident, true)],
                Stmt::Decl(Decl::Fn(fn_decl)) => vec![(&fn_decl.ident, false)],
                _ => continue,
            };
            for (ident, lexical) in names {
                let name = &*ident.sym;
                let clash = seen.iter().any(|&(n, other_lexical)| {
                    n == name && (lexical || other_lexical || !function_top)
                }) || (lexical
                    && self
                        .locals
                        .iter()
                        .any(|l| l.name == name && l.depth == self.current_scope_depth));
                if clash {
                    return Err(CompileError::SyntaxError {
                        what: format!("`{name}` has already been declared"),
                        span: ident.span,
                        location: String::new(),
                    });
                }
                seen.push((name, lexical));
            }
        }
        Ok(())
    }

    /// Declare a `let` / `const`'s names. `end` overrides where each is initialised
    /// (a `for-in/of` head: after the iterated expression).
    fn declare_lexical(&mut self, var_decl: &VarDecl, end: Option<BytePos>) -> CompileResult {
        for decl in &var_decl.decls {
            for ident in pat_idents(&decl.name) {
                self.declare_variable(ident.sym.to_string())?;
                if var_decl.kind == VarDeclKind::Const {
                    self.locals.last_mut().unwrap().read_only = true;
                }
                self.mark_tdz(end.unwrap_or(decl.span.hi));
            }
        }
        Ok(())
    }

    /// Give the local just declared a temporal dead zone ending at `decl_end`.
    fn mark_tdz(&mut self, decl_end: BytePos) {
        let always = self.in_switch;
        let local = self.locals.last().unwrap();
        // its `NewBox` (if captured) was the last instruction
        let box_at = local.captured.then(|| self.bytecode.len() - 2);
        let slot = local.slot;
        if always {
            // uninitialised from the start of the switch
            match box_at {
                Some(at) => self.bytecode[at] = Operation::NewBoxTdz(0).get_opcode(),
                None => self.emit_op(Operation::Tdz(slot)),
            }
        }
        self.locals.last_mut().unwrap().tdz = Some(Tdz {
            decl_end,
            always,
            box_at,
            needed: Cell::new(false),
        });
    }

    /// Bindings that a closure checks start uninitialised: turn their `NewBox` into
    /// `NewBoxTdz`. Done as each leaves scope (or at the end of the function).
    fn patch_tdz(&mut self, from: usize) {
        let opcode = Operation::NewBoxTdz(0).get_opcode();
        for local in &self.locals[from..] {
            if let Some(Tdz { box_at: Some(at), needed, always: false, .. }) = &local.tdz {
                if needed.get() {
                    self.bytecode[*at] = opcode;
                }
            }
        }
    }

    /// How a use of `name` at `pos` deals with the temporal dead zone.
    fn tdz_use(&self, name: &str, pos: BytePos) -> TdzUse {
        let check = |tdz: &Tdz, at: BytePos, same_function: bool| {
            if tdz.always {
                TdzUse::Check
            } else if at >= tdz.decl_end {
                TdzUse::Free
            } else if same_function {
                TdzUse::Throw
            } else {
                tdz.needed.set(true);
                TdzUse::Check
            }
        };
        if let Some(local) = self.locals.iter().rev().find(|l| l.name == name) {
            return local.tdz.as_ref().map_or(TdzUse::Free, |t| check(t, pos, true));
        }
        // a closure: what matters is where it (or the closure containing it) was made
        let mut anchor = self.created_at;
        let mut compiler = self.enclosing;
        while let Some(c) = compiler {
            if let Some(local) = c.locals.iter().rev().find(|l| l.name == name) {
                return local.tdz.as_ref().map_or(TdzUse::Free, |t| check(t, anchor, false));
            }
            anchor = c.created_at;
            compiler = c.enclosing;
        }
        TdzUse::Free
    }

    fn compile_stmt(&mut self, stmt: &Stmt, is_last_in_module: bool) -> CompileResult {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                // leave the last statement's value on the stack so the test runner can read it
                if is_last_in_module {
                    self.compile_expr(&expr_stmt.expr)?;
                } else {
                    self.compile_discarded(&expr_stmt.expr)?;
                }
            }
            Stmt::Return(return_stmt) => {
                match &return_stmt.arg {
                    Some(arg) => {
                        self.compile_expr(arg)?;
                        // an async generator's `return x` awaits x
                        if self.coroutine == Coroutine::AsyncGenerator {
                            self.emit_await();
                        }
                    }
                    None => self.emit_op(Operation::Undefined),
                }
                self.emit_unwind_tries(0)?;
                self.emit_op(Operation::Return);
            }
            // compiled when hoisted by `hoist_declarations`
            Stmt::Decl(Decl::Fn(_)) => {}
            Stmt::Decl(Decl::Class(class_decl)) => {
                let ident = &class_decl.ident;
                self.compile_class(Some(ident), &class_decl.class, Some(&ident.sym))?;
                self.emit_init_var(ident)?;
                self.emit_op(Operation::Pop);
            }
            Stmt::Decl(Decl::Var(var_decl)) => self.compile_var_decl(var_decl)?,
            Stmt::Block(block_stmt) => self.compile_block(block_stmt)?,
            Stmt::Empty(_) | Stmt::Debugger(_) => {}
            Stmt::If(if_stmt) => self.compile_if(if_stmt)?,
            Stmt::While(while_stmt) => self.compile_while(while_stmt)?,
            Stmt::DoWhile(do_while) => self.compile_do_while(do_while)?,
            Stmt::For(for_stmt) => self.compile_for(for_stmt)?,
            Stmt::ForIn(for_in) => self.compile_for_in(for_in)?,
            Stmt::ForOf(for_of) => self.compile_for_of(for_of)?,
            Stmt::Labeled(labeled) => self.compile_labeled(labeled)?,
            Stmt::Break(break_stmt) => {
                let label = break_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, false, break_stmt.span)?;
                self.emit_unwind_tries(self.breakables[target].try_depth)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].breaks.push(jump);
            }
            Stmt::Continue(continue_stmt) => {
                let label = continue_stmt.label.as_ref().map(|l| &*l.sym);
                let target = self.find_breakable(label, true, continue_stmt.span)?;
                // a `for-of` keeps its iterator open: its body region is left at the loop
                let loop_ = &self.breakables[target];
                self.emit_unwind_tries(loop_.try_depth + loop_.iter_region as usize)?;
                let jump = self.emit_jump(Operation::Jump(0));
                self.breakables[target].continues.push(jump);
            }
            Stmt::Switch(switch_stmt) => self.compile_switch(switch_stmt)?,
            Stmt::Throw(throw_stmt) => {
                self.compile_expr(&throw_stmt.arg)?;
                self.emit_op(Operation::Throw);
            }
            Stmt::Try(try_stmt) => self.compile_try(try_stmt)?,
            _ => return unsupported(stmt_kind(stmt), stmt.span()),
        }
        Ok(())
    }

    fn compile_block(&mut self, block: &BlockStmt) -> CompileResult {
        let stmts: Vec<&Stmt> = block.stmts.iter().collect();
        self.enter_scope();
        self.compile_body(&stmts, false, false)?;
        self.exit_scope();
        Ok(())
    }

    fn compile_if(&mut self, stmt: &IfStmt) -> CompileResult {
        self.compile_expr(&stmt.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.cons, false)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        if let Some(alt) = &stmt.alt {
            self.compile_stmt(alt, false)?;
        }
        self.patch_jump(end_jump);
        Ok(())
    }

    fn compile_while(&mut self, stmt: &WhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    fn compile_do_while(&mut self, stmt: &DoWhileStmt) -> CompileResult {
        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.compile_expr(&stmt.test)?;
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        Ok(())
    }

    fn compile_for(&mut self, stmt: &ForStmt) -> CompileResult {
        // `let` in the initializer is scoped to the loop
        self.enter_scope();
        // captured `let`/`const` loop variables get a fresh binding for each iteration
        let mut per_iteration = Vec::new();
        match &stmt.init {
            Some(VarDeclOrExpr::VarDecl(var_decl)) => {
                if var_decl.kind != VarDeclKind::Var {
                    let first = self.locals.len();
                    self.declare_lexical(var_decl, None)?;
                    per_iteration = self.locals[first..]
                        .iter()
                        .filter(|l| l.captured)
                        .map(|l| l.slot)
                        .collect();
                }
                self.compile_var_decl(var_decl)?;
                // closures made in the initializer keep the initial binding
                for &slot in &per_iteration {
                    self.emit_op(Operation::RenewBox(slot));
                }
            }
            Some(VarDeclOrExpr::Expr(expr)) => {
                self.compile_expr(expr)?;
                self.emit_op(Operation::Pop);
            }
            None => {}
        }

        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        let exit = match &stmt.test {
            Some(test) => {
                self.compile_expr(test)?;
                let exit = self.emit_jump(Operation::JumpIfFalse(0));
                self.emit_op(Operation::Pop);
                Some(exit)
            }
            None => None,
        };
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        // the next iteration starts with a copy, so closures from this one keep their value
        for &slot in &per_iteration {
            self.emit_op(Operation::RenewBox(slot));
        }
        if let Some(update) = &stmt.update {
            self.compile_discarded(update)?;
        }
        self.emit_loop(start);
        if let Some(exit) = exit {
            self.patch_jump(exit);
            self.emit_op(Operation::Pop);
        }
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    fn compile_for_in(&mut self, stmt: &ForInStmt) -> CompileResult {
        self.enter_scope();

        // keys array and index live in hidden slots, so break/continue leave nothing on the stack
        let keys = self.alloc_slot()?;
        let index = self.alloc_slot()?;
        // the head's names exist (uninitialised) while the object expression runs
        let target = self.loop_head_target(&stmt.left, &stmt.right)?;
        self.compile_expr(&stmt.right)?;
        self.emit_op(Operation::ForInKeys);
        self.emit_op(Operation::SetLocal(keys));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::LoadFloat64(0.0));
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);


        self.push_breakable(BreakableKind::Loop);
        let start = self.bytecode.len();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::GetLocal(keys));
        self.emit_op(load_str("length"));
        self.emit_op(Operation::GetProperty);
        self.emit_op(Operation::Lt);
        let exit = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);

        // target = keys[index]
        self.emit_op(Operation::GetLocal(keys));
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::GetProperty);
        self.bind_loop_head(&stmt.left, target)?;

        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.emit_op(Operation::GetLocal(index));
        self.emit_op(Operation::Increment);
        self.emit_op(Operation::SetLocal(index));
        self.emit_op(Operation::Pop);
        self.emit_loop(start);
        self.patch_jump(exit);
        self.emit_op(Operation::Pop);
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    /// The pattern a `for-in` / `for-of` head assigns each value to, declaring its names.
    fn loop_head_target<'h>(&mut self, head: &'h ForHead, right: &Expr) -> CompileResult<&'h Pat> {
        match head {
            ForHead::VarDecl(var_decl) => {
                if var_decl.kind != VarDeclKind::Var {
                    // the iterated expression still sees them uninitialised
                    self.declare_lexical(var_decl, Some(right.span().hi))?;
                }
                match var_decl.decls.as_slice() {
                    [decl] if decl.init.is_none() => Ok(&decl.name),
                    _ => unsupported("for-in/of declaration", var_decl.span),
                }
            }
            ForHead::Pat(pat) => Ok(pat),
            ForHead::UsingDecl(using) => unsupported("using declaration", using.span),
        }
    }

    /// Store the value on top of the stack into a loop head's target. A `let` / `const`
    /// head gets fresh bindings each iteration.
    fn bind_loop_head(&mut self, head: &ForHead, target: &Pat) -> CompileResult {
        if matches!(head, ForHead::VarDecl(d) if d.kind != VarDeclKind::Var) {
            for ident in pat_idents(target) {
                if let Some(VarRef::Box(slot)) = self.resolve_local(&ident.sym) {
                    self.emit_op(Operation::NewBox(slot));
                }
            }
        }
        self.compile_pattern(target, matches!(head, ForHead::VarDecl(_)))
    }

    /// `for (x of iterable)`. Each iteration's binding and body run in a protected region:
    /// leaving it early (`break`, `return`, a throw) closes the iterator.
    ///
    ///   GetIterator -> it; L: IterNext(it, X) TryStart(H) bind body C: TryEnd Loop L;
    ///   H: IterClose(it, throw) rethrow; X:
    fn compile_for_of(&mut self, stmt: &ForOfStmt) -> CompileResult {
        let is_await = stmt.is_await;
        if is_await && !self.coroutine.is_async() {
            return unsupported("top-level for await", stmt.span);
        }
        self.enter_scope();
        // the iterator lives in a hidden slot, so break/continue leave nothing on the stack
        let iterator = self.alloc_slot()?;
        // the iterable is evaluated with the head's names declared (but not initialised)
        let target = self.loop_head_target(&stmt.left, &stmt.right)?;
        self.compile_expr(&stmt.right)?;
        self.emit_op(if is_await {
            Operation::GetAsyncIterator
        } else {
            Operation::GetIterator
        });
        self.emit_op(Operation::SetLocal(iterator));
        self.emit_op(Operation::Pop);

        self.push_breakable(BreakableKind::Loop);
        self.breakables.last_mut().unwrap().iter_region = true;
        let start = self.bytecode.len();
        let exit = if is_await {
            // await the next result, then take its value
            self.emit_op(Operation::AsyncIterNext(iterator));
            self.emit_await();
            self.emit_jump(Operation::IterResult(iterator, 0))
        } else {
            self.emit_jump(Operation::IterNext(iterator, 0))
        };
        let handler = self.emit_jump(Operation::TryStart(0));
        self.push_try_region(None);
        self.try_stack.last_mut().unwrap().close_iterator = Some((iterator, is_await));
        self.bind_loop_head(&stmt.left, target)?;
        self.compile_stmt(&stmt.body, false)?;
        self.patch_continues();
        self.try_stack.pop();
        self.emit_op(Operation::TryEnd);
        self.emit_loop(start);

        // thrown from the binding or body: close the iterator (ignoring its errors), rethrow
        self.patch_jump(handler);
        let temp = self.temp()?;
        self.emit_op(Operation::SetLocal(temp));
        self.emit_op(Operation::Pop);
        self.emit_iter_close(iterator, is_await, 1);
        self.emit_op(Operation::GetLocal(temp));
        self.emit_op(Operation::Throw);

        self.patch_jump(exit);
        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    /// Close the iterator record in a slot; mode 1 (leaving by a throw) ignores errors.
    /// An async iterator's `return()` result is awaited.
    fn emit_iter_close(&mut self, iterator: u8, is_async: bool, mode: u8) {
        if !is_async {
            self.emit_op(Operation::IterClose(iterator, mode));
            return;
        }
        let skip = self.emit_jump(Operation::AsyncIterClose(iterator, mode, 0));
        self.emit_await();
        self.emit_op(Operation::IterCloseCheck(mode));
        self.patch_jump(skip);
    }

    fn compile_labeled(&mut self, stmt: &LabeledStmt) -> CompileResult {
        self.pending_labels.push(stmt.label.sym.to_string());
        match &*stmt.body {
            // these attach the pending labels themselves
            Stmt::While(_)
            | Stmt::DoWhile(_)
            | Stmt::For(_)
            | Stmt::ForIn(_)
            | Stmt::ForOf(_)
            | Stmt::Switch(_)
            | Stmt::Labeled(_) => self.compile_stmt(&stmt.body, false),
            body => {
                self.push_breakable(BreakableKind::Labeled);
                self.compile_stmt(body, false)?;
                self.pop_breakable();
                Ok(())
            }
        }
    }

    fn compile_switch(&mut self, stmt: &SwitchStmt) -> CompileResult {
        // the whole switch body is one block scope
        self.enter_scope();
        let discriminant = self.alloc_slot()?;
        self.compile_expr(&stmt.discriminant)?;
        self.emit_op(Operation::SetLocal(discriminant));
        self.emit_op(Operation::Pop);

        let all_stmts: Vec<&Stmt> = stmt.cases.iter().flat_map(|c| c.cons.iter()).collect();
        self.in_switch = true;
        let hoisted = self.hoist_declarations(&all_stmts, false);
        self.in_switch = false;
        hoisted?;

        self.push_breakable(BreakableKind::Switch);

        // tests, in order, each jumping to its case body
        let mut case_jumps = Vec::with_capacity(stmt.cases.len());
        for case in &stmt.cases {
            match &case.test {
                Some(test) => {
                    self.emit_op(Operation::GetLocal(discriminant));
                    self.compile_expr(test)?;
                    self.emit_op(Operation::EqEqEq);
                    let next = self.emit_jump(Operation::JumpIfFalse(0));
                    self.emit_op(Operation::Pop);
                    case_jumps.push(Some(self.emit_jump(Operation::Jump(0))));
                    self.patch_jump(next);
                    self.emit_op(Operation::Pop);
                }
                None => case_jumps.push(None),
            }
        }
        // no case matched: go to `default`, or past the switch
        let default_jump = self.emit_jump(Operation::Jump(0));
        let mut has_default = false;

        // bodies, in order, falling through
        for (case, jump) in stmt.cases.iter().zip(case_jumps) {
            match jump {
                Some(jump) => self.patch_jump(jump),
                None => {
                    self.patch_jump(default_jump);
                    has_default = true;
                }
            }
            for stmt in &case.cons {
                self.compile_stmt(stmt, false)?;
            }
        }
        if !has_default {
            self.breakables
                .last_mut()
                .unwrap()
                .breaks
                .push(default_jump);
        }

        self.pop_breakable();
        self.exit_scope();
        Ok(())
    }

    // Layouts (H = catch handler, FT = finally-on-throw, F = finally block):
    //   try/catch:          TryStart(H) B TryEnd Jump END; H: bind C; END:
    //   try/finally:        TryStart(FT) B TryEnd Jump N; FT: save F rethrow; N: F
    //   try/catch/finally:  TryStart(H) B TryEnd Jump N; H: bind TryStart(FT) C TryEnd Jump N;
    //                       FT: save F rethrow; N: F
    fn compile_try(&mut self, stmt: &TryStmt) -> CompileResult {
        let finally = stmt.finalizer.as_ref();

        let handler = self.emit_jump(Operation::TryStart(0));
        self.push_try_region(finally);
        self.compile_block(&stmt.block)?;
        self.try_stack.pop();
        self.emit_op(Operation::TryEnd);
        let mut to_normal_end = vec![self.emit_jump(Operation::Jump(0))];
        self.patch_jump(handler);

        let mut finally_handler = None;
        match &stmt.handler {
            Some(catch) => {
                // the thrown value is on the stack
                self.enter_scope();
                match &catch.param {
                    Some(pat) => {
                        for ident in pat_idents(pat) {
                            self.declare_variable(ident.sym.to_string())?;
                        }
                        self.compile_pattern(pat, true)?;
                    }
                    None => self.emit_op(Operation::Pop),
                }

                if finally.is_some() {
                    finally_handler = Some(self.emit_jump(Operation::TryStart(0)));
                    self.push_try_region(finally);
                }
                self.compile_block(&catch.body)?;
                if finally.is_some() {
                    self.try_stack.pop();
                    self.emit_op(Operation::TryEnd);
                    to_normal_end.push(self.emit_jump(Operation::Jump(0)));
                }
                self.exit_scope();
            }
            None => {}
        }

        if let Some(finally) = finally {
            // thrown out of the try (or catch): run finally, then rethrow
            if let Some(jump) = finally_handler {
                self.patch_jump(jump);
            }
            let saved = self.alloc_slot()?;
            self.emit_op(Operation::SetLocal(saved));
            self.emit_op(Operation::Pop);
            self.compile_block(finally)?;
            self.emit_op(Operation::GetLocal(saved));
            self.emit_op(Operation::Throw);

            for jump in to_normal_end {
                self.patch_jump(jump);
            }
            self.compile_block(finally)?;
        } else {
            for jump in to_normal_end {
                self.patch_jump(jump);
            }
        }
        Ok(())
    }

    fn push_try_region(&mut self, finally: Option<&BlockStmt>) {
        self.try_stack.push(TryRegion {
            finally: finally.cloned(),
            close_iterator: None,
            locals_len: self.locals.len(),
            scope_depth: self.current_scope_depth,
        });
    }

    /// Leave every try region above `depth` (innermost first): drop its handler and run
    /// its `finally` inline. The regions stay on `try_stack`, since only this exit path leaves them.
    fn emit_unwind_tries(&mut self, depth: usize) -> CompileResult {
        let mut i = self.try_stack.len();
        while i > depth {
            i -= 1;
            self.emit_op(Operation::TryEnd);
            if let Some((iterator, is_async)) = self.try_stack[i].close_iterator {
                self.emit_iter_close(iterator, is_async, 0);
            }
            let Some(finally) = self.try_stack[i].finally.clone() else {
                continue;
            };

            // compile the finally as if at the `try` statement: outside region i and
            // everything nested in it
            let locals_len = self.try_stack[i].locals_len;
            let scope_depth = self.try_stack[i].scope_depth;
            let inner_tries = self.try_stack.split_off(i);
            let inner_locals = self.locals.split_off(locals_len);
            let inner_depth = std::mem::replace(&mut self.current_scope_depth, scope_depth);
            let first_inner = self
                .breakables
                .iter()
                .position(|b| b.try_depth > i)
                .unwrap_or(self.breakables.len());
            let inner_breakables = self.breakables.split_off(first_inner);

            let result = self.compile_block(&finally);

            self.breakables.extend(inner_breakables);
            self.current_scope_depth = inner_depth;
            self.locals.extend(inner_locals);
            self.try_stack.extend(inner_tries);
            result?;
        }
        Ok(())
    }

    fn push_breakable(&mut self, kind: BreakableKind) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.breakables.push(Breakable {
            kind,
            labels,
            breaks: Vec::new(),
            continues: Vec::new(),
            try_depth: self.try_stack.len(),
            iter_region: false,
        });
    }

    fn patch_continues(&mut self) {
        let continues = std::mem::take(&mut self.breakables.last_mut().unwrap().continues);
        for jump in continues {
            self.patch_jump(jump);
        }
    }

    fn pop_breakable(&mut self) {
        let breakable = self.breakables.pop().unwrap();
        for jump in breakable.breaks {
            self.patch_jump(jump);
        }
    }

    fn find_breakable(
        &self,
        label: Option<&str>,
        is_continue: bool,
        span: Span,
    ) -> CompileResult<usize> {
        let found = self.breakables.iter().rposition(|b| match label {
            Some(label) => b.labels.iter().any(|l| l == label),
            None if is_continue => b.kind == BreakableKind::Loop,
            None => b.kind != BreakableKind::Labeled,
        });
        match found {
            Some(i) if is_continue && self.breakables[i].kind != BreakableKind::Loop => {
                unsupported("continue to a non-loop label", span)
            }
            Some(i) => Ok(i),
            None if is_continue => unsupported("continue outside a loop", span),
            None => unsupported("break outside a loop or switch", span),
        }
    }

    // ---------------------------------------------------------------------------------
    // scopes and variables
    // ---------------------------------------------------------------------------------

    fn enter_scope(&mut self) {
        self.current_scope_depth += 1;
    }

    /// Leave a block: its names go out of scope (their slots are not reused).
    fn exit_scope(&mut self) {
        self.current_scope_depth -= 1;
        let keep = self
            .locals
            .iter()
            .rposition(|l| l.depth <= self.current_scope_depth)
            .map_or(0, |i| i + 1);
        self.patch_tdz(keep);
        while self
            .locals
            .last()
            .is_some_and(|l| l.depth > self.current_scope_depth)
        {
            self.locals.pop();
        }
    }

    /// A fresh slot with no name, for compiler-internal values.
    fn alloc_slot(&mut self) -> CompileResult<u8> {
        if self.slot_count >= 256 {
            return Err(CompileError::TooManyLocals);
        }
        let slot = self.slot_count as u8;
        self.slot_count += 1;
        Ok(slot)
    }

    /// The function's scratch slot, for values held only between two adjacent instructions.
    fn temp(&mut self) -> CompileResult<u8> {
        match self.temp_slot {
            Some(slot) => Ok(slot),
            None => {
                let slot = self.alloc_slot()?;
                self.temp_slot = Some(slot);
                Ok(slot)
            }
        }
    }

    /// Declare a name in the current block and return its new slot. Emitted where the
    /// scope starts: a captured name gets a fresh box each time this point runs.
    fn declare_variable(&mut self, name: String) -> CompileResult<u8> {
        let slot = self.alloc_slot()?;
        if self.add_local(name, slot) {
            self.emit_op(Operation::NewBox(slot));
        }
        Ok(slot)
    }

    /// Add a name in the current block; returns whether it's captured (boxed).
    fn add_local(&mut self, name: String, slot: u8) -> bool {
        let captured = self.captured_names.contains(&name);
        self.locals.push(Local {
            name,
            depth: self.current_scope_depth,
            slot,
            captured,
            read_only: false,
            private: None,
            tdz: None,
        });
        captured
    }

    fn local_by_slot(&self, slot: u8) -> &Local {
        self.locals
            .iter()
            .rev()
            .find(|l| l.slot == slot)
            .expect("slot belongs to a declared local")
    }

    /// Push a declared local's value.
    fn emit_load_slot(&mut self, slot: u8) {
        let op = if self.local_by_slot(slot).captured {
            Operation::GetBox(slot)
        } else {
            Operation::GetLocal(slot)
        };
        self.emit_op(op);
    }

    /// Store the top of the stack into a declared local's slot, leaving it on the stack.
    fn emit_store_slot(&mut self, slot: u8) {
        let op = if self.local_by_slot(slot).captured {
            Operation::SetBox(slot)
        } else {
            Operation::SetLocal(slot)
        };
        self.emit_op(op);
    }

    /// Declare a name unless the current block already has it (`var` redeclaring a param, etc).
    fn declare_if_absent(&mut self, name: String) -> CompileResult<u8> {
        let existing = self
            .locals
            .iter()
            .rev()
            .take_while(|l| l.depth == self.current_scope_depth)
            .find(|l| l.name == name);
        match existing {
            Some(local) => Ok(local.slot),
            None => self.declare_variable(name),
        }
    }

    fn resolve_local(&self, name: &str) -> Option<VarRef> {
        if let Some(local) = self.locals.iter().rev().find(|l| l.name == name) {
            return Some(if local.captured {
                VarRef::Box(local.slot)
            } else {
                VarRef::Local(local.slot)
            });
        }
        self.resolve_upvalue(name).map(VarRef::Upvalue)
    }

    /// Find `name` in an enclosing function and return this function's capture index for it,
    /// adding captures along the way (clox-style).
    fn resolve_upvalue(&self, name: &str) -> Option<u8> {
        let enclosing = self.enclosing?;
        let capture = match enclosing.locals.iter().rev().find(|l| l.name == name) {
            Some(local) => {
                debug_assert!(local.captured, "`{name}` is used by a nested function");
                (true, local.slot)
            }
            None => (false, enclosing.resolve_upvalue(name)?),
        };
        let mut upvalues = self.upvalues.borrow_mut();
        if let Some(index) = upvalues.iter().position(|u| *u == capture) {
            return Some(index as u8);
        }
        upvalues.push(capture);
        Some((upvalues.len() - 1) as u8)
    }

    /// Push the value of a variable. Undeclared names are an error, except the host's
    /// `window` / `globalThis`.
    fn emit_get_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        let tdz = self.tdz_use(name, ident.span.lo);
        if tdz == TdzUse::Throw {
            self.emit_op(Operation::ThrowTdz);
            return Ok(());
        }
        match self.resolve_local(name) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::GetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::GetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::GetUpvalue(index)),
            None => match (HostGlobal::from_name(name), name) {
                (Some(host), _) => self.emit_op(Operation::HostGlobal(host as u8)),
                // read-only global constants compile to plain values
                (None, "undefined") => self.emit_op(Operation::Undefined),
                (None, "NaN") => self.emit_op(Operation::LoadFloat64(f64::NAN)),
                (None, "Infinity") => self.emit_op(Operation::LoadFloat64(f64::INFINITY)),
                (None, "arguments") if self.has_arguments() => self.emit_op(Operation::Arguments),
                (None, _) => {
                    return unsupported(format!("undeclared variable `{name}`"), ident.span)
                }
            },
        }
        if tdz == TdzUse::Check {
            self.emit_op(Operation::CheckTdz);
        }
        Ok(())
    }

    /// Whether `arguments` exists here: inside a non-arrow function, or an arrow within one.
    fn has_arguments(&self) -> bool {
        match self.enclosing {
            None => false,
            Some(enclosing) => !self.is_arrow || enclosing.has_arguments(),
        }
    }

    /// Whether `name` resolves to a read-only binding (`const`, a function expression's name).
    fn is_read_only(&self, name: &str) -> bool {
        match self.locals.iter().rev().find(|l| l.name == name) {
            Some(local) => local.read_only,
            None => self.enclosing.is_some_and(|e| e.is_read_only(name)),
        }
    }

    /// Store the top of the stack into a declared variable, leaving it on the stack.
    /// Assigning to a `const` throws a `TypeError` when it runs (after the value is computed).
    fn emit_set_var(&mut self, ident: &Ident) -> CompileResult {
        // writing before the declaration throws too (after the value is computed)
        match self.tdz_use(&ident.sym, ident.span.lo) {
            TdzUse::Throw => {
                self.emit_op(Operation::ThrowTdz);
                return Ok(());
            }
            TdzUse::Check => {
                self.emit_get_var(ident)?;
                self.emit_op(Operation::Pop);
            }
            TdzUse::Free => {}
        }
        if self.is_read_only(&ident.sym) {
            self.emit_op(Operation::Pop);
            self.emit_op(Operation::HostGlobal(HostGlobal::GlobalThis as u8));
            self.emit_op(load_str("TypeError"));
            self.emit_op(Operation::GetProperty);
            self.emit_op(load_str("Assignment to constant variable."));
            self.emit_op(Operation::UInt64(1));
            self.emit_op(Operation::New);
            self.emit_op(Operation::Throw);
            return Ok(());
        }
        self.emit_init_var(ident)
    }

    /// Like `emit_set_var`, but also for initialising a `const` in its declaration.
    fn emit_init_var(&mut self, ident: &Ident) -> CompileResult {
        let name = &*ident.sym;
        match self.resolve_local(name) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::SetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::SetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::SetUpvalue(index)),
            None if HostGlobal::from_name(name).is_some() => {
                return unsupported(format!("assignment to `{name}`"), ident.span)
            }
            None => {
                return unsupported(
                    format!("assignment to undeclared variable `{name}`"),
                    ident.span,
                )
            }
        }
        Ok(())
    }

    fn compile_var_decl(&mut self, var_decl: &VarDecl) -> CompileResult {
        for decl in var_decl.decls.iter() {
            self.compile_var_declarator(var_decl.kind, decl)?;
        }
        Ok(())
    }

    /// The name was already declared when hoisted; this only assigns it.
    fn compile_var_declarator(
        &mut self,
        kind: VarDeclKind,
        var_declarator: &VarDeclarator,
    ) -> CompileResult {
        match (&var_declarator.init, &var_declarator.name) {
            (Some(init), Pat::Ident(ident)) => self.compile_expr_named(init, &ident.id.sym)?,
            (Some(init), _) => self.compile_expr(init)?,
            // `var x;` leaves an existing value alone
            (None, _) if kind == VarDeclKind::Var => return Ok(()),
            (None, _) => self.emit_op(Operation::Undefined),
        }
        self.compile_pattern(&var_declarator.name, true)
    }

    // ---------------------------------------------------------------------------------
    // functions
    // ---------------------------------------------------------------------------------

    fn compile_fn_decl(&mut self, fn_decl: &FnDecl, internal: bool) -> CompileResult {
        self.hoisting_fn = true;
        let compiled = self.compile_function(Some(&fn_decl.ident), &fn_decl.function, false, internal);
        self.hoisting_fn = false;
        compiled?;
        self.emit_set_var(&fn_decl.ident)?;
        self.emit_op(Operation::Pop);
        Ok(())
    }

    /// Push a new function. `self_binding` makes `name` refer to the function inside its
    /// own body (named function expressions). `internal` functions get no host wrapper.
    fn compile_function(
        &mut self,
        name: Option<&Ident>,
        function: &Function,
        self_binding: bool,
        internal: bool,
    ) -> CompileResult {
        let self_name = name.filter(|_| self_binding);
        let name = name.map(|n| &*n.sym);
        self.compile_fn_value(name, self_name, function, internal, FnKind::Normal)
    }

    /// Push a function made from a `Function` node (declarations, expressions, methods).
    fn compile_fn_value(
        &mut self,
        name: Option<&str>,
        self_name: Option<&Ident>,
        function: &Function,
        internal: bool,
        kind: FnKind,
    ) -> CompileResult {
        let params: Vec<&Pat> = function.params.iter().map(|p| &p.pat).collect();
        let body = match &function.body {
            Some(body) => FnBody::Block(&body.stmts),
            None => FnBody::Block(&[]),
        };
        let coroutine = Coroutine::of(function.is_generator, function.is_async);
        // a function value exists from here (declarations are hoisted: they set no anchor)
        if !self.hoisting_fn {
            self.next_anchor = Some(function.span.lo);
        }
        self.compile_closure(name, self_name, &params, &body, internal, kind, coroutine)
    }

    fn compile_arrow(
        &mut self,
        arrow: &ArrowExpr,
        name: Option<&str>,
        internal: bool,
    ) -> CompileResult {
        let params: Vec<&Pat> = arrow.params.iter().collect();
        let body = match &*arrow.body {
            ArrowFunctionBody::FunctionBody(block) => FnBody::Block(&block.stmts),
            ArrowFunctionBody::Expr(expr) => FnBody::Expr(expr),
        };
        let coroutine = Coroutine::of(arrow.is_generator, arrow.is_async);
        self.next_anchor = Some(arrow.span.lo);
        self.compile_closure(name, None, &params, &body, internal, FnKind::Arrow, coroutine)
    }

    /// Compile a function body in its own compiler and push the closure.
    fn compile_closure(
        &mut self,
        name: Option<&str>,
        self_name: Option<&Ident>,
        params: &[&Pat],
        body: &FnBody,
        internal: bool,
        kind: FnKind,
        coroutine: Coroutine,
    ) -> CompileResult {
        let created_at = self
            .next_anchor
            .take()
            .or(self.class_anchor)
            .unwrap_or(BytePos(0));
        let mut c = Compiler::new_enclosing(self);
        c.is_arrow = kind == FnKind::Arrow;
        c.coroutine = coroutine;
        c.created_at = created_at;
        let body_stmts: Vec<&Stmt> = match body {
            FnBody::Block(stmts) => stmts.iter().collect(),
            _ => Vec::new(),
        };
        c.captured_names = captured_names(params, body, &body_stmts);
        // a base class constructor first sets up the instance's private methods and fields
        // (before its parameters); a derived one does it when `super(...)` returns
        if kind == FnKind::BaseClass {
            c.emit_op(Operation::InitInstance);
            c.emit_op(Operation::Pop);
        }

        // parameters are the first slots; the VM fills them from the call's arguments. A
        // pattern parameter gets a hidden slot that is destructured once all names exist.
        let mut length = None;
        let mut inits = Vec::new();
        let mut rest = None;
        for (i, pat) in params.iter().enumerate() {
            let (target, default) = match pat {
                Pat::Assign(assign) => (&*assign.left, Some(&*assign.right)),
                Pat::Rest(rest_pat) => {
                    rest = Some((&*rest_pat.arg, i as u8));
                    length.get_or_insert(i);
                    break; // the parser only allows it last
                }
                pat => (*pat, None),
            };
            if default.is_some() {
                length.get_or_insert(i);
            }
            let slot = c.alloc_slot()?;
            match target {
                Pat::Ident(ident) => {
                    if c.add_local(ident.id.sym.to_string(), slot) {
                        c.emit_op(Operation::BoxLocal(slot));
                    }
                    if let Some(default) = default {
                        inits.push((slot, Some(default), None, Some(&*ident.id.sym)));
                    }
                }
                pat => inits.push((slot, default, Some(pat), None)),
            }
        }
        let param_count = c.slot_count as u8;
        // names bound by patterns (and the rest parameter)
        let pattern_params = inits.iter().filter_map(|(_, _, pat, _)| *pat);
        for pat in pattern_params.chain(rest.map(|(pat, _)| pat)) {
            for ident in pat_idents(pat) {
                c.declare_variable(ident.sym.to_string())?;
            }
        }
        // defaults and patterns run in parameter order; a default is used when its
        // argument is missing or `undefined`
        for (slot, default, pat, name) in inits {
            match pat {
                None => c.emit_load_slot(slot),
                Some(_) => c.emit_op(Operation::GetLocal(slot)),
            }
            if let Some(default) = default {
                c.emit_default(default, name)?;
            }
            match pat {
                None => {
                    c.emit_store_slot(slot);
                    c.emit_op(Operation::Pop);
                }
                Some(pat) => c.compile_pattern(pat, true)?,
            }
        }
        if let Some((pat, start)) = rest {
            c.emit_op(Operation::RestArgs(start));
            c.compile_pattern(pat, true)?;
        }

        if let Some(name) = self_name {
            // a parameter with the same name wins
            if !c.locals.iter().any(|l| l.name == *name.sym) {
                let slot = c.declare_variable(name.sym.to_string())?;
                c.locals.last_mut().unwrap().read_only = true;
                c.emit_op(Operation::Callee);
                c.emit_store_slot(slot);
                c.emit_op(Operation::Pop);
            }
        }

        // a generator stops here (parameters are set up when it's called) and hands back
        // its generator object; the first `next()` resumes it (its value is unused)
        if coroutine.is_generator() {
            c.emit_op(Operation::InitialYield);
            c.emit_op(Operation::Pop);
        }

        match body {
            FnBody::Block(_) => {
                c.compile_body(&body_stmts, true, false)?;
                // implicit `return undefined`
                c.emit_op(Operation::Undefined);
            }
            FnBody::Expr(expr) => {
                c.compile_expr(expr)?;
                if coroutine == Coroutine::AsyncGenerator {
                    c.emit_await();
                }
            }
            FnBody::Field(expr, Some(name)) => c.compile_expr_named(expr, name)?,
            FnBody::Field(expr, None) => c.compile_expr(expr)?,
            FnBody::DefaultDerived => {
                c.emit_op(Operation::RestArgs(0));
                c.emit_op(Operation::SpreadArgs);
                c.emit_op(Operation::SuperCall);
                c.emit_op(Operation::SuperCallFinish);
                c.emit_op(Operation::Pop);
                c.emit_op(Operation::Undefined);
            }
            FnBody::InstanceFields(fields) => {
                for (i, field) in fields.iter().enumerate() {
                    match (field.value, &field.name) {
                        (Some(value), Some(name)) => c.compile_expr_named(value, name)?,
                        (Some(value), None) => c.compile_expr(value)?,
                        (None, _) => c.emit_op(Operation::Undefined),
                    }
                    let name_it = field.name.is_none() && field.value.is_some_and(is_anonymous_fn);
                    c.emit_op(Operation::InitField(i as u32, name_it as u8));
                }
                // the instance is what `super(...)` / the constructor's set-up gives back
                c.emit_op(Operation::This);
            }
        }
        c.emit_op(Operation::Return);
        c.patch_tdz(0);

        let captures = c.upvalues.into_inner();
        // an internal function's name and length can't be observed, so leave the name out
        let name = match name {
            Some(n) if !internal => n.to_string(),
            _ => String::new(),
        };
        self.emit_op(Operation::Function(FnInfo {
            length: length.unwrap_or(params.len()) as u8,
            params: param_count,
            kind,
            coroutine,
            internal,
            name,
            captures,
            code: c.bytecode,
        }));
        Ok(())
    }

    fn compile_call(&mut self, expr: &CallExpr) -> CompileResult {
        let callee = match &expr.callee {
            Callee::Expr(callee_expr) => callee_expr,
            Callee::Super(_) => {
                self.compile_args(&expr.args)?;
                self.emit_op(Operation::SuperCall);
                self.emit_op(Operation::SuperCallFinish);
                return Ok(());
            }
            Callee::Import(i) => return unsupported("dynamic import", i.span),
        };

        // `obj.f()` (and `(obj.f)()`) call f with obj as `this`
        let mut unwrapped = &**callee;
        while let Expr::Paren(paren) = unwrapped {
            unwrapped = &paren.expr;
        }
        let is_method = if let Expr::Fn(fn_expr) = unwrapped {
            // IIFE `(function () {...})()`: internal unless its own name escapes in its body
            let internal = match &fn_expr.ident {
                Some(ident) => !fn_escapes_in_function(&ident.sym, &fn_expr.function),
                None => true,
            };
            self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, internal)?;
            false
        } else if let Expr::Arrow(arrow) = unwrapped {
            // `(() => {...})()` can't refer to itself, so it never escapes
            self.compile_arrow(arrow, None, true)?;
            false
        } else if let Expr::SuperProp(super_prop) = unwrapped {
            // `super.m()` calls the parent's m with this `this`: [this, m]
            self.emit_op(Operation::This);
            self.compile_super_prop(&super_prop.prop)?;
            self.emit_op(Operation::SuperGet);
            true
        } else if let Expr::OptChain(chain @ OptChainExpr { base, .. }) = unwrapped {
            // `(a?.b)()` keeps `this`: [obj, f], or [undefined, undefined] if short-circuited
            if !matches!(&**base, OptChainBase::Member(_)) {
                self.compile_expr(unwrapped)?;
                false
            } else {
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, true)?;
                if !nullish.is_empty() {
                    let end = self.emit_jump(Operation::Jump(0));
                    for jump in nullish {
                        self.patch_jump(jump);
                    }
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Undefined);
                    self.emit_op(Operation::Undefined);
                    self.patch_jump(end);
                }
                true
            }
        } else if let Expr::Member(member_expr) = unwrapped {
            // stack: [obj, f]
            self.compile_expr(&member_expr.obj)?;
            self.emit_op(Operation::Dup);
            let access = self.compile_member_prop(&member_expr.prop)?;
            self.emit_member_get(access);
            true
        } else {
            self.compile_expr(callee)?;
            false
        };

        self.compile_args(&expr.args)?;
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
        Ok(())
    }

    /// Push each argument, then the argument count.
    fn compile_args(&mut self, args: &[ExprOrSpread]) -> CompileResult {
        if args.iter().any(|a| a.spread.is_some()) {
            let elems: Vec<Option<ExprOrSpread>> = args.iter().cloned().map(Some).collect();
            self.compile_spread_array(&elems)?;
            self.emit_op(Operation::SpreadArgs);
            return Ok(());
        }
        for arg in args {
            self.compile_expr(&arg.expr)?;
        }
        self.emit_op(Operation::UInt64(args.len() as u64));
        Ok(())
    }

    /// Build an array element by element, for literals and argument lists containing `...x`.
    fn compile_spread_array(&mut self, elems: &[Option<ExprOrSpread>]) -> CompileResult {
        self.emit_op(Operation::Array(0));
        for elem in elems {
            match elem {
                Some(ExprOrSpread {
                    spread: Some(_),
                    expr,
                }) => {
                    self.compile_expr(expr)?;
                    self.emit_op(Operation::ArraySpread);
                }
                Some(ExprOrSpread { spread: None, expr }) => {
                    self.compile_expr(expr)?;
                    self.emit_op(Operation::ArrayAppend);
                }
                None => {
                    self.emit_op(Operation::Hole);
                    self.emit_op(Operation::ArrayAppend);
                }
            }
        }
        Ok(())
    }

    fn compile_new(&mut self, expr: &NewExpr) -> CompileResult {
        self.compile_expr(&expr.callee)?;
        self.compile_args(expr.args.as_deref().unwrap_or_default())?;
        self.emit_op(Operation::New);
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // expressions
    // ---------------------------------------------------------------------------------

    /// Push the property key of a member expression (or its private name), and return
    /// how to read / write it.
    fn compile_member_prop(&mut self, prop: &MemberProp) -> CompileResult<Access> {
        match prop {
            MemberProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            MemberProp::Computed(computed) => self.compile_expr(&computed.expr)?,
            MemberProp::PrivateName(name) => {
                return Ok(Access::Private(self.emit_private_name(name)?))
            }
        }
        Ok(Access::Property)
    }

    /// A computed key that's both read and written (`o[k] += 1`, `o[k]++`) is converted to
    /// a property key once, so `k`'s `toString` runs once.
    fn emit_key_once(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            if !matches!(&*computed.expr, Expr::Lit(_)) {
                self.emit_op(Operation::ToPropertyKey);
            }
        }
    }

    /// [obj, key] -> [value]
    fn emit_member_get(&mut self, access: Access) {
        self.emit_op(match access {
            Access::Property => Operation::GetProperty,
            Access::Private(_) => Operation::PrivateGet,
        });
    }

    /// [obj, key, value] -> [value]
    fn emit_member_set(&mut self, access: Access) {
        match access {
            Access::Property => self.emit_op(Operation::SetProperty),
            Access::Private(kind) => {
                self.emit_op(Operation::PrivateSet);
                if kind == PrivateKind::Accessor {
                    // drop the setter's return value
                    self.emit_op(Operation::Pop);
                }
            }
        }
    }

    /// Push the private name object `#x` refers to (from the enclosing class bodies).
    fn emit_private_name(&mut self, name: &PrivateName) -> CompileResult<PrivateKind> {
        let key = private_key(name);
        let Some(kind) = self.private_kind(&key) else {
            return unsupported(format!("undeclared private name `{key}`"), name.span);
        };
        match self.resolve_local(&key) {
            Some(VarRef::Local(slot)) => self.emit_op(Operation::GetLocal(slot)),
            Some(VarRef::Box(slot)) => self.emit_op(Operation::GetBox(slot)),
            Some(VarRef::Upvalue(index)) => self.emit_op(Operation::GetUpvalue(index)),
            None => unreachable!("a private name with a kind has a slot"),
        }
        Ok(kind)
    }

    /// The kind of the innermost private name called `key` (`#x`) in scope.
    fn private_kind(&self, key: &str) -> Option<PrivateKind> {
        match self.locals.iter().rev().find(|l| l.name == key) {
            Some(local) => local.private,
            None => self.enclosing?.private_kind(key),
        }
    }

    fn compile_member_expr(&mut self, member_expr: &MemberExpr) -> CompileResult {
        self.compile_expr(&member_expr.obj)?;
        let access = self.compile_member_prop(&member_expr.prop)?;
        self.emit_member_get(access);
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> CompileResult {
        match expr {
            Expr::Lit(lit) => self.compile_lit(lit)?,
            Expr::Call(call_expr) => self.compile_call(call_expr)?,
            Expr::New(new_expr) => self.compile_new(new_expr)?,
            Expr::Member(member_expr) => self.compile_member_expr(member_expr)?,
            Expr::Ident(ident) => self.emit_get_var(ident)?,
            Expr::This(_) => self.emit_op(Operation::This),
            Expr::Bin(bin_expr) => self.compile_bin_expr(bin_expr)?,
            Expr::Unary(unary_expr) => self.compile_unary_expr(unary_expr)?,
            Expr::Update(update_expr) => self.compile_update_expr(update_expr)?,
            Expr::Paren(paren_expr) => self.compile_expr(&paren_expr.expr)?,
            Expr::Assign(assign_expr) => self.compile_assign_expr(assign_expr)?,
            Expr::Array(array_lit) => self.compile_array_lit(array_lit)?,
            Expr::Object(object_lit) => self.compile_object_lit(object_lit)?,
            Expr::Cond(cond_expr) => self.compile_cond_expr(cond_expr)?,
            Expr::Seq(seq_expr) => {
                let last = seq_expr.exprs.len() - 1;
                for (i, expr) in seq_expr.exprs.iter().enumerate() {
                    self.compile_expr(expr)?;
                    if i != last {
                        self.emit_op(Operation::Pop);
                    }
                }
            }
            Expr::Fn(fn_expr) => {
                self.compile_function(fn_expr.ident.as_ref(), &fn_expr.function, true, false)?
            }
            Expr::Arrow(arrow) => self.compile_arrow(arrow, None, false)?,
            Expr::Tpl(tpl) => self.compile_tpl(tpl)?,
            Expr::Class(class_expr) => {
                let name = class_expr.ident.as_ref().map(|i| &*i.sym);
                self.compile_class(class_expr.ident.as_ref(), &class_expr.class, name)?
            }
            Expr::SuperProp(super_prop) => {
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::SuperGet);
            }
            Expr::MetaProp(meta) if meta.kind == MetaPropKind::NewTarget => {
                self.emit_op(Operation::NewTarget)
            }
            Expr::TaggedTpl(tagged) => self.compile_tagged_tpl(tagged)?,
            Expr::Yield(yield_expr) => self.compile_yield(yield_expr)?,
            Expr::Await(await_expr) => {
                if !self.coroutine.is_async() {
                    return unsupported("top-level await", await_expr.span);
                }
                self.compile_expr(&await_expr.arg)?;
                self.emit_await();
            }
            Expr::OptChain(chain) => {
                let mut nullish = Vec::new();
                self.compile_chain(chain, &mut nullish, false)?;
                self.end_chain(nullish, Operation::Undefined);
            }
            _ => return unsupported(expr_kind(expr), expr.span()),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // generators and async functions
    // ---------------------------------------------------------------------------------

    /// [value] -> [result]: suspend until the awaited value settles. A rejection resumes
    /// at the `Throw` right after, a fulfilment just after it.
    fn emit_await(&mut self) {
        self.emit_op(Operation::Await);
        self.emit_op(Operation::Throw);
    }

    /// Emit `Yield` for the value on top of the stack. `raw` yields it as the iterator
    /// result itself (`yield*` passing on its inner results). Resuming continues after it
    /// with the value sent in by `next(v)`; `throw(e)` throws e here and `return(v)`
    /// returns v from here (running `finally` blocks and closing `for-of` iterators).
    fn emit_yield(&mut self, raw: bool) -> CompileResult {
        let [normal, throw, ret] = self.emit_multi_jump(Operation::Yield(raw as u8));
        self.patch_from_end(throw);
        self.emit_op(Operation::Throw);
        self.patch_from_end(ret);
        self.emit_generator_return()?;
        self.patch_from_end(normal);
        Ok(())
    }

    /// Emit an instruction ending in N u32 forward offsets, each counted from the end of
    /// the instruction. Returns (instruction end, operand position) pairs to patch.
    fn emit_multi_jump<const N: usize>(&mut self, op: Operation) -> [(usize, usize); N] {
        self.emit_op(op);
        let end = self.bytecode.len();
        std::array::from_fn(|i| (end, end - 4 * (N - i)))
    }

    /// Point an offset from `emit_multi_jump` at the current end of the bytecode.
    fn patch_from_end(&mut self, (end, pos): (usize, usize)) {
        let offset = (self.bytecode.len() - end) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// [value] -> return it from the generator (an async generator awaits it first).
    fn emit_generator_return(&mut self) -> CompileResult {
        if self.coroutine == Coroutine::AsyncGenerator {
            self.emit_await();
        }
        self.emit_unwind_tries(0)?;
        self.emit_op(Operation::Return);
        Ok(())
    }

    fn compile_yield(&mut self, expr: &YieldExpr) -> CompileResult {
        if !expr.delegate {
            match &expr.arg {
                Some(arg) => self.compile_expr(arg)?,
                None => self.emit_op(Operation::Undefined),
            }
            // an async generator awaits what it yields
            if self.coroutine == Coroutine::AsyncGenerator {
                self.emit_await();
            }
            return self.emit_yield(false);
        }

        // `yield* inner`: pass each next / throw / return on to inner's iterator, yielding
        // its results, until it's done (its final value is the result)
        let is_async = self.coroutine.is_async();
        let iterator = self.alloc_slot()?;
        let mode = self.alloc_slot()?;
        let arg = expr.arg.as_deref().expect("yield* always has an argument");
        self.compile_expr(arg)?;
        self.emit_op(if is_async {
            Operation::GetAsyncIterator
        } else {
            Operation::GetIterator
        });
        self.emit_op(Operation::SetLocal(iterator));
        self.emit_op(Operation::Pop);
        self.emit_set_mode(mode, 0);
        // the first value passed on is `undefined`
        self.emit_op(Operation::Undefined);

        let start = self.bytecode.len();
        // [received] -> [inner result]
        let no_return = self.emit_jump(Operation::DelegateCall(iterator, mode, 0));
        if is_async {
            self.emit_await();
        }
        // done: [value] -> the result, or the generator's return value (mode 2)
        let [done, ret] = self.emit_multi_jump(Operation::DelegateResult(mode, is_async as u8));
        let [normal, throw, resumed_return] = self.emit_multi_jump(Operation::Yield(!is_async as u8));
        for (resume, m) in [(throw, 1), (resumed_return, 2), (normal, 0)] {
            self.patch_from_end(resume);
            self.emit_set_mode(mode, m);
            self.emit_loop(start);
        }

        // inner is done with a `return(v)`, or has no `return`: return v from here
        self.patch_jump(no_return);
        self.patch_from_end(ret);
        self.emit_generator_return()?;
        self.patch_from_end(done);
        Ok(())
    }

    /// Store a `yield*` resume mode (0 next, 1 throw, 2 return) in its slot.
    fn emit_set_mode(&mut self, slot: u8, mode: u8) {
        self.emit_op(Operation::LoadFloat64(mode as f64));
        self.emit_op(Operation::SetLocal(slot));
        self.emit_op(Operation::Pop);
    }

    // ---------------------------------------------------------------------------------
    // optional chaining
    // ---------------------------------------------------------------------------------

    /// Compile one link of an optional chain (`a?.b.c()`). A `?.` that finds null /
    /// undefined jumps (collected in `nullish`) to the end of the whole chain, with one
    /// value on the stack. With `method`, a member link pushes [obj, value] for a call.
    fn compile_chain(
        &mut self,
        chain: &OptChainExpr,
        nullish: &mut Vec<usize>,
        method: bool,
    ) -> CompileResult {
        match &*chain.base {
            OptChainBase::Member(member) => {
                self.compile_chain_part(&member.obj, nullish)?;
                if chain.optional {
                    nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                }
                if method {
                    self.emit_op(Operation::Dup);
                }
                let access = self.compile_member_prop(&member.prop)?;
                self.emit_member_get(access);
            }
            OptChainBase::Call(call) => {
                let is_method = self.compile_chain_callee(&call.callee, nullish)?;
                if chain.optional {
                    if is_method {
                        // [obj, f]: leave one value for the end of the chain
                        let call_it = self.emit_jump(Operation::JumpIfNotNullish(0));
                        self.emit_op(Operation::Pop);
                        nullish.push(self.emit_jump(Operation::Jump(0)));
                        self.patch_jump(call_it);
                    } else {
                        nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                    }
                }
                self.compile_args(&call.args)?;
                self.emit_op(if is_method {
                    Operation::CallMethod
                } else {
                    Operation::Call
                });
                if method {
                    unreachable!("a call is never used as a method receiver link");
                }
            }
        }
        Ok(())
    }

    /// An expression inside a chain: further links stay part of the same chain.
    fn compile_chain_part(&mut self, expr: &Expr, nullish: &mut Vec<usize>) -> CompileResult {
        match expr {
            Expr::OptChain(chain) => self.compile_chain(chain, nullish, false),
            _ => self.compile_expr(expr),
        }
    }

    /// Push a chain call's callee: [obj, f] for a method call (returns true), else [f].
    fn compile_chain_callee(&mut self, callee: &Expr, nullish: &mut Vec<usize>) -> CompileResult<bool> {
        match callee {
            Expr::OptChain(chain) if matches!(&*chain.base, OptChainBase::Member(_)) => {
                self.compile_chain(chain, nullish, true)?;
                Ok(true)
            }
            Expr::Member(member) => {
                self.compile_expr(&member.obj)?;
                self.emit_op(Operation::Dup);
                let access = self.compile_member_prop(&member.prop)?;
                self.emit_member_get(access);
                Ok(true)
            }
            Expr::SuperProp(super_prop) => {
                self.emit_op(Operation::This);
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::SuperGet);
                Ok(true)
            }
            _ => {
                self.compile_chain_part(callee, nullish)?;
                Ok(false)
            }
        }
    }

    /// Land a chain's short-circuit jumps: their value is replaced by `result`.
    fn end_chain(&mut self, nullish: Vec<usize>, result: Operation) {
        if nullish.is_empty() {
            return;
        }
        let end = self.emit_jump(Operation::Jump(0));
        for jump in nullish {
            self.patch_jump(jump);
        }
        self.emit_op(Operation::Pop);
        self.emit_op(result);
        self.patch_jump(end);
    }

    /// Compile an expression whose value is unused, then drop it. `x++` becomes `++x`
    /// (no old value to keep), and each part of `a, b` is dropped as it goes.
    fn compile_discarded(&mut self, expr: &Expr) -> CompileResult {
        match expr {
            Expr::Update(update) if !update.prefix => {
                let prefix = UpdateExpr {
                    prefix: true,
                    ..update.clone()
                };
                self.compile_update_expr(&prefix)?;
            }
            Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.compile_discarded(expr)?;
                }
                return Ok(());
            }
            _ => self.compile_expr(expr)?,
        }
        self.emit_op(Operation::Pop);
        Ok(())
    }

    fn compile_cond_expr(&mut self, expr: &CondExpr) -> CompileResult {
        self.compile_expr(&expr.test)?;
        let else_jump = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.cons)?;
        let end_jump = self.emit_jump(Operation::Jump(0));
        self.patch_jump(else_jump);
        self.emit_op(Operation::Pop);
        self.compile_expr(&expr.alt)?;
        self.patch_jump(end_jump);
        Ok(())
    }

    fn compile_update_expr(&mut self, expr: &UpdateExpr) -> CompileResult {
        // `++` / `--` convert to a number (or keep a BigInt) and step by one
        let step = || match expr.op {
            UpdateOp::PlusPlus => Operation::Increment,
            UpdateOp::MinusMinus => Operation::Decrement,
        };
        let mut arg = &*expr.arg;
        while let Expr::Paren(paren) = arg {
            arg = &paren.expr;
        }
        match arg {
            Expr::Ident(ident) => {
                self.emit_get_var(ident)?;
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                if !expr.prefix {
                    // keep the old (numeric) value as the result
                    self.emit_op(Operation::Dup);
                }
                self.emit_op(step());
                self.emit_set_var(ident)?;
                if !expr.prefix {
                    self.emit_op(Operation::Pop);
                }
            }
            Expr::Member(member_expr) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.emit_key_once(&member_expr.prop);
                self.emit_op(Operation::Dup2);
                self.emit_member_get(access);
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                let temp = if expr.prefix {
                    None
                } else {
                    Some(self.temp()?)
                };
                if let Some(temp) = temp {
                    self.emit_op(Operation::SetLocal(temp));
                }
                self.emit_op(step());
                self.emit_member_set(access);
                if let Some(temp) = temp {
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                }
            }
            Expr::SuperProp(super_prop) => {
                // [key] -> [key, key] -> [key, value]
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::Dup);
                self.emit_op(Operation::SuperGet);
                if !expr.prefix {
                    // the result is the old value, converted
                    self.emit_op(Operation::ToNumeric);
                }
                let temp = if expr.prefix {
                    None
                } else {
                    Some(self.temp()?)
                };
                if let Some(temp) = temp {
                    self.emit_op(Operation::SetLocal(temp));
                }
                self.emit_op(step());
                self.emit_op(Operation::SuperSet);
                if let Some(temp) = temp {
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                }
            }
            _ => return unsupported("update target", expr.arg.span()),
        }
        Ok(())
    }

    fn compile_assign_expr(&mut self, expr: &AssignExpr) -> CompileResult {
        if expr.op != AssignOp::Assign {
            return self.compile_compound_assign(expr);
        }
        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // stack: [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.compile_expr(&expr.right)?;
                self.emit_member_set(access);
            }
            AssignTarget::Simple(SimpleAssignTarget::SuperProp(super_prop)) => {
                self.compile_super_prop(&super_prop.prop)?;
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::SuperSet);
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                self.compile_expr_named(&expr.right, &ident.id.sym)?;
                self.emit_set_var(&ident.id)?;
            }
            // the expression's value is the right-hand side
            AssignTarget::Pat(AssignTargetPat::Array(array_pat)) => {
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::Dup);
                self.compile_array_pattern(array_pat, false)?;
            }
            AssignTarget::Pat(AssignTargetPat::Object(object_pat)) => {
                self.compile_expr(&expr.right)?;
                self.emit_op(Operation::Dup);
                self.compile_object_pattern(object_pat, false)?;
            }
            target => return unsupported("assignment target", target.span()),
        }
        Ok(())
    }

    fn compile_compound_assign(&mut self, expr: &AssignExpr) -> CompileResult {
        // `&&=`, `||=` and `??=` only evaluate and assign the RHS when needed
        let short_circuit = match expr.op {
            AssignOp::AndAssign => Some(Operation::JumpIfFalse(0)),
            AssignOp::OrAssign => Some(Operation::JumpIfTrue(0)),
            AssignOp::NullishAssign => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        let arithmetic = match expr.op {
            AssignOp::AddAssign => Some(Operation::Add),
            AssignOp::SubAssign => Some(Operation::Sub),
            AssignOp::MulAssign => Some(Operation::Mul),
            AssignOp::DivAssign => Some(Operation::Div),
            AssignOp::ModAssign => Some(Operation::Mod),
            AssignOp::LShiftAssign => Some(Operation::LShift),
            AssignOp::RShiftAssign => Some(Operation::RShift),
            AssignOp::ZeroFillRShiftAssign => Some(Operation::ZeroFillRShift),
            AssignOp::BitOrAssign => Some(Operation::BitOr),
            AssignOp::BitXorAssign => Some(Operation::BitXor),
            AssignOp::BitAndAssign => Some(Operation::BitAnd),
            AssignOp::ExpAssign => Some(Operation::Exp),
            _ => None,
        };

        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                let ident = &ident.id;
                self.emit_get_var(ident)?;
                if let Some(jump_op) = short_circuit {
                    let end = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr_named(&expr.right, &ident.sym)?;
                    self.emit_set_var(ident)?;
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_set_var(ident)?;
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member_expr)) => {
                // [obj, key] -> [obj, key, obj, key] -> [obj, key, value]
                self.compile_expr(&member_expr.obj)?;
                let access = self.compile_member_prop(&member_expr.prop)?;
                self.emit_key_once(&member_expr.prop);
                self.emit_op(Operation::Dup2);
                self.emit_member_get(access);
                if let Some(jump_op) = short_circuit {
                    let keep = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_member_set(access);
                    let end = self.emit_jump(Operation::Jump(0));
                    // not assigning: drop [obj, key] from under the value
                    self.patch_jump(keep);
                    let temp = self.temp()?;
                    self.emit_op(Operation::SetLocal(temp));
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_member_set(access);
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::SuperProp(super_prop)) => {
                // [key] -> [key, key] -> [key, value]
                self.compile_super_prop(&super_prop.prop)?;
                self.emit_op(Operation::Dup);
                self.emit_op(Operation::SuperGet);
                if let Some(jump_op) = short_circuit {
                    let keep = self.emit_jump(jump_op);
                    self.emit_op(Operation::Pop);
                    self.compile_expr(&expr.right)?;
                    self.emit_op(Operation::SuperSet);
                    let end = self.emit_jump(Operation::Jump(0));
                    // not assigning: drop the key from under the value
                    self.patch_jump(keep);
                    let temp = self.temp()?;
                    self.emit_op(Operation::SetLocal(temp));
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::Pop);
                    self.emit_op(Operation::GetLocal(temp));
                    self.patch_jump(end);
                } else if let Some(op) = arithmetic {
                    self.compile_expr(&expr.right)?;
                    self.emit_op(op);
                    self.emit_op(Operation::SuperSet);
                }
            }
            target => return unsupported("compound assignment target", target.span()),
        }
        Ok(())
    }

    fn compile_array_lit(&mut self, array_lit: &ArrayLit) -> CompileResult {
        if array_lit.elems.iter().flatten().any(|e| e.spread.is_some()) {
            return self.compile_spread_array(&array_lit.elems);
        }
        for elem in &array_lit.elems {
            match elem {
                Some(expr_or_spread) => {
                    if let Some(spread) = expr_or_spread.spread {
                        return unsupported("spread element", spread);
                    }
                    self.compile_expr(&expr_or_spread.expr)?;
                }
                None => self.emit_op(Operation::Hole),
            }
        }
        self.emit_op(Operation::Array(array_lit.elems.len() as u32));
        Ok(())
    }

    fn compile_object_lit(&mut self, object_lit: &ObjectLit) -> CompileResult {
        // leading plain, shorthand and method properties are built in one step; the rest
        // (from the first accessor, spread or `__proto__`) are added one at a time, in order
        let simple = object_lit
            .props
            .iter()
            .take_while(|p| {
                matches!(p, PropOrSpread::Prop(prop)
                    if matches!(&**prop, Prop::Shorthand(_) | Prop::Method(_))
                        || matches!(&**prop, Prop::KeyValue(kv) if !is_proto_key(&kv.key)))
            })
            .count();
        for prop in &object_lit.props[..simple] {
            if let PropOrSpread::Prop(prop) = prop {
                self.compile_prop(prop)?;
            }
        }
        self.emit_op(Operation::Object(simple as u32));
        for prop in &object_lit.props[simple..] {
            let prop = match prop {
                PropOrSpread::Spread(spread) => {
                    self.compile_expr(&spread.expr)?;
                    self.emit_op(Operation::ObjectSpread);
                    continue;
                }
                PropOrSpread::Prop(prop) => &**prop,
            };
            match prop {
                Prop::Getter(getter) => {
                    self.compile_method(&getter.key, &getter.function, 1)?;
                    self.emit_op(Operation::DefineAccessor(0));
                }
                Prop::Setter(setter) => {
                    self.compile_method(&setter.key, &setter.function, 2)?;
                    self.emit_op(Operation::DefineAccessor(1));
                }
                // `__proto__: value` sets the prototype rather than defining a property (and
                // doesn't name an anonymous function value)
                Prop::KeyValue(key_value) if is_proto_key(&key_value.key) => {
                    self.emit_op(Operation::Dup);
                    self.compile_prop_name(&key_value.key)?;
                    self.compile_expr(&key_value.value)?;
                    self.emit_op(Operation::SetProperty);
                    self.emit_op(Operation::Pop);
                }
                _ => {
                    self.compile_prop(prop)?;
                    self.emit_op(Operation::DefineField);
                }
            }
        }
        Ok(())
    }

    /// Push a key-value, shorthand or method property's key and value.
    fn compile_prop(&mut self, prop: &Prop) -> CompileResult {
        match prop {
            Prop::KeyValue(key_value) => {
                self.compile_prop_name(&key_value.key)?;
                match static_key_name(&key_value.key) {
                    Some(name) => self.compile_expr_named(&key_value.value, &name)?,
                    None => {
                        self.compile_expr(&key_value.value)?;
                        if is_anonymous_fn(&key_value.value) {
                            self.emit_op(Operation::SetFunctionName(0));
                        }
                    }
                }
            }
            Prop::Shorthand(ident) => {
                self.emit_op(load_str(&ident.sym));
                self.emit_get_var(ident)?;
            }
            Prop::Method(method) => self.compile_method(&method.key, &method.function, 0)?,
            prop => return unsupported("object property kind", prop.span()),
        }
        Ok(())
    }

    /// Push a method's key and function. `prefix` 1 / 2 names it as a getter / setter.
    fn compile_method(&mut self, key: &PropName, function: &Function, prefix: u8) -> CompileResult {
        self.compile_prop_name(key)?;
        let name = static_key_name(key).map(|name| match prefix {
            1 => format!("get {name}"),
            2 => format!("set {name}"),
            _ => name,
        });
        self.compile_fn_value(name.as_deref(), None, function, false, FnKind::Method)?;
        if name.is_none() {
            self.emit_op(Operation::SetFunctionName(prefix));
        }
        Ok(())
    }

    /// Compile `expr`, naming it `name` if it's an anonymous function (`let f = () => {}`).
    fn compile_expr_named(&mut self, expr: &Expr, name: &str) -> CompileResult {
        let mut unwrapped = expr;
        while let Expr::Paren(paren) = unwrapped {
            unwrapped = &paren.expr;
        }
        match unwrapped {
            Expr::Fn(fn_expr) if fn_expr.ident.is_none() => {
                self.compile_fn_value(Some(name), None, &fn_expr.function, false, FnKind::Normal)
            }
            Expr::Arrow(arrow) => self.compile_arrow(arrow, Some(name), false),
            Expr::Class(class_expr) if class_expr.ident.is_none() => {
                self.compile_class(None, &class_expr.class, Some(name))
            }
            _ => self.compile_expr(expr),
        }
    }

    // ---------------------------------------------------------------------------------
    // classes
    // ---------------------------------------------------------------------------------

    /// Push a class. `ident` is the class's own name, bound (read-only) inside its body;
    /// `name` is its `name` property (the own name, or one inferred from where it's stored).
    fn compile_class(
        &mut self,
        ident: Option<&Ident>,
        class: &Class,
        name: Option<&str>,
    ) -> CompileResult {
        let outer = self.class_anchor.replace(class.span.lo);
        let compiled = self.compile_class_body(ident, class, name);
        self.class_anchor = outer;
        compiled
    }

    fn compile_class_body(
        &mut self,
        ident: Option<&Ident>,
        class: &Class,
        name: Option<&str>,
    ) -> CompileResult {
        self.enter_scope();
        let inner = match ident {
            Some(ident) => {
                let slot = self.declare_variable(ident.sym.to_string())?;
                self.locals.last_mut().unwrap().read_only = true;
                // `class A extends A` can't use it; methods (made later) always can
                let body_start = class.super_class.as_ref().map_or(ident.span.hi, |s| s.span().hi);
                self.mark_tdz(body_start);
                Some(slot)
            }
            None => None,
        };

        // [superclass?, constructor] -> MakeClass -> [class]
        let derived = class.super_class.is_some();
        if let Some(super_class) = &class.super_class {
            self.compile_expr(super_class)?;
        }

        // the body's private names are fresh each time the class is evaluated; the
        // superclass expression above still sees the outer ones
        for (key, kind) in class_private_names(class)? {
            let slot = self.declare_variable(key)?;
            self.locals.last_mut().unwrap().private = Some(kind);
            self.emit_op(Operation::NewPrivateName(kind as u8));
            self.emit_store_slot(slot);
            self.emit_op(Operation::Pop);
        }
        let constructor = class.body.iter().find_map(|m| match m {
            ClassMember::Constructor(c) => Some(c),
            _ => None,
        });
        let kind = if derived {
            FnKind::DerivedClass
        } else {
            FnKind::BaseClass
        };
        match constructor {
            Some(constructor) => {
                let mut params = Vec::new();
                for param in &constructor.params {
                    match param {
                        ParamOrTsParamProp::Param(param) => params.push(&param.pat),
                        ParamOrTsParamProp::TsParamProp(p) => {
                            return unsupported("parameter property", p.span)
                        }
                    }
                }
                let body = match &constructor.body {
                    Some(body) => FnBody::Block(&body.stmts),
                    None => FnBody::Block(&[]),
                };
                self.compile_closure(name, None, &params, &body, false, kind, Coroutine::None)?;
            }
            // `constructor(...args) { super(...args); }` for derived classes
            None if derived => {
                self.compile_closure(name, None, &[], &FnBody::DefaultDerived, false, kind, Coroutine::None)?
            }
            None => self.compile_closure(name, None, &[], &FnBody::Block(&[]), false, kind, Coroutine::None)?,
        }
        self.emit_op(Operation::MakeClass(derived as u8));

        // methods and accessors, then instance fields (initialised on construction)
        let mut statics = Vec::new();
        let mut instance_fields = Vec::new();
        for member in &class.body {
            match member {
                ClassMember::Constructor(_) | ClassMember::Empty(_) => {}
                ClassMember::Method(method) => {
                    // [class] -> [class, target]
                    self.emit_op(Operation::Dup);
                    if !method.is_static {
                        self.emit_op(load_str("prototype"));
                        self.emit_op(Operation::GetProperty);
                    }
                    let kind = match method.kind {
                        MethodKind::Method => 0,
                        MethodKind::Getter => 1,
                        MethodKind::Setter => 2,
                    };
                    self.compile_method(&method.key, &method.function, kind)?;
                    self.emit_op(Operation::DefineMethod(kind));
                }
                ClassMember::ClassProp(prop) if prop.is_static => {
                    // a computed key is evaluated (and converted) in source order, its
                    // value later with the other static initialisers
                    let key_slot = match &prop.key {
                        PropName::Computed(_) => {
                            let slot = self.alloc_slot()?;
                            self.compile_prop_name(&prop.key)?;
                            self.emit_op(Operation::ToPropertyKey);
                            self.emit_op(Operation::SetLocal(slot));
                            self.emit_op(Operation::Pop);
                            Some(slot)
                        }
                        _ => None,
                    };
                    statics.push((member, key_slot));
                }
                ClassMember::ClassProp(prop) => {
                    // the key now; the value when an instance is set up
                    self.emit_op(Operation::Dup);
                    self.compile_prop_name(&prop.key)?;
                    self.emit_op(Operation::ClassField);
                    instance_fields.push(FieldInit {
                        value: prop.value.as_deref(),
                        name: static_key_name(&prop.key),
                    });
                }
                ClassMember::StaticBlock(_) => statics.push((member, None)),
                ClassMember::PrivateMethod(method) => {
                    // [class] -> [class, class, name, fn] -> [class]
                    self.emit_op(Operation::Dup);
                    self.emit_private_name(&method.key)?;
                    let key = private_key(&method.key);
                    let (kind, name) = match method.kind {
                        MethodKind::Method => (0, key),
                        MethodKind::Getter => (1, format!("get {key}")),
                        MethodKind::Setter => (2, format!("set {key}")),
                    };
                    // accessors are only ever called by the VM; a method only needs a host
                    // wrapper if it's used as a value (all its uses are in this class body)
                    let internal = kind != 0 || !private_method_escapes(&method.key, class);
                    self.compile_fn_value(
                        Some(&name),
                        None,
                        &method.function,
                        internal,
                        FnKind::Method,
                    )?;
                    let static_flag = if method.is_static { 4 } else { 0 };
                    self.emit_op(Operation::DefinePrivateMethod(kind | static_flag));
                }
                ClassMember::PrivateProp(prop) if prop.is_static => statics.push((member, None)),
                ClassMember::PrivateProp(prop) => {
                    self.emit_op(Operation::Dup);
                    self.emit_private_name(&prop.key)?;
                    self.emit_op(Operation::ClassField);
                    instance_fields.push(FieldInit {
                        value: prop.value.as_deref(),
                        name: Some(private_key(&prop.key)),
                    });
                }
                member => return unsupported("class member", member.span()),
            }
        }

        // all instance field values are set up by one hidden method, run on each new instance
        if !instance_fields.is_empty() {
            self.emit_op(Operation::Dup);
            let body = FnBody::InstanceFields(&instance_fields);
            self.compile_closure(None, None, &[], &body, true, FnKind::Method, Coroutine::None)?;
            self.emit_op(Operation::ClassInit);
        }

        // the class's own name is usable from here on (static initialisers can use it)
        if let Some(slot) = inner {
            self.emit_store_slot(slot);
        }

        // static fields and blocks run in order, with `this` as the class
        for (member, key_slot) in statics {
            self.emit_op(Operation::Dup);
            match member {
                ClassMember::ClassProp(prop) => {
                    match key_slot {
                        Some(slot) => self.emit_op(Operation::GetLocal(slot)),
                        None => self.compile_prop_name(&prop.key)?,
                    }
                    self.compile_field_init(static_key_name(&prop.key), prop.value.as_deref())?;
                    self.emit_op(Operation::StaticField);
                }
                ClassMember::PrivateProp(prop) => {
                    self.compile_private_field(prop)?;
                    self.emit_op(Operation::StaticField);
                }
                ClassMember::StaticBlock(block) => {
                    let body = FnBody::Block(&block.body.stmts);
                    self.compile_closure(None, None, &[], &body, true, FnKind::Method, Coroutine::None)?;
                    self.emit_op(Operation::StaticBlock);
                }
                _ => unreachable!(),
            }
        }
        self.exit_scope();
        Ok(())
    }

    /// Push a private field's name object and initialiser.
    fn compile_private_field(&mut self, prop: &PrivateProp) -> CompileResult {
        self.emit_private_name(&prop.key)?;
        self.compile_field_init(Some(private_key(&prop.key)), prop.value.as_deref())
    }

    /// Push a field's initialiser: a hidden method returning the value (run with `this` as
    /// the instance, or the class for static fields), or `undefined` for `x;`. `name` names
    /// an anonymous function value.
    fn compile_field_init(&mut self, name: Option<String>, value: Option<&Expr>) -> CompileResult {
        match value {
            Some(value) => {
                let body = FnBody::Field(value, name);
                self.compile_closure(None, None, &[], &body, true, FnKind::Method, Coroutine::None)
            }
            None => {
                self.emit_op(Operation::Undefined);
                Ok(())
            }
        }
    }

    /// Push the key of `super.x` / `super[x]`.
    fn compile_super_prop(&mut self, prop: &SuperProp) -> CompileResult {
        match prop {
            SuperProp::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            SuperProp::Computed(computed) => self.compile_expr(&computed.expr)?,
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // templates
    // ---------------------------------------------------------------------------------

    fn compile_tpl(&mut self, tpl: &Tpl) -> CompileResult {
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            let Some(cooked) = &quasi.cooked else {
                return unsupported("invalid escape in template", quasi.span);
            };
            self.emit_op(Operation::LoadString(cooked.as_bytes().to_vec()));
            if let Some(expr) = tpl.exprs.get(i) {
                self.compile_expr(expr)?;
            }
        }
        if !tpl.exprs.is_empty() {
            let parts = tpl.quasis.len() + tpl.exprs.len();
            self.emit_op(Operation::Template(parts as u32));
        }
        Ok(())
    }

    /// ``tag`a${b}c` `` calls `tag(strings, b)`; ``obj.tag`...` `` passes obj as `this`.
    fn compile_tagged_tpl(&mut self, tagged: &TaggedTpl) -> CompileResult {
        let mut tag = &*tagged.tag;
        while let Expr::Paren(paren) = tag {
            tag = &paren.expr;
        }
        let is_method = if let Expr::Member(member_expr) = tag {
            self.compile_expr(&member_expr.obj)?;
            self.emit_op(Operation::Dup);
            let access = self.compile_member_prop(&member_expr.prop)?;
            self.emit_member_get(access);
            true
        } else {
            self.compile_expr(&tagged.tag)?;
            false
        };
        let pieces = tagged
            .tpl
            .quasis
            .iter()
            .map(|q| {
                let cooked = q.cooked.as_ref().map(|c| c.as_bytes().to_vec());
                (cooked, q.raw.as_bytes().to_vec())
            })
            .collect();
        self.emit_op(Operation::TemplateObject(pieces));
        for expr in &tagged.tpl.exprs {
            self.compile_expr(expr)?;
        }
        self.emit_op(Operation::UInt64(tagged.tpl.exprs.len() as u64 + 1));
        self.emit_op(if is_method {
            Operation::CallMethod
        } else {
            Operation::Call
        });
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // destructuring
    // ---------------------------------------------------------------------------------

    /// Push a property key.
    fn compile_prop_name(&mut self, key: &PropName) -> CompileResult {
        match key {
            PropName::Ident(ident) => self.emit_op(load_str(&ident.sym)),
            PropName::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            // the VM converts the number to a key the same way JS does
            PropName::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            PropName::Computed(computed) => self.compile_expr(&computed.expr)?,
            key => return unsupported("object key", key.span()),
        }
        Ok(())
    }

    /// Replace the value on top of the stack with `default` if it is `undefined`. `name`
    /// names an anonymous function default after its target.
    fn emit_default(&mut self, default: &Expr, name: Option<&str>) -> CompileResult {
        self.emit_op(Operation::Dup);
        self.emit_op(Operation::Undefined);
        self.emit_op(Operation::EqEqEq);
        let skip = self.emit_jump(Operation::JumpIfFalse(0));
        self.emit_op(Operation::Pop);
        self.emit_op(Operation::Pop);
        match name {
            Some(name) => self.compile_expr_named(default, name)?,
            None => self.compile_expr(default)?,
        }
        let end = self.emit_jump(Operation::Jump(0));
        self.patch_jump(skip);
        self.emit_op(Operation::Pop);
        self.patch_jump(end);
        Ok(())
    }

    /// Store the value on top of the stack into `pat`, consuming it. `init` initialises
    /// declared names (so `const` is allowed); otherwise this is an assignment.
    fn compile_pattern(&mut self, pat: &Pat, init: bool) -> CompileResult {
        match pat {
            Pat::Ident(ident) => {
                if init {
                    self.emit_init_var(&ident.id)?;
                } else {
                    self.emit_set_var(&ident.id)?;
                }
                self.emit_op(Operation::Pop);
            }
            Pat::Assign(assign) => {
                let name = match &*assign.left {
                    Pat::Ident(ident) => Some(&*ident.id.sym),
                    _ => None,
                };
                self.emit_default(&assign.right, name)?;
                self.compile_pattern(&assign.left, init)?;
            }
            Pat::Array(array_pat) => self.compile_array_pattern(array_pat, init)?,
            Pat::Object(object_pat) => self.compile_object_pattern(object_pat, init)?,
            // assignment targets: `[o.x, (y)] = ...`
            Pat::Expr(expr) => {
                let mut expr = &**expr;
                while let Expr::Paren(paren) = expr {
                    expr = &paren.expr;
                }
                match expr {
                    Expr::Ident(ident) => {
                        self.emit_set_var(ident)?;
                        self.emit_op(Operation::Pop);
                    }
                    Expr::Member(member_expr) => {
                        let value = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(value));
                        self.emit_op(Operation::Pop);
                        self.compile_expr(&member_expr.obj)?;
                        let access = self.compile_member_prop(&member_expr.prop)?;
                        self.emit_op(Operation::GetLocal(value));
                        self.emit_member_set(access);
                        self.emit_op(Operation::Pop);
                    }
                    Expr::SuperProp(super_prop) => {
                        let value = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(value));
                        self.emit_op(Operation::Pop);
                        self.compile_super_prop(&super_prop.prop)?;
                        self.emit_op(Operation::GetLocal(value));
                        self.emit_op(Operation::SuperSet);
                        self.emit_op(Operation::Pop);
                    }
                    expr => return unsupported("assignment target", expr.span()),
                }
            }
            pat => return unsupported("pattern", pat.span()),
        }
        Ok(())
    }

    /// `[a, , b = 1, ...rest]`: the value is iterated (only as far as needed) into an array
    /// held in a hidden slot, then each element is stored.
    fn compile_array_pattern(&mut self, array_pat: &ArrayPat, init: bool) -> CompileResult {
        let has_rest = matches!(array_pat.elems.last(), Some(Some(Pat::Rest(_))));
        let count = if has_rest {
            ITER_TAKE_ALL
        } else {
            array_pat.elems.len() as u32
        };
        let items = self.alloc_slot()?;
        self.emit_op(Operation::IterTake(count));
        self.emit_op(Operation::SetLocal(items));
        self.emit_op(Operation::Pop);
        for (i, elem) in array_pat.elems.iter().enumerate() {
            let Some(pat) = elem else { continue };
            if let Pat::Rest(rest) = pat {
                self.emit_op(Operation::GetLocal(items));
                self.emit_op(Operation::ArraySliceFrom(i as u32));
                self.compile_pattern(&rest.arg, init)?;
            } else {
                self.emit_op(Operation::GetSlotIndex(items, i as u32));
                self.compile_pattern(pat, init)?;
            }
        }
        Ok(())
    }

    /// `{ a, b: c, [k]: d = 1, ...rest }`: the value is held in a hidden slot while each
    /// property is read and stored. With a rest element, the keys used are collected too.
    fn compile_object_pattern(&mut self, object_pat: &ObjectPat, init: bool) -> CompileResult {
        let source = self.alloc_slot()?;
        self.emit_op(Operation::RequireObjectCoercible);
        self.emit_op(Operation::SetLocal(source));
        self.emit_op(Operation::Pop);

        let has_rest = object_pat
            .props
            .iter()
            .any(|p| matches!(p, ObjectPatProp::Rest(_)));
        let used_keys = if has_rest {
            let slot = self.alloc_slot()?;
            self.emit_op(Operation::Array(0));
            self.emit_op(Operation::SetLocal(slot));
            self.emit_op(Operation::Pop);
            Some(slot)
        } else {
            None
        };

        for prop in &object_pat.props {
            match prop {
                ObjectPatProp::KeyValue(key_value) if used_keys.is_none() => {
                    // `{ a: x }`: one read
                    match &key_value.key {
                        PropName::Ident(ident) => self.emit_op(Operation::GetSlotProp(
                            source,
                            ident.sym.as_bytes().to_vec(),
                        )),
                        PropName::Str(s) => self.emit_op(Operation::GetSlotProp(
                            source,
                            s.value.as_bytes().to_vec(),
                        )),
                        key => {
                            self.emit_op(Operation::GetLocal(source));
                            self.compile_prop_name(key)?;
                            self.emit_op(Operation::GetProperty);
                        }
                    }
                    self.compile_pattern(&key_value.value, init)?;
                }
                ObjectPatProp::KeyValue(key_value) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.compile_prop_name(&key_value.key)?;
                    if let Some(used) = used_keys {
                        // [source, key] -> record the key, keep it for the read
                        self.emit_op(Operation::Dup);
                        let key = self.alloc_slot()?;
                        self.emit_op(Operation::SetLocal(key));
                        self.emit_op(Operation::Pop);
                        self.emit_op(Operation::GetLocal(used));
                        self.emit_op(Operation::GetLocal(key));
                        self.emit_op(Operation::ArrayAppend);
                        self.emit_op(Operation::Pop);
                    }
                    self.emit_op(Operation::GetProperty);
                    self.compile_pattern(&key_value.value, init)?;
                }
                ObjectPatProp::Assign(assign) => {
                    // shorthand `{ a }` / `{ a = 1 }`
                    let name = &assign.key.id;
                    if let Some(used) = used_keys {
                        self.emit_op(Operation::GetLocal(used));
                        self.emit_op(load_str(&name.sym));
                        self.emit_op(Operation::ArrayAppend);
                        self.emit_op(Operation::Pop);
                    }
                    self.emit_op(Operation::GetSlotProp(source, name.sym.as_bytes().to_vec()));
                    if let Some(default) = &assign.value {
                        self.emit_default(default, Some(&name.sym))?;
                    }
                    if init {
                        self.emit_init_var(name)?;
                    } else {
                        self.emit_set_var(name)?;
                    }
                    self.emit_op(Operation::Pop);
                }
                ObjectPatProp::Rest(rest) => {
                    self.emit_op(Operation::GetLocal(source));
                    self.emit_op(Operation::GetLocal(used_keys.expect("rest has a key list")));
                    self.emit_op(Operation::ObjectRest);
                    self.compile_pattern(&rest.arg, init)?;
                }
            }
        }
        Ok(())
    }

    fn compile_bin_expr(&mut self, expr: &BinExpr) -> CompileResult {
        // `&&`, `||` and `??` only evaluate the right side when needed
        let short_circuit = match expr.op {
            BinaryOp::LogicalAnd => Some(Operation::JumpIfFalse(0)),
            BinaryOp::LogicalOr => Some(Operation::JumpIfTrue(0)),
            BinaryOp::NullishCoalescing => Some(Operation::JumpIfNotNullish(0)),
            _ => None,
        };
        if let Some(jump_op) = short_circuit {
            self.compile_expr(&expr.left)?;
            let end = self.emit_jump(jump_op);
            self.emit_op(Operation::Pop);
            self.compile_expr(&expr.right)?;
            self.patch_jump(end);
            return Ok(());
        }

        // `#x in obj`: a brand check
        if let (BinaryOp::In, Expr::PrivateName(name)) = (expr.op, &*expr.left) {
            self.emit_private_name(name)?;
            self.compile_expr(&expr.right)?;
            self.emit_op(Operation::PrivateIn);
            return Ok(());
        }

        self.compile_expr(&expr.left)?;
        self.compile_expr(&expr.right)?;
        let op = match expr.op {
            BinaryOp::Add => Operation::Add,
            BinaryOp::Sub => Operation::Sub,
            BinaryOp::Mul => Operation::Mul,
            BinaryOp::Div => Operation::Div,
            BinaryOp::Mod => Operation::Mod,
            BinaryOp::EqEq => Operation::EqEq,
            BinaryOp::NotEq => Operation::NotEq,
            BinaryOp::EqEqEq => Operation::EqEqEq,
            BinaryOp::NotEqEq => Operation::NotEqEq,
            BinaryOp::Lt => Operation::Lt,
            BinaryOp::LtEq => Operation::LtEq,
            BinaryOp::Gt => Operation::Gt,
            BinaryOp::GtEq => Operation::GtEq,
            BinaryOp::LShift => Operation::LShift,
            BinaryOp::RShift => Operation::RShift,
            BinaryOp::ZeroFillRShift => Operation::ZeroFillRShift,
            BinaryOp::BitOr => Operation::BitOr,
            BinaryOp::BitXor => Operation::BitXor,
            BinaryOp::BitAnd => Operation::BitAnd,
            BinaryOp::In => Operation::In,
            BinaryOp::InstanceOf => Operation::InstanceOf,
            BinaryOp::Exp => Operation::Exp,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing => {
                unreachable!("handled above")
            }
        };
        self.emit_op(op);
        Ok(())
    }

    fn compile_unary_expr(&mut self, expr: &UnaryExpr) -> CompileResult {
        if expr.op == UnaryOp::Delete {
            // `delete obj.key` needs the object and key, not the property value
            // `delete a?.b`: true without deleting anything when `a` is null / undefined
            if let Expr::OptChain(chain) = &*expr.arg {
                if let OptChainBase::Member(member) = &*chain.base {
                    let mut nullish = Vec::new();
                    self.compile_chain_part(&member.obj, &mut nullish)?;
                    if chain.optional {
                        nullish.push(self.emit_jump(Operation::JumpIfNullish(0)));
                    }
                    self.compile_member_prop(&member.prop)?;
                    self.emit_op(Operation::Delete);
                    self.end_chain(nullish, Operation::Bool(true));
                    return Ok(());
                }
            }
            let Expr::Member(member_expr) = &*expr.arg else {
                // `delete 1`, `delete f()`: evaluated for side effects, always true
                self.compile_expr(&expr.arg)?;
                self.emit_op(Operation::Pop);
                self.emit_op(Operation::Bool(true));
                return Ok(());
            };
            if let MemberProp::PrivateName(name) = &member_expr.prop {
                return unsupported("delete of a private name", name.span);
            }
            self.compile_expr(&member_expr.obj)?;
            self.compile_member_prop(&member_expr.prop)?;
            self.emit_op(Operation::Delete);
            return Ok(());
        }

        // `typeof window` must not throw when the host has no `window`
        if expr.op == UnaryOp::TypeOf {
            if let Expr::Ident(ident) = &*expr.arg {
                if let (None, Some(host)) = (
                    self.resolve_local(&ident.sym),
                    HostGlobal::from_name(&ident.sym),
                ) {
                    self.emit_op(Operation::HostGlobal(host as u8 | HOST_GLOBAL_TYPEOF));
                    self.emit_op(Operation::TypeOf);
                    return Ok(());
                }
            }
        }

        self.compile_expr(&expr.arg)?;
        let op = match expr.op {
            UnaryOp::Minus => Operation::UnaryMinus,
            UnaryOp::Plus => Operation::UnaryPlus,
            UnaryOp::Bang => Operation::LogicalNot,
            UnaryOp::Tilde => Operation::BitwiseNot,
            UnaryOp::TypeOf => Operation::TypeOf,
            UnaryOp::Void => Operation::Void,
            UnaryOp::Delete => unreachable!("handled above"),
        };
        self.emit_op(op);
        Ok(())
    }

    fn compile_lit(&mut self, lit: &Lit) -> CompileResult {
        match lit {
            Lit::Str(s) => self.emit_op(Operation::LoadString(s.value.as_bytes().to_vec())),
            Lit::Num(n) => self.emit_op(Operation::LoadFloat64(n.value)),
            Lit::Bool(b) => self.emit_op(Operation::Bool(b.value)),
            Lit::Null(_) => self.emit_op(Operation::Null),
            Lit::Regex(r) => {
                self.emit_op(Operation::Regex(r.exp.to_string(), r.flags.to_string()));
            }
            Lit::BigInt(b) => self.emit_op(Operation::LoadBigInt(b.value.to_string())),
            Lit::JSXText(j) => return unsupported("JSX", j.span),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // emitting
    // ---------------------------------------------------------------------------------

    /// Emit a forward jump with a placeholder offset; returns the offset's position for `patch_jump`.
    fn emit_jump(&mut self, op: Operation) -> usize {
        self.emit_op(op);
        self.bytecode.len() - 4
    }

    /// Point a jump emitted by `emit_jump` at the current end of the bytecode.
    fn patch_jump(&mut self, pos: usize) {
        let offset = (self.bytecode.len() - (pos + 4)) as u32;
        self.bytecode[pos..pos + 4].copy_from_slice(&offset.to_le_bytes());
    }

    /// Emit a backward jump to `start`.
    fn emit_loop(&mut self, start: usize) {
        // the VM lands on `start` after reading the 4 byte operand
        let offset = (self.bytecode.len() + 1 + 4 - start) as u32;
        self.emit_op(Operation::Loop(offset));
    }

    fn emit_op(&mut self, op: Operation) {
        self.bytecode.push(self.opcode_map.to_byte[op.get_opcode() as usize]);
        match op {
            Operation::LoadString(bytes) => self.emit_string(&bytes),
            Operation::LoadBigInt(digits) => self.emit_string(digits.as_bytes()),
            Operation::IterNext(slot, offset) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::IterClose(slot, mode) => {
                self.bytecode.push(slot);
                self.bytecode.push(mode);
            }
            Operation::GetSlotIndex(slot, index) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&index.to_le_bytes());
            }
            Operation::GetSlotProp(slot, name) => {
                self.bytecode.push(slot);
                self.emit_string(&name);
            }
            Operation::InitField(index, name_it) => {
                self.bytecode.extend_from_slice(&index.to_le_bytes());
                self.bytecode.push(name_it);
            }
            Operation::Yield(raw) => {
                self.bytecode.push(raw);
                self.bytecode.extend_from_slice(&[0; 12]);
            }
            Operation::DelegateCall(iterator, mode, offset) => {
                self.bytecode.push(iterator);
                self.bytecode.push(mode);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::DelegateResult(mode, is_async) => {
                self.bytecode.push(mode);
                self.bytecode.push(is_async);
                self.bytecode.extend_from_slice(&[0; 8]);
            }
            Operation::AsyncIterNext(slot) | Operation::IterCloseCheck(slot) => {
                self.bytecode.push(slot)
            }
            Operation::IterResult(slot, offset) => {
                self.bytecode.push(slot);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::AsyncIterClose(slot, mode, offset) => {
                self.bytecode.push(slot);
                self.bytecode.push(mode);
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            Operation::LoadFloat64(n) => {
                let bytes = n.to_le_bytes();
                self.bytecode.extend_from_slice(&bytes);
            }
            Operation::Bool(b) => self.bytecode.push(b as u8),
            Operation::Regex(exp, flags) => {
                self.emit_string(exp.as_bytes());
                self.emit_string(flags.as_bytes());
            }
            Operation::UInt8(n) => self.bytecode.push(n),
            Operation::UInt16(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt32(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::UInt64(n) => {
                // can't easily emit 64-bit integers in JS, so we split into two 32-bit integers
                let low = n as u32;
                let high = (n >> 32) as u32;
                self.bytecode.extend_from_slice(&low.to_le_bytes());
                self.bytecode.extend_from_slice(&high.to_le_bytes());
            }
            Operation::Function(FnInfo {
                length,
                params,
                kind,
                coroutine,
                name,
                captures,
                code,
                ..
            }) => {
                self.bytecode.push(length);
                self.bytecode.push(params);
                // kind, | 8 generator, | 16 async
                self.bytecode.push(
                    kind as u8
                        | (coroutine.is_generator() as u8) << 3
                        | (coroutine.is_async() as u8) << 4,
                );
                self.emit_string(name.as_bytes());
                // captures: count, then (from_local, index) pairs
                self.bytecode.push(captures.len() as u8);
                for (from_local, index) in captures {
                    self.bytecode.push(from_local as u8);
                    self.bytecode.push(index);
                }
                let code_len = code.len() as u32;
                self.bytecode.extend_from_slice(&code_len.to_le_bytes());
                self.bytecode.extend_from_slice(&code);
            }
            Operation::GetUpvalue(index) | Operation::SetUpvalue(index) => {
                self.bytecode.push(index)
            }
            Operation::GetBox(slot)
            | Operation::SetBox(slot)
            | Operation::NewBox(slot)
            | Operation::BoxLocal(slot)
            | Operation::RenewBox(slot)
            | Operation::NewBoxTdz(slot)
            | Operation::Tdz(slot)
            | Operation::RestArgs(slot) => self.bytecode.push(slot),
            Operation::HostGlobal(id) => self.bytecode.push(id),
            Operation::DefineAccessor(n)
            | Operation::SetFunctionName(n)
            | Operation::MakeClass(n)
            | Operation::DefineMethod(n)
            | Operation::NewPrivateName(n)
            | Operation::DefinePrivateMethod(n) => self.bytecode.push(n),
            Operation::TemplateObject(pieces) => {
                self.bytecode
                    .extend_from_slice(&(pieces.len() as u32).to_le_bytes());
                for (cooked, raw) in pieces {
                    self.bytecode.push(cooked.is_some() as u8);
                    if let Some(cooked) = cooked {
                        self.emit_string(&cooked);
                    }
                    self.emit_string(&raw);
                }
            }
            Operation::GetLocal(index) | Operation::SetLocal(index) => self.bytecode.push(index),
            Operation::Array(n)
            | Operation::Object(n)
            | Operation::IterTake(n)
            | Operation::ArraySliceFrom(n)
            | Operation::Template(n) => self.bytecode.extend_from_slice(&n.to_le_bytes()),
            Operation::Jump(offset)
            | Operation::JumpIfFalse(offset)
            | Operation::JumpIfTrue(offset)
            | Operation::JumpIfNotNullish(offset)
            | Operation::JumpIfNullish(offset)
            | Operation::Loop(offset)
            | Operation::TryStart(offset) => {
                self.bytecode.extend_from_slice(&offset.to_le_bytes());
            }
            _ => {}
        }
    }

    /// Strings are a u32 byte length followed by WTF-8 bytes.
    fn emit_string(&mut self, bytes: &[u8]) {
        self.bytecode
            .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.bytecode.extend_from_slice(bytes);
    }
}

impl Default for Compiler<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::With(_) => "with statement",
        Stmt::Decl(Decl::Class(_)) => "class declaration",
        Stmt::Decl(_) => "declaration",
        _ => "statement",
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Arrow(_) => "arrow function",
        Expr::Class(_) => "class expression",
        Expr::Tpl(_) | Expr::TaggedTpl(_) => "template literal",
        Expr::SuperProp(_) => "super property",
        Expr::MetaProp(_) => "meta property",
        _ => "expression",
    }
}

/// Collects `var` names declared anywhere in a function body, without entering nested functions.
struct VarCollector<'n> {
    names: &'n mut Vec<String>,
}

impl Visit for VarCollector<'_> {
    fn visit_var_decl(&mut self, n: &VarDecl) {
        if n.kind == VarDeclKind::Var {
            for decl in &n.decls {
                for ident in pat_idents(&decl.name) {
                    self.names.push(ident.sym.to_string());
                }
            }
        }
        n.visit_children_with(self);
    }
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Whether the function named `name` could be seen by anything other than a direct call
/// `name(...)` within `stmts`. Only directly-called functions can skip the host wrapper:
/// any other read (`g = f`, `arr.map(f)`, `window.x = f`, `typeof f`, `new f`, `{ f }`)
/// lets the value leave VM control. Shadowed names in nested scopes are counted too, which
/// can only make the answer more cautious.
fn fn_escapes(name: &str, stmts: &[&Stmt]) -> bool {
    let mut scan = EscapeScan {
        name,
        escapes: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.escapes
}

fn fn_escapes_in_function(name: &str, function: &Function) -> bool {
    let mut scan = EscapeScan {
        name,
        escapes: false,
    };
    function.visit_with(&mut scan);
    scan.escapes
}

struct EscapeScan<'n> {
    name: &'n str,
    escapes: bool,
}

impl EscapeScan<'_> {
    fn is_name(&self, expr: &Expr) -> bool {
        let mut expr = expr;
        while let Expr::Paren(paren) = expr {
            expr = &paren.expr;
        }
        matches!(expr, Expr::Ident(ident) if *ident.sym == *self.name)
    }
}

impl Visit for EscapeScan<'_> {
    fn visit_call_expr(&mut self, n: &CallExpr) {
        match &n.callee {
            // a direct call doesn't let the function out; its arguments still might
            Callee::Expr(callee) if self.is_name(callee) => n.args.visit_with(self),
            _ => n.visit_children_with(self),
        }
    }
    fn visit_expr(&mut self, n: &Expr) {
        if self.is_name(n) {
            self.escapes = true;
        }
        n.visit_children_with(self);
    }
    fn visit_prop(&mut self, n: &Prop) {
        if let Prop::Shorthand(ident) = n {
            if *ident.sym == *self.name {
                self.escapes = true;
            }
        }
        n.visit_children_with(self);
    }
}

/// Names used anywhere inside functions nested in `stmts` (not counting `stmts`' own code).
/// A local with one of these names may be captured, so it's kept in a box. Matching by
/// name alone ignores shadowing, which can only box more variables than needed.
/// `params` count too: a default value can hold a closure.
fn captured_names(params: &[&Pat], body: &FnBody, stmts: &[&Stmt]) -> HashSet<String> {
    let mut scan = CaptureScan {
        names: HashSet::new(),
        fn_depth: 0,
    };
    for param in params {
        param.visit_with(&mut scan);
    }
    if let FnBody::Expr(expr) | FnBody::Field(expr, _) = body {
        expr.visit_with(&mut scan);
    }
    if let FnBody::InstanceFields(fields) = body {
        for value in fields.iter().filter_map(|f| f.value) {
            value.visit_with(&mut scan);
        }
    }
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.names
}

struct CaptureScan {
    names: HashSet<String>,
    /// how many nested functions deep the visitor is
    fn_depth: usize,
}

impl Visit for CaptureScan {
    fn visit_function(&mut self, n: &Function) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_arrow_expr(&mut self, n: &ArrowExpr) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_constructor(&mut self, n: &Constructor) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    fn visit_static_block(&mut self, n: &StaticBlock) {
        self.fn_depth += 1;
        n.visit_children_with(self);
        self.fn_depth -= 1;
    }
    // a field's key is evaluated in the enclosing scope, its value in its own function
    fn visit_class_prop(&mut self, n: &ClassProp) {
        n.key.visit_with(self);
        self.fn_depth += 1;
        n.value.visit_with(self);
        self.fn_depth -= 1;
    }
    fn visit_private_prop(&mut self, n: &PrivateProp) {
        self.fn_depth += 1;
        n.value.visit_with(self);
        self.fn_depth -= 1;
    }
    // private names live in hidden locals named `#x`
    fn visit_private_name(&mut self, n: &PrivateName) {
        if self.fn_depth > 0 {
            self.names.insert(private_key(n));
        }
    }
    fn visit_ident(&mut self, n: &Ident) {
        if self.fn_depth > 0 {
            self.names.insert(n.sym.to_string());
        }
    }
}

/// The hidden local name for a private name: `#x` (can't clash with an identifier).
fn private_key(name: &PrivateName) -> String {
    format!("#{}", name.name)
}

/// The private names a class body declares, in source order, with what each one is.
/// A getter and setter pair share one name.
fn class_private_names(class: &Class) -> CompileResult<Vec<(String, PrivateKind)>> {
    let mut names: Vec<(String, PrivateKind, bool)> = Vec::new();
    for member in &class.body {
        let (name, kind, is_static) = match member {
            ClassMember::PrivateProp(p) => (&p.key, PrivateKind::Field, p.is_static),
            ClassMember::PrivateMethod(m) if m.kind == MethodKind::Method => {
                (&m.key, PrivateKind::Method, m.is_static)
            }
            ClassMember::PrivateMethod(m) => (&m.key, PrivateKind::Accessor, m.is_static),
            _ => continue,
        };
        let key = private_key(name);
        match names.iter().find(|(k, ..)| *k == key) {
            None => names.push((key, kind, is_static)),
            // the other half of an accessor pair
            Some((_, PrivateKind::Accessor, s))
                if kind == PrivateKind::Accessor && *s == is_static => {}
            Some(_) => return unsupported(format!("duplicate private name `{key}`"), name.span),
        }
    }
    Ok(names.into_iter().map(|(key, kind, _)| (key, kind)).collect())
}

/// Whether private method `name` is used other than by calling it (`x.#m(...)`) in the
/// class body. Private names can only be used inside the class body, so this is exact
/// (an inner class reusing the name only makes it more cautious).
fn private_method_escapes(name: &PrivateName, class: &Class) -> bool {
    let mut scan = PrivateEscapeScan {
        name: &name.name,
        escapes: false,
    };
    class.visit_with(&mut scan);
    scan.escapes
}

struct PrivateEscapeScan<'n> {
    name: &'n str,
    escapes: bool,
}

impl PrivateEscapeScan<'_> {
    fn is_name(&self, prop: &MemberProp) -> bool {
        matches!(prop, MemberProp::PrivateName(p) if *p.name == *self.name)
    }
}

impl Visit for PrivateEscapeScan<'_> {
    fn visit_call_expr(&mut self, n: &CallExpr) {
        let mut callee = match &n.callee {
            Callee::Expr(callee) => &**callee,
            _ => return n.visit_children_with(self),
        };
        while let Expr::Paren(paren) = callee {
            callee = &paren.expr;
        }
        match callee {
            // a method call doesn't let the function out
            Expr::Member(member) if self.is_name(&member.prop) => {
                member.obj.visit_with(self);
                n.args.visit_with(self);
            }
            _ => n.visit_children_with(self),
        }
    }
    fn visit_member_expr(&mut self, n: &MemberExpr) {
        if self.is_name(&n.prop) {
            self.escapes = true;
        }
        n.visit_children_with(self);
    }
}

/// The names a binding pattern declares, in source order.
fn pat_idents(pat: &Pat) -> Vec<&Ident> {
    fn walk<'p>(pat: &'p Pat, out: &mut Vec<&'p Ident>) {
        match pat {
            Pat::Ident(ident) => out.push(&ident.id),
            Pat::Array(array_pat) => {
                for elem in array_pat.elems.iter().flatten() {
                    walk(elem, out);
                }
            }
            Pat::Object(object_pat) => {
                for prop in &object_pat.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => walk(&key_value.value, out),
                        ObjectPatProp::Assign(assign) => out.push(&assign.key.id),
                        ObjectPatProp::Rest(rest) => walk(&rest.arg, out),
                    }
                }
            }
            Pat::Assign(assign) => walk(&assign.left, out),
            Pat::Rest(rest) => walk(&rest.arg, out),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }
    let mut out = Vec::new();
    walk(pat, &mut out);
    out
}

/// A property key known at compile time, as a function name (`None` for computed keys and
/// numbers that need JS formatting).
fn static_key_name(key: &PropName) -> Option<String> {
    match key {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(s) => Some(String::from_utf8_lossy(s.value.as_bytes()).into_owned()),
        PropName::Num(n) if n.value.fract() == 0.0 && n.value < 1e21 => {
            Some(format!("{}", n.value as u64))
        }
        _ => None,
    }
}

/// A non-computed `__proto__` key.
fn is_proto_key(key: &PropName) -> bool {
    match key {
        PropName::Ident(ident) => &*ident.sym == "__proto__",
        PropName::Str(s) => s.value.as_bytes() == b"__proto__",
        _ => false,
    }
}

/// An unnamed function or arrow expression, which takes its name from where it's stored.
fn is_anonymous_fn(expr: &Expr) -> bool {
    let mut expr = expr;
    while let Expr::Paren(paren) = expr {
        expr = &paren.expr;
    }
    match expr {
        Expr::Fn(f) => f.ident.is_none(),
        Expr::Class(c) => c.ident.is_none(),
        Expr::Arrow(_) => true,
        _ => false,
    }
}
