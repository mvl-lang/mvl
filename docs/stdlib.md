# Standard Library: Three Tiers

Derived from cross-language analysis of Rust std, Go std, Python stdlib, and C libc. Principle: **core is tiny (~30 types/functions), standard is complete (~200), everything else is packages.**

#### Core (every program needs these)

**Types:** `Bool`, `Int` (arbitrary precision), `Int8`..`Int64`, `UInt8`..`UInt64`, `Float32`, `Float64`, `Byte`, `Char` (Unicode scalar), `String` (UTF-8, immutable), `Array[T]`, `Map[K,V]`, `Set[T]`, `Option[T]`, `Result[T,E]`, `Tuple`, `Range`, `Iterator[T]`.

**String ops:** `len`, `is_empty`, `concat`, `join`, `split`, `trim`, `contains`, `starts_with`, `ends_with`, `find`, `replace`, `to_upper`, `to_lower`, `format`, `to_string`/`from_string`, `chars`, `bytes`.

**Collection ops:** `new`, `push`, `pop`, `insert`, `remove`, `get` → `Option[T]` (never panic), `contains`, `len`, `is_empty`, `iter` → `Iterator[T]`, `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort`, `sort_by`, `reverse`, `enumerate`, `zip`, `collect`, `min`, `max`, `sum`.

**Iterator protocol:** All collection types implement `Iterator[T]` via `.iter()`. Lazy operations (`map`, `filter`, `flat_map`) return `Iterator[U]` — no allocation until a terminal operation forces evaluation. Terminal operations: `fold`, `collect`, `any`, `all`, `find`, `sum`, `min`, `max`.

```mvl
// Lazy — returns Iterator[Int], no allocation yet
fn map[T, U](self: Iterator[T], f: fn(T) -> U) -> Iterator[U]
fn filter[T](self: Iterator[T], pred: fn(&T) -> Bool) -> Iterator[T]
fn flat_map[T, U](self: Iterator[T], f: fn(T) -> Iterator[U]) -> Iterator[U]
fn enumerate[T](self: Iterator[T]) -> Iterator[(UInt, T)]
fn zip[T, U](self: Iterator[T], other: Iterator[U]) -> Iterator[(T, U)]

// Terminal — forces evaluation
fn fold[T, U](self: Iterator[T], init: U, f: fn(U, T) -> U) -> U
fn collect[T](self: Iterator[T]) -> Array[T]
fn any[T](self: Iterator[T], pred: fn(&T) -> Bool) -> Bool
fn all[T](self: Iterator[T], pred: fn(&T) -> Bool) -> Bool
fn find[T](self: Iterator[T], pred: fn(&T) -> Bool) -> Option[T]
fn sum[T](self: Iterator[T]) -> T  where T: Add, T: Default
fn min[T](self: Iterator[T]) -> Option[T]  where T: Ord
fn max[T](self: Iterator[T]) -> Option[T]  where T: Ord
```

**Errors:** `Result[T,E]` + `Option[T]` with `?` propagation. `.map()`, `.and_then()`, `.unwrap_or()`. `Error` interface with `.message()` and `.source()`. `panic` for unrecoverable only.

**Math (core):** `+`, `-`, `*`, `/`, `%`, `abs`, `min`, `max`. `checked_add`, `checked_sub`, `checked_mul`, `checked_div` → `Option` (overflow-safe). Default `+` on fixed-width integers requires proof of no overflow or is a compile error.

**I/O (core):** `print`, `println`, `eprint`, `eprintln`, `stdin`, `stdout`, `stderr`.

**OS (core):** `env.get(key)` → `Option[String]`, `args()` → `Array[String]`, `exit(code)`, `current_dir()` → `Result[Path]`, `chdir(path)` → `Result ! Env`.

#### Standard (most programs need these)

| Category | What's included |
|----------|----------------|
| **File I/O** | `File.open` → `Result ! FileRead`, `File.create` → `Result ! FileWrite`, `read_to_string`, `write`, `BufReader`/`BufWriter`, `Reader`/`Writer` traits |
| **Path** | `Path` type, `join`, `parent`, `file_name`, `extension`, `exists`, `is_file`, `is_dir`, `is_symlink` |
| **Filesystem** | `create_dir_all`, `remove_file`, `remove_dir`, `read_dir`, `metadata`, `copy`, `rename`, `create_symlink`, `read_link`, `set_permissions`, `permissions` |
| **Regex** | `Regex` type with `match`, `find_all`, `replace`, `captures` |
| **Math** | `floor`, `ceil`, `round`, `sqrt`, `pow`, `sin`, `cos`, `tan`, `log`, `exp`, `PI`, `E`, `NAN`, `INFINITY` |
| **Random** | `random.int(min,max)`, `random.float()`, `random.choice()`, `crypto_random.bytes(n)` |
| **Time** | `Instant`, `DateTime`, `Duration`, `now()`, `sleep()`, `format()`, `parse()`, timezone (IANA) |
| **Concurrency** | `spawn(fn)` → `Handle[T]`, `Channel[T]`, `Mutex[T]`, `RwLock[T]`, `Atomic[T]`, `select` |
| **JSON** | `json.encode(value)` → `Result[String]`, `json.decode[T](string)` → `Result[T]` |
| **TOML** | `toml.encode(value)` → `Result[String]`, `toml.decode[T](string)` → `Result[T]`. Config file format — MVL's own `dependency.toml` uses it. |
| **Crypto (basic)** | `sha256`, `sha512`, `crypto_random.bytes` |
| **Process** | `process.spawn(cmd, args)` -> `Result[Child] ! ProcessSpawn` with `.stdin(Pipe)`, `.stdout(Capture)`, `.stderr(Capture)`. `child.wait()` -> `ExitStatus`. Process stdout is `Tainted`. |
| **OS** | `env.set`, `env.all`, `current_dir`, `chdir`, `getuid`, `getgid`, `signal.on(SIGINT, handler)` |
| **Testing** | `#[test]`, `assert`, `assert_eq`, `assert_ne`, built-in test runner, `#[bench]` |
| **Logging** | `log.debug`, `log.info`, `log.warn`, `log.error`, structured key-value pairs |

#### Extended (packages, not stdlib)

Networking (TCP, HTTP, TLS, DNS, WebSocket), serialization extras (YAML, XML, CSV, protobuf), crypto extras (AES, RSA, ECDSA, bcrypt, argon2, X.509), database drivers, CLI argument parsing, compression, advanced data structures (B-tree, trie, bloom filter, LRU).

Note: YAML is deliberately in extended, not standard. Complex spec (anchors, aliases, implicit typing), security footguns (`Norway` becomes `false`). TOML covers the config use case in stdlib.

**Key decisions vs other languages:**
- **JSON in stdlib** (unlike Rust, C) — universal interchange format, can't be optional
- **Regex in stdlib** (like Go, Python) — used by ~70%+ of projects
- **HTTP in extended** (unlike Go) — keep stdlib lean, HTTP is complex
- **DateTime with timezones in stdlib** (unlike Rust) — punting to external is a mistake
- **Checked arithmetic by default** (unlike everyone) — overflow is a bug, wrapping/saturating must be explicit

# How the Type System Changes the Stdlib

The eleven requirements make familiar stdlib functions unfamiliar:

```mvl
// Division: Req 10 — denominator can't be zero
fn divide(a: Int, b: Int where b != 0) -> Int

// Collection access: Req 4 — returns Option, never panics
fn Map.get(key: K) -> Option[V]
fn Array.get(index: UInt) -> Option[T]

// File I/O: Req 7 — effects declared
fn read_file(path: Path) -> Result[String, IOError] ! FileRead
fn write_file(path: Path, data: String) -> Result<(), IOError> ! FileWrite

// Network: Req 11 — data from network is Tainted
fn http_get(url: Clean[Url]) -> Result[Tainted[Response], NetError] ! Net

// String formatting: Req 11 — no tainted interpolation
fn sql_query(template: String, params: Array[SqlParam]) -> Query ! DB
// format("SELECT * WHERE id = {}", tainted_input)  → COMPILE ERROR

// Numeric: Req 10 — overflow checked by default
let a: Int32 = Int32.MAX
let b = a + 1                    // COMPILE ERROR: potential overflow
let b = a.checked_add(1)         // → Option<Int32>
let b = a.wrapping_add(1)        // → Int32 (explicit wrap)
let b = a.saturating_add(1)      // → Int32 (clamps to MAX)

// Resource cleanup: Req 6 — files must be closed
fn open_file(path: Path) -> Result[File, IOError] ! FileRead
// File has linear type: must be consumed (closed/passed/returned)

// Random: Req 7 — randomness is an effect
fn random_int(min: Int, max: Int) -> Int ! Random
// Pure functions can't call random — visible in type
```

**The pattern:** every stdlib function that can fail returns `Result` or `Option`. Every function with side effects declares them. Every function receiving external data tags it `Tainted`. Every numeric operation on bounded types checks overflow. The stdlib isn't just "functions you call" — it's "contracts the compiler verifies."
