//! Phase 1 fuzz target: grammar generator → Rust transpiler.
//!
//! Each libFuzzer iteration:
//!   1. Feed raw bytes into the grammar-guided generator to produce MVL source.
//!   2. Parse it with the standard Parser (error-tolerant — returns a Program even on errors).
//!   3. Call `transpile()` and assert no panics, non-empty Rust output.
//!
//! A panic from step 2 or 3 is surfaced as a libFuzzer finding.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler::transpile;
use mvl_fuzz::generator::Generator;

fuzz_target!(|data: &[u8]| {
    // Generate MVL source from the raw fuzzer bytes.
    let mut gen = Generator::new(data);
    let Ok(src) = gen.gen_program() else {
        // Buffer exhausted before a complete program — skip this input.
        return;
    };

    // Parse — always produces a Program; errors are non-fatal and collected.
    let (mut parser, _lex_errors) = Parser::new(&src);
    let prog = parser.parse_program();

    // Transpile — must not panic regardless of parse errors in the AST.
    let output = transpile(&prog, "fuzz_target");

    // Non-empty Rust output is the minimum bar.
    assert!(!output.lib_rs.is_empty());
});
