# Introduction to MVL

## The inversion

For sixty years, programming languages have been designed for humans to write. Readability, ergonomics, expressiveness — the metrics that matter when a person sits at a keyboard and types code character by character. Syntactic sugar exists because humans are slow. List comprehensions exist because humans find loops verbose. Operator overloading exists because humans want `matrix_a + matrix_b` to look like math.

That era is ending.

Large language models generate code in any language, at any verbosity, with any annotation burden — for free. The cost of writing code has collapsed to near zero. A function that takes a human twenty minutes to write takes an LLM two seconds. The annotation burden that made dependently-typed languages impractical for industry — every function needs a termination proof, every value needs a security label, every integer needs a range constraint — disappears when the machine writes the annotations.

So why are we still designing languages for humans to write?

The MVL — the Minimum Verification Language — turns the question around. Instead of asking "what's pleasant to type?", it asks: **what's the maximum a compiler can verify per token of generated code?** Not what's easy to write. What's easy to prove correct.

## The problem MVL solves

Modern software has two crises converging simultaneously.

**The cybersecurity crisis.** In April 2026, Anthropic demonstrated that an AI model could autonomously discover and exploit vulnerabilities — finding a 27-year-old OpenBSD bug for $50 in compute, building a full remote code execution chain on FreeBSD with zero human involvement. Over 99% of the vulnerabilities it found remain unpatched. CrowdStrike's CTO observed that "what once took months now happens in minutes." Runtime defenses — firewalls, WAFs, intrusion detection — are reactive. They catch attacks after they happen. The attack surface is growing faster than defenders can patch.

**The safety crisis.** As AI-generated code enters mission-critical systems — avionics, medical devices, industrial control, financial infrastructure — the question of assurance becomes non-negotiable. ISO 15026, IEC 61508, and DO-178C don't ask "is this code well-written?" They ask "where is the evidence that this code satisfies its specification?" When a human writes code, the evidence is the human's reasoning. When an LLM writes code, the human's reasoning is gone. The evidence must come from somewhere else.

The MVL's answer to both: **move verification from runtime to compile time, from discipline to types, from human memory to compiler proofs.**

## Eleven requirements

The MVL compiler verifies eleven properties. The first seven come from the convergence of formal methods (Curry-Howard, Hoare, Girard) and safety-critical industrial practice (MISRA C, DO-178C, IEC 61508). Theory asked "what can a compiler prove?" Practice asked "what kills people when unproven?" Same answer.

The last four — termination checking, data race freedom, refinement types, and information flow control — were known for decades but considered impractical. The annotation burden was too high for human developers. Every recursive function needs a termination proof. Every value needs a security label. Every integer needs a range constraint. When LLMs generate all code, that burden is zero. Properties that were too expensive become free.

The result: code that compiles in the MVL is *well-formed*. Types are sound, memory is safe, all cases are handled, all errors are visible in signatures, side effects are declared, data flow is tracked, functions provably terminate, no data races exist, values are within valid ranges, and secrets cannot leak to public channels. This is internal quality — proven at compile time, at zero runtime cost.

What the compiler cannot prove — does the code do the right thing? does `sort()` actually sort? — is external quality, handled by tests. But every property the compiler verifies is a category of tests you never write. The stronger the well-formedness, the less validation work remains.

## The design

The MVL is deliberately the smallest general-purpose language. About ten statement forms, five expression forms, three declaration forms. Compare Python's thirty, Rust's twenty, Go's fifteen.

Everything that exists for human writability is dropped. No anonymous lambdas, no list comprehensions, no decorators, no operator overloading, no implicit conversions, no default arguments, no macros, no ternary operator, no string interpolation, no inheritance, no exceptions, no null, no global state. Each of these was a convenience for human typists. Each of them hides information from the compiler. In the MVL, the LLM generates the verbose explicit version, and the compiler verifies all of it.

What survives: `fn`, `let`, `if`/`else`, `match`, `for`, `return`, method calls, `?` propagation, type declarations, and modules. One way to branch. One way to loop. One way to handle errors. One way to represent absence.

The standard library follows the same philosophy. `Map.get()` returns `Option[T]` — never panics, never returns null. `format()` takes IFC-typed arguments — tainted strings cannot enter clean queries. Division requires a non-zero denominator in the type. File operations declare their effects. The stdlib isn't just functions you call — it's contracts the compiler verifies.

## The compilation strategy

Phase 1 transpiles MVL to Rust. Rust already scores six out of eleven on the requirements (type safety, memory safety, totality, null elimination, error visibility, ownership) — the transpilation adds the remaining five (effect tracking, termination, race freedom, refinements, information flow control) as a verification layer on top. This gets MVL running fast with access to Rust's ecosystem.

Phase 2 targets LLVM IR directly. One compiler, one trust boundary, one proof chain. The ISPE model (Intent → Specification → Program → Executable) requires the P→E step to be deterministic and proof-preserving. Two compilers in the chain — MVL then Rust — means two sets of opinions that might disagree. LLVM is the clean target: the MVL compiler proves the properties, LLVM generates the binary.

Phase 3 is self-hosting: the MVL compiler rewritten in MVL, compiled by itself. If the language can express its own compiler with all eleven requirements verified, it's general-purpose enough for anything.

## Who is this for?

The MVL is not for humans to write. It's for LLMs to generate, compilers to verify, and humans to review at the points where the compiler's guarantees end.

It's for organizations building mission-critical systems who need audit evidence that code satisfies its specification — and need that evidence generated automatically, not manually. It's for security teams who want entire vulnerability classes eliminated at compile time rather than caught at runtime. It's for the transition from AAE-3 (process-based assurance: specs before code, evidence linked to artifacts) to AAE-5 (compiler-based assurance: the compiler generates the evidence that external certification requires).

The field is making LLMs better at existing languages. Nobody is asking the opposite question: what if the language were designed for the LLM? The MVL is that question, answered.
