// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.args` stdlib functions.
//!
//! `std/args.mvl` is now a pure MVL implementation — no C-ABI backing is
//! required.  The builtin `get_arg`, `get_args`, and `get_env` functions have
//! been removed; this module is intentionally empty.
//!
//! `std.env.args()` (LLVM-backed via `_mvl_env_args`) provides raw argv access.
