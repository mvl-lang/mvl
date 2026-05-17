// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust runtime backing for `std.args`.
//!
//! `std/args.mvl` is now a pure MVL implementation — no Rust backing required.
//! This module is intentionally empty; it exists so that the transpiler's
//! `use mvl_runtime::stdlib::args::*;` wildcard import (emitted for any
//! `use std.args.*` declaration) resolves without error.
