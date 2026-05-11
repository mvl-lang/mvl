// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

// Unit-error `Result<T, ()>` is idiomatic for LL(1) parser error-propagation.
// Errors are pushed into `Parser::errors`; `Err(())` is a control-flow signal.
#![allow(clippy::result_unit_err)]

pub mod mvl;
