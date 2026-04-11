// Unit-error `Result<T, ()>` is idiomatic for LL(1) parser error-propagation.
// Errors are pushed into `Parser::errors`; `Err(())` is a control-flow signal.
#![allow(clippy::result_unit_err)]

pub mod mvl;

#[cfg(test)]
mod corpus_debug {
    use crate::mvl::checker::check;
    use crate::mvl::parser::Parser;
    #[test]
    fn debug_propagation_corpus() {
        let src = include_str!("../tests/corpus/04_effects/propagation.mvl");
        let (mut p, errs) = Parser::new(src);
        let prog = p.parse_program();
        eprintln!("Lex errors: {:?}", errs);
        eprintln!("Parse errors: {:?}", p.errors());
        eprintln!("Decls: {}", prog.declarations.len());
        let r = check(&prog);
        eprintln!("Check errors: {:?}", r.errors);
    }
}
