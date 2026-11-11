//! The bytecode format: every `Operation`, its opcode byte and operand layout, and the
//! small enums its operands are made of.
//!
//! The `opcodes!` table lists every opcode once and generates `Operation::get_opcode`, the
//! `OPCODES` metadata table and the `op` constants. The `match` in `get_opcode` is
//! exhaustive, so a new `Operation` variant won't compile until it's listed, and the
//! compiler, the disassembler and the VM can't drift apart.
//!
//! `OpcodeMap` renumbers opcodes for a build: the identity map keeps the canonical bytes
//! (so snapshots stay stable), while a seeded map permutes them.

// -------------------------------------------------------------------------------------
// operations
// -------------------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum Operation {
    // ---- constants
    /// "kumo" (WTF-8 bytes, so lone surrogates survive)
    LoadString(Vec<u8>),
    LoadFloat64(f64),
    /// a BigInt literal, as its decimal digits
    LoadBigInt(String),
    #[allow(dead_code)]
    UInt8(u8),
    #[allow(dead_code)]
    UInt16(u16),
    #[allow(dead_code)]
    UInt32(u32),
    UInt64(u64),
    Bool(bool),
    Null,
    Undefined,
    /// /exp/flags
    Regex(String, String),

    // ---- stack
    Pop,
    /// [a] -> [a, a]
    Dup,
    /// [a, b] -> [a, b, a, b]
    Dup2,

    // ---- operators
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
    /// unary `-`
    UnaryMinus,
    /// unary `+`
    UnaryPlus,
    /// `!`
    LogicalNot,
    /// `~`
    BitwiseNot,
    /// `typeof`
    TypeOf,
    /// `void`
    Void,
    /// `x + 1` after converting x to a number (a BigInt steps by `1n`)
    Increment,
    /// `x - 1` after converting x to a number (a BigInt steps by `1n`)
    Decrement,
    /// convert to a number, keeping a BigInt as it is (the old value of `x++`)
    ToNumeric,

    // ---- properties
    /// [obj, key] -> [obj[key]]
    GetProperty,
    /// [obj, key, value] -> [value]
    SetProperty,
    /// [obj, key] -> [bool]: `delete obj[key]`
    Delete,
    /// [key] -> [property key]: a string or symbol, converted the way JS does
    ToPropertyKey,

    // ---- variables
    /// Get the local slot n
    GetLocal(u8),
    /// Set the local slot n, leaving the value on the stack
    SetLocal(u8),
    /// Get a captured variable of the running closure: capture index
    GetUpvalue(u8),
    /// Set a captured variable of the running closure: capture index
    SetUpvalue(u8),
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
    /// The host's `window` / `globalThis`. With `true`, push `undefined` instead of
    /// throwing when it's missing (`typeof window`).
    HostGlobal(HostGlobal, bool),
    /// `this`
    This,
    /// The currently running function (for named function expressions)
    Callee,
    /// `arguments`
    Arguments,
    /// `new.target`
    NewTarget,

    // ---- control flow
    /// Jump forward by offset
    Jump(u32),
    /// Jump forward by offset if top of stack is falsy (does not pop)
    JumpIfFalse(u32),
    /// Jump forward by offset if top of stack is truthy (does not pop)
    JumpIfTrue(u32),
    /// Jump forward by offset if top of stack is not null/undefined (does not pop)
    JumpIfNotNullish(u32),
    /// Jump forward by offset if top of stack is null/undefined (does not pop)
    JumpIfNullish(u32),
    /// Jump backward by offset
    Loop(u32),
    /// `throw value`
    Throw,
    /// Install an exception handler at a forward offset; the thrown value is pushed there
    TryStart(u32),
    /// Remove the innermost exception handler
    TryEnd,
    /// [value] -> suspend until the value settles, then push the result. A rejection
    /// resumes at the instruction after (a `Throw`), a fulfilment after that.
    Await,

    // ---- functions and calls
    /// Create a closure (see `FnInfo`)
    Function(FnInfo),
    /// [f, ...args, argc] -> [result]
    Call,
    /// [obj, f, ...args, argc] -> [result]: calls f with obj as `this`
    CallMethod,
    /// `new f(...args)`: stack is [f, ...args, argc]
    New,
    Return,
    /// [args array] -> [...args, argc] for a call with spread arguments
    SpreadArgs,
    /// Push an array of the call's arguments from this index on (rest parameter)
    RestArgs(u8),
    /// [key, fn] (not popped): name fn after a computed key, with a `get ` / `set ` prefix
    SetFunctionName(MethodFlavor),

    // ---- arrays, objects and templates
    /// Create array with n elements
    Array(u32),
    /// An array literal hole (`[1, , 3]`)
    Hole,
    /// [arr, value] -> [arr] with value appended (a `Hole` just extends the length)
    ArrayAppend,
    /// [arr, iterable] -> [arr] with every iterated value appended (`...x`)
    ArraySpread,
    /// Create object with n properties
    Object(u32),
    /// [obj, key, value] -> [obj] with an own data property (object literal)
    DefineField,
    /// [obj, key, fn] -> [obj] with fn as the property's getter or setter
    DefineAccessor(Accessor),
    /// [obj, source] -> [obj] with source's own enumerable properties copied (`...source`)
    ObjectSpread,
    /// Join n parts (strings and values, alternating, string first) as a template does
    Template(u32),
    /// Push the strings array for a tagged template: (cooked, raw) per piece, cooked
    /// `None` for an invalid escape. The same array is reused each time this site runs.
    TemplateObject(Vec<(Option<Vec<u8>>, Vec<u8>)>),

    // ---- iteration and destructuring
    /// Replace an object with an array of its `for-in` keys
    ForInKeys,
    /// [iterable] -> [iterator record] (`for-of`)
    GetIterator,
    /// Step the iterator record in a slot: push its next value, or jump forward by the
    /// offset when it's done
    IterNext(u8, u32),
    /// Close the iterator record in a slot (call its `return()`) unless it's done
    IterClose(u8, IterExit),
    /// Replace an iterable with an array of its first n values (`ITER_TAKE_ALL`: every
    /// value), closing the iterator if it isn't done; arrays are used as they are
    IterTake(u32),
    /// Replace an array with its elements from an index on (array pattern rest)
    ArraySliceFrom(u32),
    /// Throw a `TypeError` if the top of the stack is `null` / `undefined` (does not pop)
    RequireObjectCoercible,
    /// [obj, keys] -> [copy of obj's own enumerable properties not in keys]
    ObjectRest,

    // ---- classes
    /// [superclass if derived, constructor] -> [class]: link prototypes, lock `prototype`
    MakeClass(bool),
    /// [target, key, fn] -> []: a non-enumerable method, getter or setter
    DefineMethod(MethodFlavor),
    /// [class, key] -> []: an instance field's key (in order); `ClassInit` sets up values
    ClassField,
    /// [class, key, init or undefined] -> []: define a static field now
    StaticField,
    /// [class, fn] -> []: run a static block with `this` as the class
    StaticBlock,
    /// [class, fn] -> []: the class's instance field method
    ClassInit,
    /// [] -> [this]: set up a new instance's private methods and fields (start of a base
    /// class constructor); the fields' method runs in this loop and gives back `this`
    InitInstance,
    /// [value] -> []: define instance field n (its key is from the class) on `this`; `true`
    /// names an anonymous function value after a computed key
    InitField(u32, bool),
    /// [...args, argc] -> [parent constructor's result]: `super(...)`
    SuperCall,
    /// [parent constructor's result] -> [this]: bind `this` after `super(...)`, then set up
    /// the instance like `InitInstance`
    SuperCallFinish,
    /// [key] -> [value]: `super[key]`
    SuperGet,
    /// [key, value] -> [value]: `super[key] = value`
    SuperSet,
    /// Push a new private name for a class body
    NewPrivateName(PrivateKind),
    /// [obj, name] -> [value]: `obj.#x` (a private getter is called)
    PrivateGet,
    /// [obj, name, value] -> [value]: `obj.#x = value`. A private setter is called with
    /// the value left under its result: [value, result], so the compiler pops after it.
    PrivateSet,
    /// [name, obj] -> [bool]: `#x in obj`
    PrivateIn,
    /// [class, name, fn] -> []: a private method, getter or setter (`true`: static)
    DefinePrivateMethod(MethodFlavor, bool),
}

/// `IterTake` count meaning "take every value".
pub(crate) const ITER_TAKE_ALL: u32 = u32::MAX;

/// A closure to create. An `internal` function is only ever called directly from VM code,
/// so it gets no host wrapper.
#[derive(Debug)]
pub(crate) struct FnInfo {
    /// the function's `length`: parameters before the first default or rest
    pub length: u8,
    /// parameter slots filled from the call's arguments
    pub params: u8,
    pub kind: FnKind,
    pub is_async: bool,
    pub internal: bool,
    pub name: String,
    pub captures: Vec<Capture>,
    pub code: Vec<u8>,
}

/// `FnInfo` flags byte: the `FnKind` in the low bits, plus this bit for async functions.
pub(crate) const FN_ASYNC: u8 = 0b1000;
pub(crate) const FN_KIND_MASK: u8 = 0b0111;

/// Arrows take `this` and `arguments` from where they're created; arrows and methods
/// (object literal methods, getters, setters) can't be used with `new`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FnKind {
    Normal = 0,
    Arrow = 1,
    Method = 2,
    /// class constructors: only callable with `new`
    BaseClass = 3,
    /// a derived class's `this` comes from its `super(...)` call
    DerivedClass = 4,
}

impl FnKind {
    /// How the disassembler shows a function kind byte.
    pub(crate) fn label(byte: u8) -> &'static str {
        ["normal", "arrow", "method", "class", "derived class"]
            .get(byte as usize)
            .copied()
            .unwrap_or("?")
    }
}

/// A variable a closure captures, from the enclosing function's point of view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Capture {
    /// a boxed local slot of the enclosing function
    Local(u8),
    /// one of the enclosing function's own captures
    Upvalue(u8),
}

/// Undeclared names that resolve to the host environment. Everything else must be declared.
#[derive(Debug, Clone, Copy)]
pub(crate) enum HostGlobal {
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

/// `HostGlobal` operand bit: push `undefined` instead of throwing when missing.
pub(crate) const HOST_GLOBAL_TYPEOF: u8 = 0b10;

/// What a private name (`#x`) declares. Known at compile time because private names are
/// always declared in an enclosing class body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PrivateKind {
    Field = 0,
    Method = 1,
    /// a getter and / or setter
    Accessor = 2,
}

/// A plain method, a getter or a setter (`DefineMethod`, `DefinePrivateMethod`, and the
/// name prefix of `SetFunctionName`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MethodFlavor {
    Method = 0,
    Getter = 1,
    Setter = 2,
}

/// `DefinePrivateMethod` operand bit for a static method.
pub(crate) const PRIVATE_METHOD_STATIC: u8 = 0b100;

/// Which half of an object literal accessor `DefineAccessor` defines.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Accessor {
    Getter = 0,
    Setter = 1,
}

/// How a loop is left when `IterClose` closes its iterator.
#[derive(Debug, Clone, Copy)]
pub(crate) enum IterExit {
    /// `break` / `return`: errors from `return()` propagate
    Normal = 0,
    /// a throw: errors from `return()` are ignored, the original throw goes on
    Throw = 1,
}

// -------------------------------------------------------------------------------------
// the opcode table
// -------------------------------------------------------------------------------------

/// How an instruction's operand bytes are laid out (for the disassembler).
#[derive(Clone, Copy)]
pub enum Operand {
    U8,
    U16,
    U32,
    F64,
    /// u32 byte length, then WTF-8 bytes
    Str,
    /// u32 forward offset from the end of this operand
    Jump,
    /// u32 backward offset (`Loop`): the target is this operand's end minus it
    Back,
}

/// An opcode's canonical byte, name and operand layout.
pub struct OpInfo {
    pub code: u8,
    pub name: &'static str,
    pub operands: &'static [Operand],
}

/// Define every opcode once. Rows are
/// `Variant[(pattern)] [as Name] [if guard] => 0xNN, [operands]`; `Name` (default: the
/// variant) names the opcode in `OPCODES` and `op`.
macro_rules! opcodes {
    ( $( $variant:ident $( ( $($pat:tt)* ) )? $( as $alias:ident )? $( if $guard:expr )?
         => $code:literal , [ $( $op:ident ),* ] );+ $(;)? ) => {
        impl Operation {
            /// The canonical byte for this operation (before any `OpcodeMap` permutation).
            pub(crate) fn get_opcode(&self) -> u8 {
                match self {
                    $( Operation::$variant $( ( $($pat)* ) )? $( if $guard )? => $code, )+
                }
            }
        }

        /// Every opcode's canonical byte, name and operand layout.
        pub const OPCODES: &[OpInfo] = &[
            $( OpInfo {
                code: $code,
                name: opcode_name!($variant $($alias)?),
                operands: &[ $( Operand::$op ),* ],
            }, )+
        ];

        /// Every opcode's canonical byte, by name.
        #[allow(non_upper_case_globals)]
        pub mod op {
            $( opcode_const!($code, $variant $($alias)?); )+
        }
    };
}

macro_rules! opcode_name {
    ($variant:ident) => { stringify!($variant) };
    ($variant:ident $alias:ident) => { stringify!($alias) };
}

macro_rules! opcode_const {
    ($code:literal, $variant:ident) => { pub const $variant: u8 = $code; };
    ($code:literal, $variant:ident $alias:ident) => { pub const $alias: u8 = $code; };
}

#[rustfmt::skip]
opcodes! {
    LoadString(..)                               => 0x01, [Str];
    LoadFloat64(..)                              => 0x02, [F64];
    Bool(..)                                     => 0x03, [U8];
    Pop                                          => 0x04, [];
    Null                                         => 0x05, [];
    Regex(..)                                    => 0x06, [Str, Str];
    Undefined                                    => 0x07, [];
    Return                                       => 0x08, [];
    UInt8(..)                                    => 0x0b, [U8];
    UInt16(..)                                   => 0x0c, [U16];
    UInt32(..)                                   => 0x0d, [U32];
    UInt64(..)                                   => 0x0e, [U32, U32];
    Call                                         => 0x0f, [];
    GetProperty                                  => 0x10, [];
    SetProperty                                  => 0x11, [];
    EqEq                                         => 0x12, [];
    NotEq                                        => 0x13, [];
    EqEqEq                                       => 0x14, [];
    NotEqEq                                      => 0x15, [];
    Lt                                           => 0x16, [];
    LtEq                                         => 0x17, [];
    Gt                                           => 0x18, [];
    GtEq                                         => 0x19, [];
    LShift                                       => 0x1a, [];
    RShift                                       => 0x1b, [];
    ZeroFillRShift                               => 0x1c, [];
    Add                                          => 0x1d, [];
    Sub                                          => 0x1e, [];
    Mul                                          => 0x1f, [];
    Div                                          => 0x20, [];
    Mod                                          => 0x21, [];
    BitOr                                        => 0x22, [];
    BitXor                                       => 0x23, [];
    BitAnd                                       => 0x24, [];
    In                                           => 0x27, [];
    InstanceOf                                   => 0x28, [];
    Exp                                          => 0x29, [];
    GetLocal(..)                                 => 0x2b, [U8];
    SetLocal(..)                                 => 0x2c, [U8];
    UnaryMinus                                   => 0x2d, [];
    UnaryPlus                                    => 0x2e, [];
    LogicalNot                                   => 0x2f, [];
    BitwiseNot                                   => 0x30, [];
    TypeOf                                       => 0x31, [];
    Void                                         => 0x32, [];
    Delete                                       => 0x33, [];
    Array(..)                                    => 0x34, [U32];
    Object(..)                                   => 0x35, [U32];
    Dup                                          => 0x36, [];
    // (the guarded row must come first: rows match in order)
    Function(info) as InternalFunction if info.internal => 0x4b, [];
    Function(..)                                 => 0x37, [];
    GetUpvalue(..)                               => 0x38, [U8];
    SetUpvalue(..)                               => 0x39, [U8];
    Jump(..)                                     => 0x3a, [Jump];
    JumpIfFalse(..)                              => 0x3b, [Jump];
    JumpIfTrue(..)                               => 0x3c, [Jump];
    JumpIfNotNullish(..)                         => 0x3d, [Jump];
    CallMethod                                   => 0x3e, [];
    This                                         => 0x3f, [];
    HostGlobal(..)                               => 0x40, [U8];
    Loop(..)                                     => 0x41, [Back];
    Dup2                                         => 0x42, [];
    Callee                                       => 0x43, [];
    Arguments                                    => 0x44, [];
    New                                          => 0x45, [];
    Throw                                        => 0x46, [];
    TryStart(..)                                 => 0x47, [Jump];
    TryEnd                                       => 0x48, [];
    ForInKeys                                    => 0x49, [];
    Hole                                         => 0x4a, [];
    GetBox(..)                                   => 0x4c, [U8];
    SetBox(..)                                   => 0x4d, [U8];
    NewBox(..)                                   => 0x4e, [U8];
    BoxLocal(..)                                 => 0x4f, [U8];
    RenewBox(..)                                 => 0x50, [U8];
    ArrayAppend                                  => 0x51, [];
    ArraySpread                                  => 0x52, [];
    SpreadArgs                                   => 0x53, [];
    RestArgs(..)                                 => 0x54, [U8];
    IterTake(..)                                 => 0x55, [U32];
    ArraySliceFrom(..)                           => 0x56, [U32];
    RequireObjectCoercible                       => 0x57, [];
    ObjectRest                                   => 0x58, [];
    Template(..)                                 => 0x59, [U32];
    TemplateObject(..)                           => 0x5a, [];
    DefineField                                  => 0x5b, [];
    DefineAccessor(..)                           => 0x5c, [U8];
    ObjectSpread                                 => 0x5d, [];
    SetFunctionName(..)                          => 0x5e, [U8];
    MakeClass(..)                                => 0x5f, [U8];
    DefineMethod(..)                             => 0x60, [U8];
    ClassField                                   => 0x61, [];
    StaticField                                  => 0x62, [];
    StaticBlock                                  => 0x63, [];
    SuperCall                                    => 0x64, [];
    SuperGet                                     => 0x65, [];
    SuperSet                                     => 0x66, [];
    NewTarget                                    => 0x67, [];
    NewPrivateName(..)                           => 0x68, [U8];
    PrivateGet                                   => 0x69, [];
    PrivateSet                                   => 0x6a, [];
    PrivateIn                                    => 0x6b, [];
    DefinePrivateMethod(..)                      => 0x6c, [U8];
    LoadBigInt(..)                               => 0x6d, [Str];
    Increment                                    => 0x6e, [];
    Decrement                                    => 0x6f, [];
    ToNumeric                                    => 0x70, [];
    GetIterator                                  => 0x71, [];
    IterNext(..)                                 => 0x72, [U8, Jump];
    IterClose(..)                                => 0x73, [U8, U8];
    JumpIfNullish(..)                            => 0x74, [Jump];
    Await                                        => 0x77, [];
    ToPropertyKey                                => 0x7f, [];
    InitInstance                                 => 0x80, [];
    SuperCallFinish                              => 0x81, [];
    InitField(..)                                => 0x82, [U32, U8];
    ClassInit                                    => 0x83, [];
    GetSlotIndex(..)                             => 0x84, [U8, U32];
    GetSlotProp(..)                              => 0x85, [U8, Str];
    NewBoxTdz(..)                                => 0x86, [U8];
    Tdz(..)                                      => 0x87, [U8];
    CheckTdz                                     => 0x88, [];
    ThrowTdz                                     => 0x89, [];
}

// -------------------------------------------------------------------------------------
// the VM's view: opcodes.d.ts and opcode maps
// -------------------------------------------------------------------------------------

/// `src/vm/opcodes.d.ts`: the canonical opcode bytes as the `Op` constants vm.ts uses.
/// The VM build (`tools/build.mjs`) replaces each `Op.Name` with its byte.
pub fn typescript_declarations() -> String {
    let mut out = String::from(
        "// Generated from the opcode table in src/opcode.rs by `kumoc --emit-opcodes`; don't edit.\n\
         // vm.ts refers to opcodes only as `Op.Name`. tools/build.mjs replaces each with its\n\
         // byte: the canonical one below, or a renumbered one for a shuffled build.\n\
         declare const Op: {\n",
    );
    let mut opcodes: Vec<&OpInfo> = OPCODES.iter().collect();
    opcodes.sort_by_key(|o| o.code);
    for o in opcodes {
        out.push_str(&format!("    readonly {}: 0x{:02x};\n", o.name, o.code));
    }
    out.push_str("};\n");
    out
}

/// A renumbering of the opcode bytes: `to_byte` is applied when emitting, `from_byte`
/// inverts it (for the disassembler). Only the bytes that are actual opcodes are permuted;
/// every other byte value maps to itself, so the map stays a bijection on 0..=255.
#[derive(Clone)]
pub struct OpcodeMap {
    pub to_byte: [u8; 256],
    pub from_byte: [u8; 256],
}

impl OpcodeMap {
    /// The canonical numbering: every byte maps to itself.
    pub fn identity() -> Self {
        let map: [u8; 256] = std::array::from_fn(|i| i as u8);
        OpcodeMap {
            to_byte: map,
            from_byte: map,
        }
    }

    /// A map written by `tools/build.mjs`: `{ "LoadString": 23, ... }`, giving every
    /// opcode's byte in that build. The bytes must be the opcode bytes, permuted.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let names: std::collections::HashMap<String, u8> =
            serde_json::from_str(json).map_err(|e| format!("opcode map: {e}"))?;
        let mut map = Self::identity();
        let mut used = [false; 256];
        for o in OPCODES {
            let byte = *names
                .get(o.name)
                .ok_or_else(|| format!("opcode map: no byte for `{}`", o.name))?;
            if OPCODES.iter().all(|other| other.code != byte) || used[byte as usize] {
                return Err(format!(
                    "opcode map: `{}` -> 0x{byte:02x} isn't a permutation of the opcode bytes",
                    o.name
                ));
            }
            used[byte as usize] = true;
            map.to_byte[o.code as usize] = byte;
            map.from_byte[byte as usize] = o.code;
        }
        if names.len() != OPCODES.len() {
            return Err("opcode map: unknown opcode names".into());
        }
        Ok(map)
    }

    pub fn is_identity(&self) -> bool {
        self.to_byte
            .iter()
            .enumerate()
            .all(|(i, b)| *b as usize == i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_bytes_and_names_are_distinct() {
        let mut seen = [false; 256];
        for o in OPCODES {
            assert!(!seen[o.code as usize], "duplicate opcode 0x{:02x}", o.code);
            seen[o.code as usize] = true;
        }
        let names: std::collections::HashSet<_> = OPCODES.iter().map(|o| o.name).collect();
        assert_eq!(names.len(), OPCODES.len(), "duplicate opcode name");
    }

    /// The checked-in declarations match the table. After changing it, run
    /// `cargo run --bin kumoc -- --emit-opcodes src/vm/opcodes.d.ts`.
    #[test]
    fn opcodes_d_ts_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/vm/opcodes.d.ts");
        let checked_in = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            checked_in == typescript_declarations(),
            "src/vm/opcodes.d.ts is stale: run `cargo run --bin kumoc -- --emit-opcodes src/vm/opcodes.d.ts`"
        );
    }

    fn json(pairs: impl Iterator<Item = (&'static str, u8)>) -> String {
        let fields: Vec<String> = pairs.map(|(n, b)| format!("\"{n}\": {b}")).collect();
        format!("{{{}}}", fields.join(", "))
    }

    #[test]
    fn map_from_json_inverts() {
        // rotate every opcode's byte to the next opcode's
        let mut codes: Vec<u8> = OPCODES.iter().map(|o| o.code).collect();
        codes.sort_unstable();
        let next = |c: u8| codes[(codes.iter().position(|&x| x == c).unwrap() + 1) % codes.len()];
        let map = OpcodeMap::from_json(&json(OPCODES.iter().map(|o| (o.name, next(o.code))))).unwrap();
        assert!(!map.is_identity());
        for b in 0..=255u8 {
            assert_eq!(map.from_byte[map.to_byte[b as usize] as usize], b);
        }
        let canonical = json(OPCODES.iter().map(|o| (o.name, o.code)));
        assert!(OpcodeMap::from_json(&canonical).unwrap().is_identity());
    }

    #[test]
    fn map_from_json_rejects_bad_maps() {
        let missing = json(OPCODES.iter().skip(1).map(|o| (o.name, o.code)));
        assert!(OpcodeMap::from_json(&missing).is_err());
        let duplicate = json(OPCODES.iter().map(|o| (o.name, OPCODES[0].code)));
        assert!(OpcodeMap::from_json(&duplicate).is_err());
        let not_an_opcode = json(OPCODES.iter().map(|o| (o.name, if o.code == 0x01 { 0xff } else { o.code })));
        assert!(OpcodeMap::from_json(&not_an_opcode).is_err());
    }
}
