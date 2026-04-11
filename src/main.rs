use mvl::mvl::parser::lexer::Lexer;

fn main() {
    println!("mvl compiler v{}", env!("CARGO_PKG_VERSION"));

    // Quick smoke-test: lex a snippet and show token count
    // Fix #12: use early return instead of process::exit so the binary remains
    // useful even when the embedded snippet has lex errors.
    let src = "fn main() -> Int { 42 }";
    let (tokens, errors) = Lexer::new(src).tokenize();
    if !errors.is_empty() {
        eprintln!("lex errors: {:?}", errors);
        return;
    }
    println!("lexed {} tokens", tokens.len());
}
