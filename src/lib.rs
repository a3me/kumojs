//! kumojs: compiles JavaScript to bytecode for the kumo VM.

mod compiler;
pub mod disasm;
mod error;
pub mod opcode;

pub use compiler::Compiler;
pub use error::{CompileError, Location};
