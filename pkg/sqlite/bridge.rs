//! bridge.rs — pkg.sqlite Rust backend
//!
//! Implements the `extern "rust"` functions declared in src/internal/ffi.mvl.
//! Uses rusqlite with a bundled SQLite library (no system sqlite3 required).
//!
//! # Handle tables (all OnceLock + Mutex)
//!
//!   CONNECTIONS  i64 → rusqlite::Connection
//!   PARAM_BUFS   i64 → Vec<SqliteVal>   (accumulated params, keyed by db handle)
//!   RESULTS      i64 → QueryResult       (materialised query rows)
//!   ERRORS       i64 → (errno, msg)      (last error per db; -1 = last open failure)
//!
//! See #785.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::types::{Value as RValue, ValueRef};

#[repr(transparent)]
pub struct Clean<T>(pub T);

// ── Internal value type ───────────────────────────────────────────────────────

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

// ── Error classification ──────────────────────────────────────────────────────

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

// ── Blob encoding ─────────────────────────────────────────────────────────────

fn decode_blob(csv: &str) -> Vec<u8> {
    if csv.is_empty() {
        return Vec::new();
    }
    csv.split(',')
        .filter_map(|s| s.trim().parse::<u8>().ok())
        .collect()
}

fn encode_blob(b: &[u8]) -> String {
    b.iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

// ── Val conversion ────────────────────────────────────────────────────────────

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

// ── Connection ────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn sqlite_open(path: String) -> i64 {
    match rusqlite::Connection::open(&path) {
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
pub extern "Rust" fn sqlite_close(db: i64) {
    connections().lock().unwrap().remove(&db);
    param_bufs().lock().unwrap().remove(&db);
    errors().lock().unwrap().remove(&db);
}

#[no_mangle]
pub extern "Rust" fn sqlite_errmsg(db: i64) -> String {
    errors()
        .lock()
        .unwrap()
        .get(&db)
        .map(|(_, m)| m.clone())
        .unwrap_or_default()
}

#[no_mangle]
pub extern "Rust" fn sqlite_errno(db: i64) -> i64 {
    errors()
        .lock()
        .unwrap()
        .get(&db)
        .map(|(c, _)| *c)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "Rust" fn sqlite_changes(db: i64) -> i64 {
    connections()
        .lock()
        .unwrap()
        .get(&db)
        .map(|c| c.changes() as i64)
        .unwrap_or(0)
}

// ── Parameter binding ─────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn sqlite_param_reset(db: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().clear();
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_null(db: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Null);
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_bool(db: i64, v: bool) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Bool(v));
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_int(db: i64, v: i64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Int(v));
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_float(db: i64, v: f64) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Float(v));
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_text(db: i64, v: String) {
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Text(v));
}

#[no_mangle]
pub extern "Rust" fn sqlite_param_blob(db: i64, v: String) {
    let bytes = decode_blob(&v);
    param_bufs().lock().unwrap().entry(db).or_default().push(SqliteVal::Blob(bytes));
}

// ── Execute ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn sqlite_execute(db: i64, sql: Clean<String>) -> i64 {
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
        let Some(conn) = guard.get(&db) else {
            return -1;
        };
        conn.execute(&sql.0, rusqlite::params_from_iter(params.iter()))
            .map(|n| n as i64)
            .map_err(|e| classify(&e))
    };

    match result {
        Ok(n) => n,
        Err(err) => {
            errors().lock().unwrap().insert(db, err);
            -1
        }
    }
}

// ── Query ─────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn sqlite_query(db: i64, sql: Clean<String>) -> i64 {
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
        let conn = guard
            .get(&db)
            .ok_or_else(|| (4i64, "invalid db handle".to_string()))?;
        let mut stmt = conn.prepare(&sql.0).map_err(|e| classify(&e))?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let n_cols = col_names.len();
        let mut rows: Vec<Vec<SqliteVal>> = Vec::new();
        let mut iter = stmt
            .query(rusqlite::params_from_iter(params.iter()))
            .map_err(|e| classify(&e))?;
        while let Some(row) = iter.next().map_err(|e| classify(&e))? {
            let cells: Vec<SqliteVal> = (0..n_cols)
                .map(|i| row.get_ref(i).map(from_ref).unwrap_or(SqliteVal::Null))
                .collect();
            rows.push(cells);
        }
        Ok(QueryResult { col_names, rows })
    })();

    match result {
        Ok(qr) => {
            let rh = next_handle();
            results().lock().unwrap().insert(rh, qr);
            rh
        }
        Err(err) => {
            errors().lock().unwrap().insert(db, err);
            -1
        }
    }
}

// ── Result inspection ─────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn sqlite_result_col_count(result: i64) -> i64 {
    results()
        .lock()
        .unwrap()
        .get(&result)
        .map(|r| r.col_names.len() as i64)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_row_count(result: i64) -> i64 {
    results()
        .lock()
        .unwrap()
        .get(&result)
        .map(|r| r.rows.len() as i64)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_col_name(result: i64, col: i64) -> String {
    results()
        .lock()
        .unwrap()
        .get(&result)
        .and_then(|r| r.col_names.get(col as usize).cloned())
        .unwrap_or_default()
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_type(result: i64, row: i64, col: i64) -> i64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Null => 0,
        SqliteVal::Bool(_) => 1,
        SqliteVal::Int(_) => 2,
        SqliteVal::Float(_) => 3,
        SqliteVal::Text(_) => 4,
        SqliteVal::Blob(_) => 5,
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_bool(result: i64, row: i64, col: i64) -> bool {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Bool(b) => *b,
        SqliteVal::Int(n) => *n != 0,
        _ => false,
    })
    .unwrap_or(false)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_int(result: i64, row: i64, col: i64) -> i64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Int(n) => *n,
        SqliteVal::Bool(b) => *b as i64,
        _ => 0,
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_float(result: i64, row: i64, col: i64) -> f64 {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Float(f) => *f,
        SqliteVal::Int(n) => *n as f64,
        _ => 0.0,
    })
    .unwrap_or(0.0)
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_text(result: i64, row: i64, col: i64) -> String {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Text(s) => s.clone(),
        _ => String::new(),
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_cell_blob(result: i64, row: i64, col: i64) -> String {
    with_cell(result, row, col, |v| match v {
        SqliteVal::Blob(b) => encode_blob(b),
        _ => String::new(),
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "Rust" fn sqlite_result_drop(result: i64) {
    results().lock().unwrap().remove(&result);
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn with_cell<T>(result: i64, row: i64, col: i64, f: impl Fn(&SqliteVal) -> T) -> Option<T> {
    let guard = results().lock().unwrap();
    let cell = guard
        .get(&result)?
        .rows
        .get(row as usize)?
        .get(col as usize)?;
    Some(f(cell))
}
