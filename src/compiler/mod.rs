//! The compiler: JavaScript source to kumo bytecode.
//!
//! `Compiler` is the public entry point and only holds options. Each function body
//! (including the script itself) is compiled by its own `FnCompiler`, whose methods are
//! split across the submodules by area. The submodules are children of this one, so they
//! can reach `FnCompiler`'s private fields.

mod analysis;
mod call;
mod class;
mod emit;
mod expr;
mod func;
mod literal;
mod pattern;
mod scope;
mod stmt;

/// Everything the submodules share, so each can start with `use super::prelude::*`.
mod prelude {
    pub(super) use super::analysis::*;
    pub(super) use super::emit::load_str;
    pub(super) use super::{
        Access, Bind, BodyKind, Breakable, BreakableKind, Exit, FieldInit, FnBody, FnCompiler,
        FnOptions, Local, Tdz, TdzUse, TryRegion, VarRef,
    };
    pub(super) use crate::error::{syntax_error, unsupported, CompileError, CompileResult};
    pub(super) use crate::opcode::*;
    pub(super) use std::cell::{Cell, RefCell};
    pub(super) use std::collections::HashSet;
    pub(super) use swc_common::{BytePos, Span, Spanned};
    #[rustfmt::skip]
    pub(super) use swc_ecma_ast::{
        ArrayLit, ArrayPat, ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget,
        AssignTargetPat, BinExpr, BinaryOp, BlockStmt, CallExpr, Callee, Class, ClassMember,
        ClassProp, CondExpr, Constructor, Decl, DoWhileStmt, Expr, ExprOrSpread, FnDecl, FnExpr,
        ForHead, ForInStmt, ForOfStmt, ForStmt, Function, Ident, IfStmt, LabeledStmt, Lit,
        MemberExpr, MemberProp, MetaPropKind, MethodKind, Module, ModuleItem, NewExpr, ObjectLit,
        ObjectPat, ObjectPatProp, OptChainBase, OptChainExpr, ParamOrTsParamProp, Pat,
        PrivateName, PrivateProp, Prop, PropName, PropOrSpread, SimpleAssignTarget, StaticBlock,
        Stmt, SuperProp, SuperPropExpr, SwitchStmt, TaggedTpl, Tpl, TryStmt, UnaryExpr, UnaryOp,
        UpdateExpr, UpdateOp, VarDecl, VarDeclKind, VarDeclOrExpr, VarDeclarator, WhileStmt,
    };
    pub(super) use swc_ecma_visit::{Visit, VisitWith};
}

use crate::error::Location;
use prelude::*;
use std::path::Path;
use swc_common::sync::Lrc;
use swc_common::{SourceFile, SourceMap};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};

/// Compiles scripts to kumo bytecode.
pub struct Compiler {
    /// how emitted opcode bytes are renumbered (identity unless a build shuffles them)
    opcode_map: OpcodeMap,
}

impl Compiler {
    pub fn new() -> Self {
        Compiler {
            opcode_map: OpcodeMap::identity(),
        }
    }

    /// Emit opcodes renumbered by `map` (the VM running the bytecode must match it).
    pub fn with_opcode_map(mut self, map: OpcodeMap) -> Self {
        self.opcode_map = map;
        self
    }

    /// The `OpcodeMap` this compiler emits under.
    pub fn opcode_map(&self) -> &OpcodeMap {
        &self.opcode_map
    }

    pub fn compile_file(&self, path: &Path) -> Result<Vec<u8>, CompileError> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm
            .load_file(path)
            .map_err(|e| CompileError::LoadFile(e.to_string()))?;
        self.parse_and_compile(&cm, &fm)
    }

    /// Compile JS source held in memory. `name` is used in error locations.
    pub fn compile_source(&self, name: &str, source: &str) -> Result<Vec<u8>, CompileError> {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            swc_common::FileName::Custom(name.to_string()).into(),
            source.to_string(),
        );
        self.parse_and_compile(&cm, &fm)
    }

    /// Compile a parsed module. Errors carry spans but no resolved locations.
    pub fn compile(&self, module: &Module) -> Result<Vec<u8>, CompileError> {
        let mut c = FnCompiler::new(&self.opcode_map);
        c.compile_module(module)?;
        Ok(c.bytecode)
    }

    fn parse_and_compile(&self, cm: &SourceMap, fm: &SourceFile) -> Result<Vec<u8>, CompileError> {
        let lexer = Lexer::new(
            Syntax::Es(Default::default()),
            Default::default(),
            StringInput::from(fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);

        // parsed as a module, so input is always strict mode
        let parse_error = |e: swc_ecma_parser::error::Error| CompileError::Parse {
            message: e.kind().msg().to_string(),
            location: Some(Location::of(cm, e.span())),
        };
        let module = parser.parse_module().map_err(parse_error)?;
        if let Some(e) = parser.take_errors().into_iter().next() {
            return Err(parse_error(e));
        }
        self.compile(&module).map_err(|e| e.locate(cm))
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiles one function body (or the script) to bytecode.
struct FnCompiler<'a> {
    bytecode: Vec<u8>,
    /// how emitted opcode bytes are renumbered; shared by every nested function so one
    /// script's bytecode is internally consistent
    opcode_map: &'a OpcodeMap,
    enclosing: Option<&'a FnCompiler<'a>>,
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
    /// names used anywhere inside functions nested in this one; locals with these names
    /// are kept in boxes so closures can share them (see `captured_names`)
    captured_names: HashSet<String>,
    /// variables this function captures, in capture-index order
    upvalues: RefCell<Vec<Capture>>,
    /// compiling an arrow function: `this` and `arguments` come from the enclosing function
    is_arrow: bool,
    /// compiling an async function (`await` suspends it)
    is_async: bool,
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
}

impl<'a> FnCompiler<'a> {
    fn new(opcode_map: &'a OpcodeMap) -> Self {
        FnCompiler {
            bytecode: Vec::new(),
            opcode_map,
            enclosing: None,
            locals: Vec::new(),
            slot_count: 0,
            current_scope_depth: 0,
            breakables: Vec::new(),
            try_stack: Vec::new(),
            pending_labels: Vec::new(),
            temp_slot: None,
            captured_names: HashSet::new(),
            upvalues: RefCell::new(Vec::new()),
            is_arrow: false,
            is_async: false,
            created_at: BytePos(0),
            next_anchor: None,
            in_switch: false,
            hoisting_fn: false,
            class_anchor: None,
        }
    }

    /// A compiler for a function nested in `enclosing`.
    fn new_enclosing(enclosing: &'a FnCompiler<'a>) -> Self {
        FnCompiler {
            enclosing: Some(enclosing),
            ..FnCompiler::new(enclosing.opcode_map)
        }
    }

    fn compile_module(&mut self, module: &Module) -> CompileResult {
        let mut stmts = Vec::with_capacity(module.body.len());
        for item in &module.body {
            match item {
                ModuleItem::Stmt(stmt) => stmts.push(stmt),
                ModuleItem::ModuleDecl(decl) => return unsupported("import/export", decl.span()),
            }
        }
        // the module body behaves like a function body
        self.captured_names = captured_names(&[], &FnBody::Block(&[]), &stmts);
        self.compile_body(&stmts, BodyKind::Module)?;
        self.patch_tdz(0);
        Ok(())
    }
}

// -------------------------------------------------------------------------------------
// shared types
// -------------------------------------------------------------------------------------

/// Which kind of statement list `compile_body` compiles.
#[derive(Clone, Copy, PartialEq)]
enum BodyKind {
    /// the script: hoists `var`s, and leaves the last expression statement's value on the
    /// stack (the test runner reads it)
    Module,
    /// a function body: hoists `var`s
    Function,
    /// a block or `switch` body: only its own `let` / `const` / functions / classes
    Block,
}

/// Whether storing into a pattern declares its names or assigns to them.
#[derive(Clone, Copy, PartialEq)]
enum Bind {
    /// initialise declared names (so a `const` can be set)
    Init,
    /// an assignment expression
    Assign,
}

/// `break` or `continue`.
#[derive(Clone, Copy, PartialEq)]
enum Exit {
    Break,
    Continue,
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

/// How a member expression's key is read and written.
#[derive(Clone, Copy)]
enum Access {
    /// `obj.x` / `obj[x]`: the key is on the stack
    Property,
    /// `obj.#x`: the private name object is on the stack
    Private(PrivateKind),
}

/// Where a variable lives, from the point of view of the function using it.
#[derive(Clone, Copy)]
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
    /// exit
    close_iterator: Option<u8>,
    /// scope at the `try` statement, so the inlined `finally` resolves names correctly
    locals_len: usize,
    scope_depth: usize,
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

/// How a closure is made, besides its parameters and body.
#[derive(Clone, Copy)]
struct FnOptions<'n> {
    /// its `name` property
    name: Option<&'n str>,
    /// a named function expression's own name, bound (read-only) inside its body
    self_name: Option<&'n Ident>,
    kind: FnKind,
    is_async: bool,
    /// only ever called directly from VM code, so it needs no host wrapper
    internal: bool,
}

impl FnOptions<'_> {
    fn new(kind: FnKind) -> Self {
        FnOptions {
            name: None,
            self_name: None,
            kind,
            is_async: false,
            internal: false,
        }
    }
}
