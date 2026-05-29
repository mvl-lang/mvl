//! Phase 2 fuzz target: grammar generator → LLVM codegen pipeline.
//!
//! Each libFuzzer iteration:
//!   1. Feed raw bytes into the grammar-guided generator to produce MVL source.
//!   2. Parse it with the standard error-tolerant Parser.
//!   3. Call `compile_to_ir()` — must not panic; clean Err is acceptable.
//!   4. Assert the returned IR string is non-empty when compilation succeeds.
//!
//! A panic from any step is a bug. A clean Err (unsupported construct, type
//! error, etc.) is expected and silently skipped.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mvl::mvl::backends::llvm::LlvmCompiler;
use mvl::mvl::parser::Parser;
use mvl_fuzz::generator::Generator;

// One context for the lifetime of the fuzz process — amortises Context::create() cost.
std::thread_local! {
    static COMPILER: LlvmCompiler = LlvmCompiler::new();
}

fuzz_target!(|data: &[u8]| {
    let mut gen = Generator::new(data);
    let Ok(src) = gen.gen_program() else {
        return;
    };

    let (mut parser, _lex_errors) = Parser::new(&src);
    let prog = parser.parse_program();

    // compile_to_ir returns Result<String, String> — never panics by contract.
    COMPILER.with(|compiler| {
        if let Ok(ir) = compiler.compile_to_ir(&prog, "fuzz_target") {
            assert!(!ir.is_empty());
        }
        // Err variant means unsupported/invalid construct — not a bug, skip silently.
    });
});
