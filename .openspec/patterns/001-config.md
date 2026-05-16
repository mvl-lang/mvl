# Pattern 001: Layered Configuration (Defaults → TOML → Env → CLI → Struct)

## Summary

Load configuration from multiple sources in priority order. Later layers override earlier
ones. Type safety is enforced at struct construction, not at parsing.

## Reference implementation

`examples/actor_webserver/config.mvl`

## Priority order (lowest → highest)

| Layer | Source | Effect |
|-------|--------|--------|
| 1 | Compiled defaults (`fn defaults()`) | none |
| 2 | TOML file (`config.toml`) | `FileRead` |
| 3 | Environment variables (`PREFIX_KEY`) | `Env` |
| 4 | CLI arguments (`--key value`) | `Console` |
| 5 | Struct construction (validation) | none |

Each layer deep-merges into the previous via `std.config.merge`.

## Code pattern

```mvl
use std.config.{ConfigValue, ConfigError, merge, with_env, get_path,
                as_int, as_string, as_bool, from_toml}
use std.toml.{toml_decode}
use std.io.{read_file}
use std.args.{get_arg}
use std.strings.{str_concat, str_parse_int}

pub type ServerConfig = struct {
    host:  String,
    port:  Int,
    debug: Bool,
}

fn defaults() -> ConfigValue {
    ConfigValue::Map({
        "host":  ConfigValue::String("localhost"),
        "port":  ConfigValue::Int(8080),
        "debug": ConfigValue::Bool(false),
    })
}

pub fn load_config() -> Result[ServerConfig, ConfigError] ! Env + FileRead + Console {
    // Layer 1: defaults
    let cfg: ConfigValue = defaults();

    // Layer 2: TOML file (optional — silently skipped if absent)
    let cfg: ConfigValue = match read_file("config.toml") {
        Ok(raw) => match toml_decode(str_concat(raw, "")) {
            Ok(tv)  => merge(cfg, from_toml(tv)),
            Err(_)  => cfg,
        },
        Err(_) => cfg,
    };

    // Layer 3: env vars (MYAPP_HOST, MYAPP_PORT, MYAPP_DEBUG)
    let cfg: ConfigValue = match with_env(cfg, "MYAPP") {
        Ok(c)  => c,
        Err(_) => cfg,
    };

    // Layer 4: CLI args (--host, --port, --debug)
    let cfg: ConfigValue = merge_cli_args(cfg);

    // Layer 5: construct typed struct — validation at struct boundary
    Ok(ServerConfig {
        host:  as_string(get_path(cfg, "host").unwrap_or(ConfigValue::String("localhost")))?,
        port:  as_int(get_path(cfg, "port").unwrap_or(ConfigValue::Int(8080)))?,
        debug: as_bool(get_path(cfg, "debug").unwrap_or(ConfigValue::Bool(false))).unwrap_or(false),
    })
}

fn merge_cli_args(cfg: ConfigValue) -> ConfigValue ! Console {
    let cfg: ConfigValue = match get_arg("host") {
        Some(v) => merge(cfg, ConfigValue::Map({"host": ConfigValue::String(str_concat(v, ""))})),
        None    => cfg,
    };
    let cfg: ConfigValue = match get_arg("port") {
        Some(v) => match str_parse_int(str_concat(v, "")) {
            Ok(n)  => merge(cfg, ConfigValue::Map({"port": ConfigValue::Int(n)})),
            Err(_) => cfg,
        },
        None => cfg,
    };
    let cfg: ConfigValue = match get_arg("debug") {
        Some(_) => merge(cfg, ConfigValue::Map({"debug": ConfigValue::Bool(true)})),
        None    => cfg,
    };
    cfg
}
```

## Key design points

- **Each layer overrides the previous** via `std.config.merge` (deep merge for nested maps).
- **Type safety at struct construction**, not at parsing — `as_*` helpers coerce and validate.
- **IFC boundary**: `read_file` returns `Tainted[String]`; coerce to `String` via
  `str_concat(raw, "")` before passing to `toml_decode`. Config files are
  operator-controlled and trusted at this boundary.
- **Effects are explicit**: `Env + FileRead + Console` propagate to callers.
- **Missing file / unset env / absent CLI arg** are all silently skipped — only wrong types
  produce errors.

## Local config.toml

```toml
host  = "127.0.0.1"
port  = 8080
debug = false
```

Place `config.toml` next to the entry-point `.mvl` file. The `load_config` function reads it
relative to the working directory where `mvl run` is invoked.

## Testing

Pure config logic (defaults, merging, struct extraction) can be tested without I/O effects
by inlining `defaults()` and `extract()` in `*_test.mvl` files. See
`examples/actor_webserver/config_test.mvl` for a complete example.

TOML parsing can be tested by decoding a literal string:

```mvl
test fn toml_overrides_port() -> Unit {
    let merged: ConfigValue = match toml_decode("port = 9090\n") {
        Ok(tv)  => merge(defaults(), from_toml(tv)),
        Err(_)  => defaults(),
    };
    assert_eq(as_int(get_path(merged, "port").unwrap_or(ConfigValue::Int(0))), Ok(9090))
}
```
