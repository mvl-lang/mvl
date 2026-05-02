//! Grammar-guided MVL source generator.
//!
//! Produces syntactically valid MVL programs by walking the grammar productions
//! with depth-budgeted recursion.  Each choice point consumes bytes from an
//! `arbitrary::Unstructured` buffer so libFuzzer can guide coverage.
//!
//! Usage:
//! ```ignore
//! let mut gen = Generator::new(data);
//! let Ok(src) = gen.gen_program() else { return; };
//! ```

use arbitrary::{Result, Unstructured};

const MAX_DEPTH: usize = 6;

pub struct Generator<'a> {
    u: Unstructured<'a>,
    depth: usize,
    counter: usize,
}

impl<'a> Generator<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Generator {
            u: Unstructured::new(data),
            depth: 0,
            counter: 0,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        let n = self.counter;
        self.counter += 1;
        format!("{prefix}{n}")
    }

    fn pick<T: Copy>(&mut self, choices: &[T]) -> Result<T> {
        Ok(*self.u.choose(choices)?)
    }

    fn small(&mut self, max_exclusive: u8) -> Result<u8> {
        Ok(self.u.arbitrary::<u8>()? % max_exclusive)
    }

    // ── Base types ────────────────────────────────────────────────────────

    fn gen_base_type(&mut self) -> Result<&'static str> {
        self.pick(&["Int", "Float", "Bool", "String"])
    }

    pub fn gen_type(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Ok(self.gen_base_type()?.to_string());
        }
        self.depth += 1;
        let result = match self.small(6)? {
            0 => self.gen_base_type()?.to_string(),
            1 => format!("Option<{}>", self.gen_base_type()?),
            2 => {
                let ok = self.gen_base_type()?;
                let err = self.gen_base_type()?;
                format!("Result<{ok}, {err}>")
            }
            3 => format!("&{}", self.gen_base_type()?),
            4 => {
                let a = self.gen_base_type()?;
                let b = self.gen_base_type()?;
                format!("({a}, {b})")
            }
            _ => self.gen_base_type()?.to_string(),
        };
        self.depth -= 1;
        Ok(result)
    }

    // ── Literals ──────────────────────────────────────────────────────────

    fn gen_literal(&mut self) -> Result<String> {
        Ok(match self.small(5)? {
            0 => format!("{}", self.u.arbitrary::<i32>()?),
            1 => format!("{:.1}", self.u.arbitrary::<i8>()? as f64),
            2 => "true".to_string(),
            3 => "false".to_string(),
            _ => "\"hello\"".to_string(),
        })
    }

    // ── Expressions ───────────────────────────────────────────────────────

    pub fn gen_expr(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return self.gen_literal();
        }
        self.depth += 1;
        let result = match self.small(7)? {
            0 => self.gen_literal()?,
            1 => {
                let left = self.gen_literal()?;
                let right = self.gen_literal()?;
                let op = self.pick(&["+", "-", "*", "==", "!=", "<", ">"])?;
                format!("({left} {op} {right})")
            }
            2 => {
                // if-else — use bool literal as condition so it type-checks
                let cond = self.pick(&["true", "false"])?;
                let then = self.gen_expr()?;
                let else_ = self.gen_expr()?;
                format!("if {cond} {{ {then} }} else {{ {else_} }}")
            }
            3 => {
                // negation
                let inner = self.gen_literal()?;
                format!("(-{inner})")
            }
            4 => {
                // tuple literal
                let a = self.gen_literal()?;
                let b = self.gen_literal()?;
                format!("({a}, {b})")
            }
            5 => {
                // unit
                "()".to_string()
            }
            _ => self.gen_literal()?,
        };
        self.depth -= 1;
        Ok(result)
    }

    // ── Statements ────────────────────────────────────────────────────────

    fn gen_stmt(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Ok(format!("{};", self.gen_literal()?));
        }
        self.depth += 1;
        let result = match self.small(4)? {
            0 => {
                // explicit type on let binding (required by #408)
                let name = self.fresh("x");
                let ty = self.gen_base_type()?;
                let val = self.gen_literal()?;
                format!("let {name}: {ty} = {val};")
            }
            1 => format!("return {};", self.gen_expr()?),
            2 => {
                // while loop
                let cond = self.pick(&["true", "false"])?;
                let body = self.gen_simple_block()?;
                format!("while {cond} {body}")
            }
            _ => format!("{};", self.gen_expr()?),
        };
        self.depth -= 1;
        Ok(result)
    }

    fn gen_simple_block(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Ok("{ }".to_string());
        }
        let n = self.small(3)? as usize;
        let mut stmts = Vec::with_capacity(n);
        for _ in 0..n {
            stmts.push(self.gen_stmt()?);
        }
        Ok(format!("{{ {} }}", stmts.join(" ")))
    }

    fn gen_block(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Ok(format!("{{ {} }}", self.gen_literal()?));
        }
        let n = self.small(4)? as usize;
        let mut stmts = Vec::with_capacity(n);
        for _ in 0..n {
            stmts.push(self.gen_stmt()?);
        }
        // tail expression (block return value)
        let tail = self.gen_expr()?;
        Ok(format!("{{ {} {tail} }}", stmts.join(" ")))
    }

    // ── Declarations ──────────────────────────────────────────────────────

    fn gen_param(&mut self, idx: usize) -> Result<String> {
        let ty = self.gen_base_type()?;
        Ok(format!("p{idx}: {ty}"))
    }

    fn gen_fn_decl(&mut self) -> Result<String> {
        let name = self.fresh("fn_");
        let n_params = self.small(4)? as usize;
        let params: Result<Vec<_>> = (0..n_params).map(|i| self.gen_param(i)).collect();
        let ret = self.gen_type()?;
        let body = self.gen_block()?;
        Ok(format!("fn {name}({}) -> {ret} {body}", params?.join(", ")))
    }

    fn gen_type_alias(&mut self) -> Result<String> {
        let name = self.fresh("T");
        let ty = self.gen_base_type()?;
        Ok(format!("type {name} = {ty};"))
    }

    fn gen_const_decl(&mut self) -> Result<String> {
        let name = self.fresh("C").to_uppercase();
        let ty = self.gen_base_type()?;
        let val = self.gen_literal()?;
        Ok(format!("const {name}: {ty} = {val};"))
    }

    fn gen_decl(&mut self) -> Result<String> {
        let vis = if self.u.arbitrary::<bool>()? { "pub " } else { "" };
        let body = match self.small(3)? {
            0 => self.gen_fn_decl()?,
            1 => self.gen_type_alias()?,
            _ => self.gen_const_decl()?,
        };
        Ok(format!("{vis}{body}"))
    }

    // ── Program ───────────────────────────────────────────────────────────

    pub fn gen_program(&mut self) -> Result<String> {
        let n = (self.small(3)? + 1) as usize;
        let decls: Result<Vec<_>> = (0..n).map(|_| self.gen_decl()).collect();
        Ok(decls?.join("\n\n"))
    }

    // ── Differential program (Phase 3) ────────────────────────────────────
    //
    // Produces a terminating program with a main that prints Int results so
    // both backends can be run and their stdout compared.  Only uses constructs
    // verified to work on both backends: Int arithmetic, if/else, total fns,
    // println with a format string.

    fn gen_int_expr(&mut self, params: &[String]) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Ok(format!("{}", (self.u.arbitrary::<u8>()? % 20 + 1) as i64));
        }
        self.depth += 1;
        let result = match self.small(5)? {
            0 => format!("{}", (self.u.arbitrary::<u8>()? % 20 + 1) as i64),
            1 if !params.is_empty() => self.u.choose(params)?.clone(),
            2 => {
                let left = self.gen_int_expr(params)?;
                let right = self.gen_int_expr(params)?;
                let op = self.pick(&["+", "-", "*"])?;
                format!("({left} {op} {right})")
            }
            3 if params.len() >= 2 => {
                let a = self.u.choose(params)?.clone();
                let b = self.u.choose(params)?.clone();
                let then_val = self.gen_int_expr(params)?;
                let else_val = self.gen_int_expr(params)?;
                format!("if {a} > {b} {{ {then_val} }} else {{ {else_val} }}")
            }
            _ => format!("{}", (self.u.arbitrary::<u8>()? % 20 + 1) as i64),
        };
        self.depth -= 1;
        Ok(result)
    }

    pub fn gen_diff_program(&mut self) -> Result<String> {
        let n_helpers = (self.small(3)? + 1) as usize;
        let mut decls = Vec::new();

        // Helper functions: total fns taking 1-2 Int params, returning Int.
        // No loops, no recursion → guaranteed to terminate.
        let mut calls = Vec::new();
        for i in 0..n_helpers {
            let name = format!("f{i}");
            let n_params = (self.small(2)? + 1) as usize;
            let params: Vec<String> = (0..n_params).map(|j| format!("p{j}: Int")).collect();
            let param_names: Vec<String> = (0..n_params).map(|j| format!("p{j}")).collect();
            let body = self.gen_int_expr(&param_names)?;
            decls.push(format!("total fn {name}({}) -> Int {{ {body} }}", params.join(", ")));

            // Concrete args main will pass — small positive values avoid overflow.
            let args: Result<Vec<_>> = (0..n_params)
                .map(|_| Ok(format!("{}", (self.u.arbitrary::<u8>()? % 20 + 1) as i64)))
                .collect();
            calls.push((format!("r{i}"), format!("{name}({})", args?.join(", "))));
        }

        // main: call each helper, println its result.
        let mut main_body = Vec::new();
        for (var, call) in &calls {
            main_body.push(format!("    let {var}: Int = {call};"));
            main_body.push(format!("    println(\"{{}}\", {var});"));
        }
        decls.push(format!(
            "fn main() -> Unit ! Console {{\n{}\n}}",
            main_body.join("\n")
        ));

        Ok(decls.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_non_empty_program() {
        let data = [42u8; 256];
        let mut gen = Generator::new(&data);
        let prog = gen.gen_program().unwrap();
        assert!(!prog.is_empty());
    }

    #[test]
    fn depth_guard_prevents_overflow() {
        // Very large input → deep recursion should still terminate
        let data = vec![255u8; 4096];
        let mut gen = Generator::new(&data);
        let result = gen.gen_program();
        assert!(result.is_ok());
    }
}
