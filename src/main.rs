use mvl::mvl::parser::lexer::Lexer;

fn main() {
    println!("mvl compiler v{}", env!("CARGO_PKG_VERSION"));

    // Quick smoke-test: lex a snippet and show token count
    let src = "fn main() -> Int { 42 }";
    let (tokens, errors) = Lexer::new(src).tokenize();
    if !errors.is_empty() {
        eprintln!("lex errors: {:?}", errors);
        std::process::exit(1);
    }
    println!("lexed {} tokens", tokens.len());
}
