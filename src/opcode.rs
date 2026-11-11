//! The one place opcodes are defined. The `opcodes!` macro lists every opcode once — its
//! `Operation` pattern, its byte, and its operand layout — and generates both
//! `Operation::get_opcode` and the `OPCODES` metadata table. The enum's exhaustiveness
//! check means a new `Operation` variant won't compile until it's listed here, so the
//! compiler, the disassembler and the generated VM can't drift apart.
//!
//! `OpcodeMap` lets a build renumber opcodes: the identity map keeps the canonical bytes
//! (so snapshots stay stable), while a seeded map permutes them, and the VM generator
//! rewrites the interpreter's `switch` to match.

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
    /// u32 forward offset from the end of the whole instruction
    JumpFromEnd,
}

/// An opcode's canonical byte, name and operand layout.
pub struct OpInfo {
    pub code: u8,
    pub name: &'static str,
    pub operands: &'static [Operand],
}

/// Define every opcode once. Generates `Operation::get_opcode` and `OPCODES`.
/// Rows are `Variant[(pattern)] [if guard] => 0xNN, [operands]`.
macro_rules! opcodes {
    ( $( $variant:ident $( ( $($pat:tt)* ) )? $( if $guard:expr )? => $code:literal , [ $( $op:ident ),* ] );+ $(;)? ) => {
        impl Operation {
            /// The canonical byte for this operation (before any `OpcodeMap` permutation).
            pub fn get_opcode(&self) -> u8 {
                match self {
                    $( Operation::$variant $( ( $($pat)* ) )? $( if $guard )? => $code, )+
                }
            }
        }

        /// Every opcode's canonical byte, name and operand layout (the single source of
        /// truth shared by the disassembler and the VM generator).
        pub const OPCODES: &[$crate::opcode::OpInfo] = &[
            $( $crate::opcode::OpInfo {
                code: $code,
                name: stringify!($variant),
                operands: &[ $( $crate::opcode::Operand::$op ),* ],
            }, )+
        ];
    };
}

pub(crate) use opcodes;

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
        let mut map = [0u8; 256];
        for (i, slot) in map.iter_mut().enumerate() {
            *slot = i as u8;
        }
        OpcodeMap {
            to_byte: map,
            from_byte: map,
        }
    }

    /// A seeded permutation of the opcode bytes among themselves (non-opcode bytes
    /// unchanged). The same seed always gives the same map.
    pub fn from_seed(seed: u64) -> Self {
        let mut map = Self::identity();
        // the bytes that are real opcodes, shuffled among themselves
        let mut codes: Vec<u8> = crate::OPCODES.iter().map(|o| o.code).collect();
        codes.sort_unstable();
        codes.dedup();
        let mut shuffled = codes.clone();
        // Fisher–Yates with a small deterministic PRNG (splitmix64)
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        for i in (1..shuffled.len()).rev() {
            let j = (next() % (i as u64 + 1)) as usize;
            shuffled.swap(i, j);
        }
        for (from, to) in codes.iter().zip(shuffled.iter()) {
            map.to_byte[*from as usize] = *to;
            map.from_byte[*to as usize] = *from;
        }
        map
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

    fn is_permutation(bytes: &[u8; 256]) -> bool {
        let mut seen = [false; 256];
        for &b in bytes {
            if seen[b as usize] {
                return false;
            }
            seen[b as usize] = true;
        }
        true
    }

    #[test]
    fn opcode_bytes_are_distinct() {
        let mut seen = [false; 256];
        for o in crate::OPCODES {
            assert!(!seen[o.code as usize], "duplicate opcode 0x{:02x}", o.code);
            seen[o.code as usize] = true;
        }
    }

    #[test]
    fn identity_is_identity() {
        assert!(OpcodeMap::identity().is_identity());
    }

    #[test]
    fn seeded_map_is_a_bijection_that_inverts() {
        for seed in [0u64, 1, 42, 0xdead_beef, u64::MAX] {
            let map = OpcodeMap::from_seed(seed);
            assert!(
                is_permutation(&map.to_byte),
                "seed {seed}: to_byte not a permutation"
            );
            for b in 0..=255u8 {
                assert_eq!(map.from_byte[map.to_byte[b as usize] as usize], b);
            }
            // every real opcode is renumbered to another real opcode (so the VM switch,
            // which only has cases for real opcodes, still covers them)
            let codes: std::collections::HashSet<u8> =
                crate::OPCODES.iter().map(|o| o.code).collect();
            for &c in &codes {
                assert!(codes.contains(&map.to_byte[c as usize]));
            }
        }
    }

    #[test]
    fn seeded_map_is_deterministic() {
        assert_eq!(
            OpcodeMap::from_seed(7).to_byte,
            OpcodeMap::from_seed(7).to_byte
        );
        assert_ne!(
            OpcodeMap::from_seed(1).to_byte,
            OpcodeMap::from_seed(2).to_byte
        );
    }
}
