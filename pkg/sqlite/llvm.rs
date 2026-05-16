//! llvm.rs — pkg.sqlite LLVM backend (C-ABI layer)
//!
//! Mirrors bridge.rs with `extern "C"` symbols so the LLVM backend can link
//! against this file instead of bridge.rs.  The LLVM codegen resolves
//! `extern "c"` declarations in ffi.mvl to these symbols via the linker.
//!
//! # Build wiring
//!
//! This file is compiled and linked as part of `mvl build --backend llvm`.
//! The build system discovers it via the #811-A convention: any package
//! containing `llvm.rs` alongside `bridge.rs` gets both compiled, with the
//! correct one selected per backend.
//!
//! # String ABI
//!
//! Input strings  → `*const MvlString`  (caller owns, not freed here)
//! Output strings → `*mut MvlString`    (allocated via `mvl_string_new`,
//!                                       caller owns and must drop)
//!
//! MvlString layout matches `runtime/llvm/src/memory.rs`:
//!   { ptr: *mut u8, len: u64, cap: u64, refcount: u64 }
//!
//! `mvl_string_new` is supplied by the LLVM runtime (always linked).
//!
//! # Bool ABI
//!
//! MVL Bool is a 1-byte integer at the LLVM IR level; i8 in C-ABI.
//!
//! See #785, #811.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::types::{Value as RValue, ValueRef};

#[repr(transparent)]
pub struct Clean<T>(pub T);

// ── MvlString (mirrored from runtime/llvm/src/memory.rs) ─────────────────────

#[repr(C)]
pub struct MvlString {
    pub ptr: *mut u8,
    pub len: u64,
    pub cap: u64,
    pub refcount: u64,
}

extern "C" {
    fn mvl_string_new(ptr: *const u8, len: usize) -> *mut MvlString;
}

unsafe fn read_str(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = unsafe { (*s).len as usize };
    if len == 0 || unsafe { (*s).ptr.is_null() } {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts((*s).ptr as *const u8, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn new_mvl_str(s: &str) -> *mut MvlString {
    let b = s.as_bytes();
    unsafe { mvl_string_new(b.as_ptr(), b.len()) }
}

// ── Internal types (identical to bridge.rs) ───────────────────────────────────

#[derive(Clone)]
enum SqliteVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}

struct QueryResult {
    col_names: Vec<String>,
    rows: Vec<Vec<SqliteVal>>,
}

// ── Global handle tables ──────────────────────────────────────────────────────

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_handle() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

fn connections() -> &'static Mutex<HashMap<i64, rusqlite::Connection>> {
    static C: OnceLock<Mutex<HashMap<i64, rusqlite::Connection>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn param_bufs() -> &'static Mutex<HashMap<i64, Vec<SqliteVal>>> {
    static P: OnceLock<Mutex<HashMap<i64, Vec<SqliteVal>>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn results() -> &'static Mutex<HashMap<i64, QueryResult>> {
    static R: OnceLock<Mutex<HashMap<i64, QueryResult>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn errors() -> &'static Mutex<HashMap<i64, (i64, String)>> {
    static E: OnceLock<Mutex<HashMap<i64, (i64, String)>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

fn classify(e: &rusqlite::Error) -> (i64, String) {
    let msg = e.to_string();
    match e {
        rusqlite::Error::QueryReturnedNoRows => (0, msg),
        rusqlite::Error::SqliteFailure(err, _) => match err.code {
            rusqlite::ErrorCode::ConstraintViolation => (1, msg),
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => (3, msg),
            _ if msg.contains("no such table") || msg.contains("no such column") => (0, msg),
            _ => (4, msg),
        },
        rusqlite::Error::SqlInputError { .. } => (2, msg),
        _ => (4, msg),
    }
}

fn store_err(db: i64, e: &rusqlite::Error) {
    errors().lock().unwrap().insert(db, classify(e));
}

fn decode_blob(csv: &str) -> Vec<u8> {
    if csv.is_empty() {
        return Vec::new();
    }
    csv.split(',')
        .filter_map(|s| s.trim().parse::<u8>().ok())
        .collect()
}

fn encode_blob(b: &[u8]) -> String {
    b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

fn to_rvalue(v: SqliteVal) -> RValue {
    match v {
        SqliteVal::Null => RValue::Null,
        SqliteVal::Bool(b) => RValue::Integer(b as i64),
        SqliteVal::Int(n) => RValue::Integer(n),
        SqliteVal::Float(f) => RValue::Real(f),
        SqliteVal::Text(s) => RValue::Text(s),
        SqliteVal::Blob(b) => RValue::Blob(b),
    }
}

fn from_ref(vr: ValueRef<'_>) -> SqliteVal {
    match vr {
        ValueRef::Null => SqliteVal::Null,
        ValueRef::Integer(n) => SqliteVal::Int(n),
        ValueRef::Real(f) => SqliteVal::Float(f),
        ValueRef::Text(s) => SqliteVal::Text(String::from_utf8_lossy(s).into_owned()),
        ValueRef::Blob(b) => SqliteVal::Blob(b.to_vec()),
    }
}

fn with_cell<T>(result: i64, row: i64, col: i64, f: impl Fn(&SqliteVal) -> T) -> Option<T> {
    let guard = results().lock().unwrap();
    let cell = guard
        .get(&result)?
        .rows
        .get(row as usize)?
        .get(col as usize)?;
    Some(f(cell))
}

// ── C-ABI exports ─────────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn sqlite_open(path: *const MvlString) -> i64 {
    let p = unsafe { read_str(path) };
    match rusqlite::Connection::open(&p) {
        Ok(conn) => {
            let h = next_handle();
            connections().lock().unwrap().insert(h, conn);
            h
        }
        Err(e) => {
            store_err(-1, &e);
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn sqlite_close(db: i64) {
    connections().lock().unwrap().remove(&db);
    param_bufs().lock().unwrap().remove(&db);
    errors().lock().unwrap().remove(&db);
}

#[no_mangle]
pub extern "C" fn sqlite_errmsg(db: i64) -> *mut MvlString {
    let msg = errors()
        .lock()
        .unwrap()
        .get(&db)
        .map(|(_, m)| m.clone())
        .unwrap_or_default();
    new_mvl_str(&msg)
}

#[no_mangle]
pub extern "C" fn sqlite_errno(db: i64) -> i64 {
    errors()
        .lock()
        .unwrap()
        .get(&db)
        .map(|(c, _)| *c)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn sqlite_changes(db: i64) -> i64 {
    connections()
        .lock()
        .unwrap()
        .get(&db)
        .map(|c| c.changes() as i64)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_param_reset(db: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().clear();
}

#[no_mangle]
pub extern "C" fn sqlite_param_null(db: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Null);
}

#[no_mangle]
pub extern "C" fn sqlite_param_bool(db: i64, v: i8) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Bool(v != 0));
}

#[no_mangle]
pub extern "C" fn sqlite_param_int(db: i64, v: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Int(v));
}

#[no_mangle]
pub extern "C" fn sqlite_param_float(db: i64, v: f64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Float(v));
}

#[no_mangle]
pub unsafe extern "C" fn sqlite_param_text(db: i64, v: *const MvlString) {
    let s = unsafe { read_str(v) };
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Text(s));
}

#[no_mangle]
pub unsafe extern "C" fn sqlite_param_blob(db: i64, v: *const MvlString) {
    let csv = unsafe { read_str(v) };
    let bytes = decode_blob(&csv);
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Blob(bytes));
}

#[no_mangle]
pub unsafe extern "C" fn sqlite_execute(db: i64, sql: Clean<*const MvlString>) -> i64 {
    let sql_str = unsafe { read_str(sql.0) };
    let params: Vec<RValue> = param_bufs()
        .lock()
        .unwrap()
        .remove(&db)
        .unwrap_or_default()
        .into_iter()
        .map(to_rvalue)
        .collect();
    let result = {
        let guard = connections().lock().unwrap();
        let Some(conn) = guard.get(&db) else { return -1; };
        conn.execute(&sql_str, rusqlite::params_from_iter(params.iter()))
            .map(|n| n as i64)
            .map_err(|e| classify(&e))
    };
    match result {
        Ok(n) => n,
        Err(err) => { errors().lock().unwrap().insert(db, err); -1 }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite_query(db: i64, sql: Clean<*const MvlString>) -> i64 {
    let sql_str = unsafe { read_str(sql.0) };
    let params: Vec<RValue> = param_bufs()
        .lock()
        .unwrap()
        .remove(&db)
        .unwrap_or_default()
        .into_iter()
        .map(to_rvalue)
        .collect();
    let result: Result<QueryResult, (i64, String)> = (|| {
        let guard = connections().lock().unwrap();
        let conn = guard.get(&db).ok_or_else(|| (4i64, "invalid db handle".to_string()))?;
        let mut stmt = conn.prepare(&sql_str).map_err(|e| classify(&e))?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let n_cols = col_names.len();
        let mut rows: Vec<Vec<SqliteVal>> = Vec::new();
        let mut iter = stmt.query(rusqlite::params_from_iter(params.iter())).map_err(|e| classify(&e))?;
        while let Some(row) = iter.next().map_err(|e| classify(&e))? {
            let cells = (0..n_cols).map(|i| row.get_ref(i).map(from_ref).unwrap_or(SqliteVal::Null)).collect();
            rows.push(cells);
        }
        Ok(QueryResult { col_names, rows })
    })();
    match result {
        Ok(qr) => { let rh = next_handle(); results().lock().unwrap().insert(rh, qr); rh }
        Err(err) => { errors().lock().unwrap().insert(db, err); -1 }
    }
}

#[no_mangle]
pub extern "C" fn sqlite_result_col_count(result: i64) -> i64 {
    results().lock().unwrap().get(&result).map(|r| r.col_names.len() as i64).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_row_count(result: i64) -> i64 {
    results().lock().unwrap().get(&result).map(|r| r.rows.len() as i64).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_col_name(result: i64, col: i64) -> *mut MvlString {
    let name = results()
        .lock()
        .unwrap()
        .get(&result)
        .and_then(|r| r.col_names.get(col as usize).cloned())
        .unwrap_or_default();
    new_mvl_str(&name)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_type(result: i64, row: i64, col: i64) -> i64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Null => 0, SqliteVal::Bool(_) => 1, SqliteVal::Int(_) => 2,
        SqliteVal::Float(_) => 3, SqliteVal::Text(_) => 4, SqliteVal::Blob(_) => 5,
    }).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_bool(result: i64, row: i64, col: i64) -> i8 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Bool(b) => *b as i8,
        SqliteVal::Int(n) => (*n != 0) as i8,
        _ => 0,
    }).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_int(result: i64, row: i64, col: i64) -> i64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Int(n) => *n,
        SqliteVal::Bool(b) => *b as i64,
        _ => 0,
    }).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_float(result: i64, row: i64, col: i64) -> f64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Float(f) => *f,
        SqliteVal::Int(n) => *n as f64,
        _ => 0.0,
    }).unwrap_or(0.0)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_text(result: i64, row: i64, col: i64) -> *mut MvlString {
    let s = with_cell(result, row, col, |v| match v {
        SqliteVal::Text(s) => s.clone(),
        _ => String::new(),
    }).unwrap_or_default();
    new_mvl_str(&s)
}

#[no_mangle]
pub extern "C" fn sqlite_result_cell_blob(result: i64, row: i64, col: i64) -> *mut MvlString {
    let s = with_cell(result, row, col, |v| match v {
        SqliteVal::Blob(b) => encode_blob(b),
        _ => String::new(),
    }).unwrap_or_default();
    new_mvl_str(&s)
}

#[no_mangle]
pub extern "C" fn sqlite_result_drop(result: i64) {
    results().lock().unwrap().remove(&result);
}
