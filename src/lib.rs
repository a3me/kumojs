pub mod disasm;
pub mod embed;
pub mod opcode;

// The compiler's methods are split across these modules by area. The `Compiler` struct and
// all the shared types stay in this root module, so the submodules (its descendants) can
// reach their private fields; the submodules hold `impl Compiler` blocks and free helpers.
mod analysis;
mod compile_class;
mod compile_expr;
mod compile_fn;
mod compile_pattern;
mod compile_scope;
mod compile_stmt;
mod emit;
mod error;

// Free helpers the compile modules call, re-exported so each can pull them in with `use
// crate::*` instead of repeating a long import list.
pub(crate) use analysis::*;
pub(crate) use error::*;

// Shared std / swc imports, re-exported for the same reason.
pub(crate) use std::cell::{Cell, RefCell};
pub(crate) use std::collections::HashSet;
pub(crate) use swc_common::sync::Lrc;
pub(crate) use swc_common::{BytePos, SourceFile, SourceMap, Span, Spanned};
#[rustfmt::skip]
pub(crate) use swc_ecma_ast::{
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
pub(crate) use swc_ecma_visit::{Visit, VisitWith};

use std::path::Path;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
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
    pub(crate) fn of(is_generator: bool, is_async: bool) -> Self {
        match (is_generator, is_async) {
            (false, false) => Coroutine::None,
            (true, false) => Coroutine::Generator,
            (false, true) => Coroutine::Async,
            (true, true) => Coroutine::AsyncGenerator,
        }
    }
    pub(crate) fn is_generator(self) -> bool {
        matches!(self, Coroutine::Generator | Coroutine::AsyncGenerator)
    }
    pub(crate) fn is_async(self) -> bool {
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
    pub(crate) fn from_name(name: &str) -> Option<Self> {
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

    pub(crate) fn new_enclosing(enclosing: &'a Compiler<'a>) -> Compiler<'a> {
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

    pub(crate) fn parse_and_compile(
        &mut self,
        cm: &SourceMap,
        fm: &SourceFile,
    ) -> CompileResult<Vec<u8>> {
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
}

impl Default for Compiler<'_> {
    fn default() -> Self {
        Self::new()
    }
}
