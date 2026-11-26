//! Lua `db` API：脚本读写业务库（`data.db`）。
//!
//! - `db.query(sql, params?)`：只读 SQL，返回行数组（列名为 key，NULL 列缺省）
//! - `db.exec(sql, params?)`：写 SQL（INSERT/UPDATE/DELETE），返回 `{ changes, last_id }`
//! - `db.begin()`：开启事务，返回 tx 对象（`tx:query` / `tx:exec` / `tx:commit` / `tx:rollback`）
//!
//! 失败一律返回 `nil, err`（与 modbus 等 API 一致）。
//!
//! 权限：脚本在 `_manifest.lua` 里声明可读/写哪些表（见 [`DbPerm`]），未声明的表一律拒绝。
//! 校验不靠解析 SQL，而是在 SQLite 的 authorizer 回调里按**实际访问的表**拦截——视图、触发器、
//! 子查询里引用的表同样会被检查，不可能被 SQL 写法绕过。DDL、PRAGMA、ATTACH、手写事务语句一律拒绝。
//!
//! 所有写操作（含被拒绝/失败的）都会记录到 `t_operation_log`。

use std::{
    collections::{BTreeSet, HashSet},
    ffi::{CStr, c_char, c_int, c_void},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use futures_util::TryStreamExt;
use libsqlite3_sys as ffi;
use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};
use sqlx::{
    Column, Row, Sqlite, SqliteConnection, TypeInfo, ValueRef,
    pool::PoolConnection,
    query::Query,
    sqlite::{SqliteArguments, SqliteRow},
};

type Json = serde_json::Value;

/// 单条 SQL 的执行超时（含等待写锁），超时后连接会被丢弃而不是还回池
const OP_TIMEOUT: Duration = Duration::from_secs(10);
/// `db.query` 单次最多返回的行数，超过则报错，提示加 LIMIT
const MAX_ROWS: usize = 10_000;
/// 操作日志里 SQL+参数的最大字符数
const LOG_CONTENT_MAX: usize = 1000;

// ── 权限 ────────────────────────────────────────────────────────────────────

/// 脚本的表权限。表名一律小写，`"*"` 表示所有表。声明了写权限的表隐含读权限。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbPerm {
    read: HashSet<String>,
    write: HashSet<String>,
}

impl DbPerm {
    pub fn new(read: HashSet<String>, write: HashSet<String>) -> Self {
        Self { read, write }
    }

    pub(crate) fn can_write(&self, table: &str) -> bool {
        self.write.contains("*") || self.write.contains(table)
    }

    pub(crate) fn can_read(&self, table: &str) -> bool {
        self.read.contains("*") || self.read.contains(table) || self.can_write(table)
    }
}

// ── authorizer ──────────────────────────────────────────────────────────────

struct AuthCtx {
    perm: DbPerm,
    allow_write: bool,
    /// 本条语句实际写入的表（含触发器间接写入的）
    written: StdMutex<BTreeSet<String>>,
    /// 第一次被拒绝的原因，用于给脚本返回比 "not authorized" 更清楚的错误
    denied: StdMutex<Option<String>>,
}

impl AuthCtx {
    fn new(perm: &DbPerm, allow_write: bool) -> Arc<Self> {
        Arc::new(Self {
            perm: perm.clone(),
            allow_write,
            written: StdMutex::new(BTreeSet::new()),
            denied: StdMutex::new(None),
        })
    }

    fn deny(&self, msg: String) -> c_int {
        let mut d = self.denied.lock().unwrap_or_else(|e| e.into_inner());
        if d.is_none() {
            *d = Some(msg);
        }
        ffi::SQLITE_DENY
    }

    fn written_tables(&self) -> BTreeSet<String> {
        self.written
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn map_err(&self, e: sqlx::Error) -> String {
        match self
            .denied
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            Some(d) => d,
            None => e.to_string(),
        }
    }
}

fn is_schema_table(t: &str) -> bool {
    matches!(
        t,
        "sqlite_master" | "sqlite_schema" | "sqlite_temp_master" | "sqlite_temp_schema"
    )
}

unsafe extern "C" fn authorizer(
    user: *mut c_void,
    action: c_int,
    a1: *const c_char,
    _a2: *const c_char,
    _a3: *const c_char,
    _a4: *const c_char,
) -> c_int {
    // SAFETY: user 指向 `AuthCtx`，在 authorizer 装载期间由 `Armed` 的调用方持有 Arc 保证存活
    let ctx = unsafe { &*(user as *const AuthCtx) };
    let table = if a1.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(a1) }
                .to_string_lossy()
                .to_ascii_lowercase(),
        )
    };
    match action {
        ffi::SQLITE_SELECT | ffi::SQLITE_FUNCTION | ffi::SQLITE_RECURSIVE => ffi::SQLITE_OK,
        ffi::SQLITE_READ => match table {
            None => ffi::SQLITE_OK,
            Some(t) if t.is_empty() || is_schema_table(&t) => ffi::SQLITE_OK,
            Some(t) if ctx.perm.can_read(&t) => ffi::SQLITE_OK,
            Some(t) => ctx.deny(format!("无权限：表 {t} 未声明读权限")),
        },
        ffi::SQLITE_INSERT | ffi::SQLITE_UPDATE | ffi::SQLITE_DELETE => {
            let Some(t) = table else {
                return ctx.deny("无法确定写入的表".into());
            };
            if !ctx.allow_write {
                return ctx.deny(format!("db.query 只读，不能修改表 {t}（请用 db.exec）"));
            }
            if is_schema_table(&t) {
                return ctx.deny("不允许修改系统表".into());
            }
            if t == "sqlite_sequence" {
                return ffi::SQLITE_OK;
            }
            if ctx.perm.can_write(&t) {
                ctx.written
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(t);
                ffi::SQLITE_OK
            } else {
                ctx.deny(format!("无权限：表 {t} 未声明写权限"))
            }
        }
        ffi::SQLITE_TRANSACTION | ffi::SQLITE_SAVEPOINT => {
            ctx.deny("不允许手写事务语句，请使用 db.begin()".into())
        }
        other => ctx.deny(format!(
            "不允许的 SQL 操作（DDL/PRAGMA/ATTACH 等，code={other}）"
        )),
    }
}

/// 通过 sqlx 的句柄锁安装/清除 authorizer：持锁期间连接的后台线程保证空闲，不会和我们并发访问
async fn set_auth(
    conn: &mut SqliteConnection,
    ctx: Option<&Arc<AuthCtx>>,
) -> Result<(), sqlx::Error> {
    let mut handle = conn.lock_handle().await?;
    let raw = handle.as_raw_handle().as_ptr();
    unsafe {
        match ctx {
            Some(c) => {
                ffi::sqlite3_set_authorizer(raw, Some(authorizer), Arc::as_ptr(c) as *mut c_void)
            }
            None => ffi::sqlite3_set_authorizer(raw, None, std::ptr::null_mut()),
        };
    }
    Ok(())
}

/// authorizer 装载期间持有连接。若中途被取消（脚本热重载/超时）或清除失败，
/// authorizer 会残留在连接上，所以此时直接丢弃该连接，不还回池，避免污染 API 层的查询。
struct Armed<'a> {
    conn: &'a mut PoolConnection<Sqlite>,
    done: bool,
}

impl<'a> Armed<'a> {
    async fn new(
        conn: &'a mut PoolConnection<Sqlite>,
        ctx: &Arc<AuthCtx>,
    ) -> Result<Self, sqlx::Error> {
        let mut armed = Self { conn, done: true };
        // 先标记为未完成：装载失败/被取消时同样走丢弃连接的路径
        armed.done = false;
        set_auth(armed.conn, Some(ctx)).await?;
        Ok(armed)
    }

    async fn disarm(mut self) -> Result<(), sqlx::Error> {
        set_auth(self.conn, None).await?;
        self.done = true;
        Ok(())
    }
}

impl Drop for Armed<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.conn.close_on_drop();
        }
    }
}

// ── 执行 ────────────────────────────────────────────────────────────────────

/// 只允许单条语句：sqlx 会逐条执行 `a; b`，前一条已生效后一条才报错就不是原子的，
/// 所以在这里直接拒绝。简单扫描：跳过字符串/标识符引号和注释，其余位置出现 `;` 且其后还有内容即为多语句。
fn check_single_statement(sql: &str) -> Result<(), String> {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut seen_semi = false;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                if seen_semi {
                    return Err("只允许单条 SQL 语句".into());
                }
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
            }
            b'[' => {
                if seen_semi {
                    return Err("只允许单条 SQL 语句".into());
                }
                while i < b.len() && b[i] != b']' {
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b';' => seen_semi = true,
            c if !c.is_ascii_whitespace() && seen_semi => {
                return Err("只允许单条 SQL 语句".into());
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

fn bind_all<'q>(
    mut q: Query<'q, Sqlite, SqliteArguments<'q>>,
    params: &[Json],
) -> Query<'q, Sqlite, SqliteArguments<'q>> {
    for p in params {
        q = match p {
            Json::Null => q.bind(Option::<i64>::None),
            Json::Bool(b) => q.bind(*b),
            Json::Number(n) => match n.as_i64() {
                Some(i) => q.bind(i),
                None => q.bind(n.as_f64().unwrap_or(0.0)),
            },
            Json::String(s) => q.bind(s.clone()),
            other => q.bind(other.to_string()),
        };
    }
    q
}

async fn run_query(
    conn: &mut PoolConnection<Sqlite>,
    perm: &DbPerm,
    sql: &str,
    params: &[Json],
) -> Result<Vec<SqliteRow>, String> {
    let ctx = AuthCtx::new(perm, false);
    check_single_statement(sql)?;
    let armed = Armed::new(conn, &ctx).await.map_err(|e| ctx.map_err(e))?;
    let result: Result<Vec<SqliteRow>, String> = async {
        // persistent(false)：不进语句缓存，保证每次都重新 prepare 并过 authorizer，
        // 也避免脚本复用 API 层缓存过的语句而绕过权限检查
        let q = bind_all(sqlx::query(sql).persistent(false), params);
        let mut stream = q.fetch(&mut **armed.conn);
        let mut rows = Vec::new();
        while let Some(row) = stream.try_next().await.map_err(|e| ctx.map_err(e))? {
            if rows.len() >= MAX_ROWS {
                return Err(format!("结果超过 {MAX_ROWS} 行，请加 LIMIT 或缩小范围"));
            }
            rows.push(row);
        }
        Ok(rows)
    }
    .await;
    armed.disarm().await.map_err(|e| e.to_string())?;
    result
}

struct ExecOk {
    changes: u64,
    last_id: i64,
}

async fn run_exec(
    conn: &mut PoolConnection<Sqlite>,
    perm: &DbPerm,
    sql: &str,
    params: &[Json],
) -> (Result<ExecOk, String>, BTreeSet<String>) {
    let ctx = AuthCtx::new(perm, true);
    if let Err(e) = check_single_statement(sql) {
        return (Err(e), BTreeSet::new());
    }
    let armed = match Armed::new(conn, &ctx).await {
        Ok(a) => a,
        Err(e) => return (Err(ctx.map_err(e)), BTreeSet::new()),
    };
    let q = bind_all(sqlx::query(sql).persistent(false), params);
    let res = q.execute(&mut **armed.conn).await;
    let mapped = res
        .map(|r| ExecOk {
            changes: r.rows_affected(),
            last_id: r.last_insert_rowid(),
        })
        .map_err(|e| ctx.map_err(e));
    if let Err(e) = armed.disarm().await {
        return (Err(e.to_string()), ctx.written_tables());
    }
    (mapped, ctx.written_tables())
}

// ── 操作日志 ────────────────────────────────────────────────────────────────

struct LogEntry {
    target: String,
    content: String,
    result: String,
}

fn make_log(
    sql: &str,
    params: &[Json],
    written: &BTreeSet<String>,
    result: &Result<ExecOk, String>,
) -> LogEntry {
    let mut content = if params.is_empty() {
        sql.trim().to_string()
    } else {
        format!("{} | params={}", sql.trim(), Json::Array(params.to_vec()))
    };
    if content.chars().count() > LOG_CONTENT_MAX {
        content = content.chars().take(LOG_CONTENT_MAX).collect::<String>() + "…";
    }
    LogEntry {
        target: if written.is_empty() {
            "-".into()
        } else {
            written.iter().cloned().collect::<Vec<_>>().join(",")
        },
        content,
        result: match result {
            Ok(r) => format!("成功: 影响 {} 行", r.changes),
            Err(e) => format!("失败: {e}"),
        },
    }
}

async fn write_log(conn: &mut SqliteConnection, operator: &str, e: &LogEntry, suffix: &str) {
    let res = sqlx::query(
        "INSERT INTO t_operation_log (operator, op_type, target, content, result) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(operator)
    .bind("script_sql")
    .bind(&e.target)
    .bind(&e.content)
    .bind(format!("{}{}", e.result, suffix))
    .execute(conn)
    .await;
    if let Err(err) = res {
        tracing::warn!("[mod:db] 写入操作日志失败: {err}");
    }
}

// ── Lua 绑定 ────────────────────────────────────────────────────────────────

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn to_params(lua: &Lua, v: Option<Value>) -> mlua::Result<Vec<Json>> {
    match v {
        None | Some(Value::Nil) => Ok(vec![]),
        Some(v) => match lua.from_value::<Json>(v)? {
            Json::Array(a) => Ok(a),
            Json::Object(o) if o.is_empty() => Ok(vec![]),
            _ => Err(rt("params 必须是数组，如 {1, \"a\"}")),
        },
    }
}

fn row_to_lua(lua: &Lua, row: &SqliteRow) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(|e| rt(e.to_string()))?;
        if raw.is_null() {
            continue;
        }
        let ty = raw.type_info().name().to_ascii_uppercase();
        let v = match ty.as_str() {
            "INTEGER" | "INT" | "BIGINT" | "INT8" | "BOOLEAN" => Value::Integer(
                row.try_get_unchecked::<i64, _>(i)
                    .map_err(|e| rt(e.to_string()))?,
            ),
            "REAL" | "FLOAT" | "DOUBLE" | "NUMERIC" => Value::Number(
                row.try_get_unchecked::<f64, _>(i)
                    .map_err(|e| rt(e.to_string()))?,
            ),
            "BLOB" => Value::String(
                lua.create_string(
                    row.try_get_unchecked::<Vec<u8>, _>(i)
                        .map_err(|e| rt(e.to_string()))?,
                )?,
            ),
            _ => Value::String(
                lua.create_string(
                    row.try_get_unchecked::<String, _>(i)
                        .map_err(|e| rt(e.to_string()))?,
                )?,
            ),
        };
        t.set(col.name(), v)?;
    }
    Ok(t)
}

fn rows_to_lua(lua: &Lua, rows: &[SqliteRow]) -> mlua::Result<Table> {
    let out = lua.create_table_with_capacity(rows.len(), 0)?;
    for (i, row) in rows.iter().enumerate() {
        out.set(i + 1, row_to_lua(lua, row)?)?;
    }
    Ok(out)
}

fn exec_to_lua(lua: &Lua, r: &ExecOk) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("changes", r.changes as i64)?;
    t.set("last_id", r.last_id)?;
    Ok(t)
}

/// `(结果, nil)` 或 `(nil, 错误信息)`
fn ok_or_err(lua: &Lua, r: Result<Value, String>) -> mlua::Result<(Value, Value)> {
    match r {
        Ok(v) => Ok((v, Value::Nil)),
        Err(e) => Ok((Value::Nil, Value::String(lua.create_string(&e)?))),
    }
}

async fn acquire() -> Result<PoolConnection<Sqlite>, String> {
    let pool = collector_core::utils::database::get_database().map_err(|e| e.to_string())?;
    pool.acquire().await.map_err(|e| e.to_string())
}

async fn timed<T>(fut: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    match tokio::time::timeout(OP_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(format!("数据库操作超时（{}s）", OP_TIMEOUT.as_secs())),
    }
}

/// 事务状态：持有一条独占连接。未提交就被丢弃（脚本忘了 commit、热重载等）时，
/// 直接关闭该连接，SQLite 会自动回滚未提交内容。
struct TxState {
    conn: Option<PoolConnection<Sqlite>>,
    pending: Vec<LogEntry>,
}

impl Drop for TxState {
    fn drop(&mut self) {
        if let Some(c) = self.conn.as_mut() {
            c.close_on_drop();
            tracing::warn!(
                "[mod:db] 事务未提交/回滚即被丢弃，已自动回滚（{} 条语句）",
                self.pending.len()
            );
        }
    }
}

struct Tx {
    perm: DbPerm,
    operator: String,
    state: Arc<tokio::sync::Mutex<TxState>>,
}

const TX_CLOSED: &str = "事务已结束";

impl UserData for Tx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method(
            "query",
            |lua, this, (sql, params): (String, Option<Value>)| async move {
                let params = to_params(&lua, params)?;
                let mut st = this.state.lock().await;
                let r = match st.conn.as_mut() {
                    None => Err(TX_CLOSED.to_string()),
                    Some(conn) => timed(run_query(conn, &this.perm, &sql, &params)).await,
                };
                drop(st);
                ok_or_err(
                    &lua,
                    r.and_then(|rows| {
                        rows_to_lua(&lua, &rows)
                            .map(Value::Table)
                            .map_err(|e| e.to_string())
                    }),
                )
            },
        );

        methods.add_async_method(
            "exec",
            |lua, this, (sql, params): (String, Option<Value>)| async move {
                let params = to_params(&lua, params)?;
                let mut st = this.state.lock().await;
                let Some(conn) = st.conn.as_mut() else {
                    drop(st);
                    return ok_or_err(&lua, Err(TX_CLOSED.into()));
                };
                let (res, written) = match tokio::time::timeout(
                    OP_TIMEOUT,
                    run_exec(conn, &this.perm, &sql, &params),
                )
                .await
                {
                    Ok(v) => v,
                    Err(_) => (
                        Err(format!("数据库操作超时（{}s）", OP_TIMEOUT.as_secs())),
                        BTreeSet::new(),
                    ),
                };
                st.pending.push(make_log(&sql, &params, &written, &res));
                drop(st);
                ok_or_err(
                    &lua,
                    res.and_then(|r| {
                        exec_to_lua(&lua, &r)
                            .map(Value::Table)
                            .map_err(|e| e.to_string())
                    }),
                )
            },
        );

        methods.add_async_method("commit", |lua, this, ()| async move {
            let mut st = this.state.lock().await;
            let Some(mut conn) = st.conn.take() else {
                drop(st);
                return ok_or_err(&lua, Err(TX_CLOSED.into()));
            };
            // 日志和业务写入在同一个事务里提交，保证"有写入必有日志"
            let pending = std::mem::take(&mut st.pending);
            drop(st);
            for e in &pending {
                write_log(&mut conn, &this.operator, e, "").await;
            }
            let r = sqlx::query("COMMIT").execute(&mut *conn).await;
            match r {
                Ok(_) => ok_or_err(&lua, Ok(Value::Boolean(true))),
                Err(e) => {
                    conn.close_on_drop();
                    ok_or_err(&lua, Err(format!("提交失败: {e}")))
                }
            }
        });

        methods.add_async_method("rollback", |lua, this, ()| async move {
            let mut st = this.state.lock().await;
            let Some(mut conn) = st.conn.take() else {
                drop(st);
                return ok_or_err(&lua, Err(TX_CLOSED.into()));
            };
            let pending = std::mem::take(&mut st.pending);
            drop(st);
            if let Err(e) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                conn.close_on_drop();
                return ok_or_err(&lua, Err(format!("回滚失败: {e}")));
            }
            // 回滚后补记日志：被回滚的语句也要留痕，便于审计
            for e in &pending {
                write_log(&mut conn, &this.operator, e, "（事务已回滚）").await;
            }
            ok_or_err(&lua, Ok(Value::Boolean(true)))
        });
    }
}

pub fn create_db_table(lua: &Lua, perm: DbPerm, script: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let operator = format!("lua:{script}");

    {
        let perm = perm.clone();
        table.set(
            "query",
            lua.create_async_function(move |lua, (sql, params): (String, Option<Value>)| {
                let perm = perm.clone();
                async move {
                    let params = to_params(&lua, params)?;
                    let r = timed(async {
                        let mut conn = acquire().await?;
                        run_query(&mut conn, &perm, &sql, &params).await
                    })
                    .await;
                    ok_or_err(
                        &lua,
                        r.and_then(|rows| {
                            rows_to_lua(&lua, &rows)
                                .map(Value::Table)
                                .map_err(|e| e.to_string())
                        }),
                    )
                }
            })?,
        )?;
    }

    {
        let perm = perm.clone();
        let operator = operator.clone();
        table.set(
            "exec",
            lua.create_async_function(move |lua, (sql, params): (String, Option<Value>)| {
                let perm = perm.clone();
                let operator = operator.clone();
                async move {
                    let params = to_params(&lua, params)?;
                    let r = timed(async {
                        let mut conn = acquire().await?;
                        let (res, written) = run_exec(&mut conn, &perm, &sql, &params).await;
                        let entry = make_log(&sql, &params, &written, &res);
                        write_log(&mut conn, &operator, &entry, "").await;
                        res
                    })
                    .await;
                    ok_or_err(
                        &lua,
                        r.and_then(|r| {
                            exec_to_lua(&lua, &r)
                                .map(Value::Table)
                                .map_err(|e| e.to_string())
                        }),
                    )
                }
            })?,
        )?;
    }

    {
        table.set(
            "begin",
            lua.create_async_function(move |lua, ()| {
                let perm = perm.clone();
                let operator = operator.clone();
                async move {
                    let r = timed(async {
                        let mut conn = acquire().await?;
                        sqlx::query("BEGIN")
                            .execute(&mut *conn)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(conn)
                    })
                    .await;
                    match r {
                        Ok(conn) => {
                            let tx = Tx {
                                perm,
                                operator,
                                state: Arc::new(tokio::sync::Mutex::new(TxState {
                                    conn: Some(conn),
                                    pending: Vec::new(),
                                })),
                            };
                            Ok((Value::UserData(lua.create_userdata(tx)?), Value::Nil))
                        }
                        Err(e) => ok_or_err(&lua, Err(e)),
                    }
                }
            })?,
        )?;
    }

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn perm(read: &[&str], write: &[&str]) -> DbPerm {
        DbPerm::new(set(read), set(write))
    }

    /// 单连接内存库：authorizer 装在连接上，单连接便于同时验证"用完是否清除"
    async fn test_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE t_a (id INTEGER PRIMARY KEY AUTOINCREMENT, v INTEGER, note TEXT, f REAL)",
            "CREATE TABLE t_b (id INTEGER PRIMARY KEY AUTOINCREMENT, v INTEGER)",
            "CREATE TABLE t_secret (id INTEGER PRIMARY KEY, pwd TEXT)",
            "CREATE TABLE t_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, msg TEXT)",
            "CREATE TABLE t_operation_log (id INTEGER PRIMARY KEY AUTOINCREMENT, operator TEXT NOT NULL, \
             op_type TEXT NOT NULL, target TEXT NOT NULL, content TEXT NOT NULL, result TEXT NOT NULL, \
             created_at TIMESTAMP DEFAULT (datetime('now','localtime')))",
            "CREATE VIEW v_secret AS SELECT id FROM t_secret",
            // 写 t_b 会触发间接写 t_audit
            "CREATE TRIGGER trg_b AFTER INSERT ON t_b BEGIN INSERT INTO t_audit(msg) VALUES ('x'); END",
            "INSERT INTO t_a (v, note, f) VALUES (1, 'one', 1.5), (2, NULL, NULL)",
            "INSERT INTO t_secret VALUES (1, 'hash')",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn query(pool: &sqlx::SqlitePool, p: &DbPerm, sql: &str, params: &[Json]) -> Result<usize, String> {
        let mut conn = pool.acquire().await.unwrap();
        run_query(&mut conn, p, sql, params).await.map(|r| r.len())
    }

    async fn exec(pool: &sqlx::SqlitePool, p: &DbPerm, sql: &str, params: &[Json]) -> (Result<u64, String>, BTreeSet<String>) {
        let mut conn = pool.acquire().await.unwrap();
        let (r, w) = run_exec(&mut conn, p, sql, params).await;
        (r.map(|o| o.changes), w)
    }

    // ── DbPerm ──────────────────────────────────────────────────────────────

    #[test]
    fn perm_default_denies_everything() {
        let p = DbPerm::default();
        assert!(!p.can_read("t_a"));
        assert!(!p.can_write("t_a"));
    }

    #[test]
    fn perm_write_implies_read_but_not_vice_versa() {
        let p = perm(&["t_a"], &["t_b"]);
        assert!(p.can_read("t_a") && !p.can_write("t_a"));
        assert!(p.can_read("t_b") && p.can_write("t_b"));
        assert!(!p.can_read("t_c"));
    }

    #[test]
    fn perm_wildcard() {
        let p = perm(&["*"], &[]);
        assert!(p.can_read("anything") && !p.can_write("anything"));
        let p = perm(&[], &["*"]);
        assert!(p.can_read("anything") && p.can_write("anything"));
    }

    // ── 单语句检查 ──────────────────────────────────────────────────────────

    #[test]
    fn single_statement_accepts_normal_sql() {
        for sql in [
            "select 1",
            "select 1;",
            "select 1;  \n ",
            "select ';' as x",
            "select \"a;b\" from t",
            "select [a;b] from t",
            "select `a;b` from t",
            "select 1 -- ; drop table t",
            "select 1 /* ; drop table t */",
            "select 'it''s;ok'",
        ] {
            assert!(check_single_statement(sql).is_ok(), "应放行: {sql}");
        }
    }

    #[test]
    fn single_statement_rejects_multi() {
        for sql in [
            "select 1; select 2",
            "select 1;select 2",
            "update t set a=1; drop table t",
            "select 1; 'x'",
            "select 1; \"x\"",
            "select 1; [x]",
            "select 1; -- c\n select 2",
            "select 1; /* c */ select 2",
        ] {
            assert!(check_single_statement(sql).is_err(), "应拒绝: {sql}");
        }
    }

    // ── query：读权限 ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn query_allowed_table() {
        let pool = test_pool().await;
        assert_eq!(query(&pool, &perm(&["t_a"], &[]), "select * from t_a", &[]).await, Ok(2));
    }

    #[tokio::test]
    async fn query_without_table_needs_no_perm() {
        let pool = test_pool().await;
        assert_eq!(query(&pool, &DbPerm::default(), "select 1", &[]).await, Ok(1));
    }

    #[tokio::test]
    async fn query_denied_table() {
        let pool = test_pool().await;
        let e = query(&pool, &perm(&["t_a"], &[]), "select * from t_secret", &[]).await.unwrap_err();
        assert!(e.contains("t_secret") && e.contains("读权限"), "{e}");
    }

    #[tokio::test]
    async fn query_denied_via_subquery_join_and_view() {
        let pool = test_pool().await;
        let p = perm(&["t_a"], &[]);
        for sql in [
            "select * from t_a where id in (select id from t_secret)",
            "select * from t_a join t_secret on t_a.id = t_secret.id",
            "select * from v_secret",
            "with c as (select * from t_secret) select * from c",
            "select * from t_a union select id, pwd, null, null from t_secret",
        ] {
            assert!(query(&pool, &p, sql, &[]).await.is_err(), "应拒绝: {sql}");
        }
    }

    #[tokio::test]
    async fn query_case_insensitive_table_name() {
        let pool = test_pool().await;
        assert_eq!(query(&pool, &perm(&["t_a"], &[]), "select * from T_A", &[]).await, Ok(2));
    }

    #[tokio::test]
    async fn query_is_read_only() {
        let pool = test_pool().await;
        let p = perm(&["*"], &["*"]); // 即使有写权限，query 也不能写
        for sql in ["delete from t_a", "update t_a set v = 9", "insert into t_a(v) values (1)"] {
            let e = query(&pool, &p, sql, &[]).await.unwrap_err();
            assert!(e.contains("只读"), "{sql}: {e}");
        }
        assert_eq!(query(&pool, &p, "select * from t_a", &[]).await, Ok(2));
    }

    #[tokio::test]
    async fn query_params_binding_blocks_injection() {
        let pool = test_pool().await;
        let p = perm(&["t_a"], &[]);
        let evil = Json::String("1 or 1=1".into());
        assert_eq!(query(&pool, &p, "select * from t_a where id = ?", &[evil]).await, Ok(0));
        assert_eq!(query(&pool, &p, "select * from t_a where id = ?", &[Json::from(1)]).await, Ok(1));
    }

    #[tokio::test]
    async fn query_row_limit() {
        let pool = test_pool().await;
        let sql = format!("with recursive c(n) as (select 1 union all select n+1 from c where n < {}) select n from c", MAX_ROWS + 1);
        // 递归 CTE 不访问用户表，不需要表权限
        let e = query(&pool, &DbPerm::default(), &sql, &[]).await.unwrap_err();
        assert!(e.contains("超过"), "{e}");
    }

    // ── exec：写权限 ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn exec_allowed_and_reports_tables() {
        let pool = test_pool().await;
        let p = perm(&[], &["t_a"]);
        let (r, w) = exec(&pool, &p, "update t_a set v = ? where id = 1", &[Json::from(7)]).await;
        assert_eq!(r, Ok(1));
        assert_eq!(w, BTreeSet::from(["t_a".to_string()]));
        assert_eq!(query(&pool, &p, "select * from t_a where v = 7", &[]).await, Ok(1));
    }

    #[tokio::test]
    async fn exec_insert_returns_last_id() {
        let pool = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let r = run_exec(&mut conn, &perm(&[], &["t_a"]), "insert into t_a(v) values (5)", &[]).await.0.unwrap();
        assert_eq!((r.changes, r.last_id), (1, 3));
    }

    #[tokio::test]
    async fn exec_denied_write_table_leaves_data_untouched() {
        let pool = test_pool().await;
        let p = perm(&["t_a"], &["t_b"]); // t_a 只读
        let (r, _) = exec(&pool, &p, "update t_a set v = 99", &[]).await;
        assert!(r.unwrap_err().contains("写权限"));
        assert_eq!(query(&pool, &perm(&["t_a"], &[]), "select * from t_a where v = 99", &[]).await, Ok(0));
    }

    #[tokio::test]
    async fn exec_write_needs_read_of_referenced_tables() {
        let pool = test_pool().await;
        // 写 t_a 的同时从 t_secret 读 → 需要 t_secret 的读权限
        let p = perm(&[], &["t_a"]);
        let (r, _) = exec(&pool, &p, "insert into t_a(v) select id from t_secret", &[]).await;
        assert!(r.unwrap_err().contains("t_secret"));
        let p = perm(&["t_secret"], &["t_a"]);
        let (r, _) = exec(&pool, &p, "insert into t_a(v) select id from t_secret", &[]).await;
        assert_eq!(r, Ok(1));
    }

    #[tokio::test]
    async fn exec_trigger_indirect_write_is_checked_and_reported() {
        let pool = test_pool().await;
        // 只有 t_b 写权限：触发器会间接写 t_audit，必须被拒绝
        let (r, _) = exec(&pool, &perm(&[], &["t_b"]), "insert into t_b(v) values (1)", &[]).await;
        assert!(r.unwrap_err().contains("t_audit"));
        // 整条语句回滚，t_b 不应有数据
        assert_eq!(query(&pool, &perm(&["t_b"], &[]), "select * from t_b", &[]).await, Ok(0));
        // 两张都授权：成功且 written 里含间接写入的表
        let (r, w) = exec(&pool, &perm(&[], &["t_b", "t_audit"]), "insert into t_b(v) values (1)", &[]).await;
        assert_eq!(r, Ok(1));
        assert_eq!(w, BTreeSet::from(["t_audit".to_string(), "t_b".to_string()]));
    }

    #[tokio::test]
    async fn exec_rejects_ddl_pragma_attach_and_txn_statements() {
        let pool = test_pool().await;
        let p = perm(&["*"], &["*"]);
        for sql in [
            "drop table t_a",
            "create table t_x(id integer)",
            "alter table t_a add column z integer",
            "create index i on t_a(v)",
            "create trigger tr after insert on t_a begin select 1; end",
            "pragma writable_schema = 1",
            "attach database ':memory:' as x",
            "vacuum",
            "begin",
            "commit",
            "savepoint s",
            "delete from sqlite_master",
        ] {
            let (r, _) = exec(&pool, &p, sql, &[]).await;
            assert!(r.is_err(), "应拒绝: {sql}");
        }
        // 表仍然完好
        assert_eq!(query(&pool, &p, "select * from t_a", &[]).await, Ok(2));
    }

    #[tokio::test]
    async fn exec_rejects_multi_statement_atomically() {
        let pool = test_pool().await;
        let p = perm(&[], &["*"]);
        let (r, _) = exec(&pool, &p, "update t_a set v = 100; update t_a set v = 200", &[]).await;
        assert!(r.unwrap_err().contains("单条"));
        assert_eq!(query(&pool, &perm(&["t_a"], &[]), "select * from t_a where v in (100,200)", &[]).await, Ok(0));
    }

    #[tokio::test]
    async fn failed_exec_does_not_leave_authorizer_on_connection() {
        let pool = test_pool().await;
        // 先用受限权限失败一次，再用本连接（池里只有一条）做一次不受限的原生操作，
        // 若 authorizer 残留，这里的 DDL 会失败
        let (r, _) = exec(&pool, &perm(&[], &[]), "update t_a set v = 1", &[]).await;
        assert!(r.is_err());
        sqlx::query("create table t_after (id integer)").execute(&pool).await.unwrap();
        sqlx::query("select * from t_secret").fetch_all(&pool).await.unwrap();
        // 成功路径也一样
        let _ = query(&pool, &perm(&["t_a"], &[]), "select * from t_a", &[]).await;
        sqlx::query("select * from t_secret").fetch_all(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn params_types_roundtrip() {
        let pool = test_pool().await;
        let p = perm(&["t_a"], &["t_a"]);
        let (r, _) = exec(
            &pool,
            &p,
            "insert into t_a(v, note, f) values (?, ?, ?)",
            &[Json::from(42), Json::from("héllo"), Json::from(2.25)],
        )
        .await;
        assert_eq!(r, Ok(1));
        let mut conn = pool.acquire().await.unwrap();
        let rows = run_query(&mut conn, &p, "select v, note, f from t_a where id = 3", &[]).await.unwrap();
        let lua = Lua::new();
        let t = row_to_lua(&lua, &rows[0]).unwrap();
        assert_eq!(t.get::<i64>("v").unwrap(), 42);
        assert_eq!(t.get::<String>("note").unwrap(), "héllo");
        assert_eq!(t.get::<f64>("f").unwrap(), 2.25);
    }

    #[tokio::test]
    async fn null_columns_are_absent_in_lua_row() {
        let pool = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let rows = run_query(&mut conn, &perm(&["t_a"], &[]), "select v, note, f from t_a where id = 2", &[]).await.unwrap();
        let lua = Lua::new();
        let t = row_to_lua(&lua, &rows[0]).unwrap();
        assert_eq!(t.get::<i64>("v").unwrap(), 2);
        assert!(matches!(t.get::<Value>("note").unwrap(), Value::Nil));
        assert!(matches!(t.get::<Value>("f").unwrap(), Value::Nil));
    }

    // ── 操作日志 ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn make_log_formats_success_failure_and_truncates() {
        let ok: Result<ExecOk, String> = Ok(ExecOk { changes: 3, last_id: 0 });
        let w = BTreeSet::from(["t_a".to_string(), "t_b".to_string()]);
        let e = make_log("update t_a set v=?", &[Json::from(1)], &w, &ok);
        assert_eq!(e.target, "t_a,t_b");
        assert!(e.content.contains("params=[1]") && e.result.contains("影响 3 行"));

        let bad: Result<ExecOk, String> = Err("无权限".into());
        let e = make_log("drop table x", &[], &BTreeSet::new(), &bad);
        assert_eq!(e.target, "-");
        assert!(e.result.starts_with("失败") && e.result.contains("无权限"));

        let long = format!("select '{}'", "a".repeat(5000));
        let e = make_log(&long, &[], &BTreeSet::new(), &ok);
        assert_eq!(e.content.chars().count(), LOG_CONTENT_MAX + 1);
        assert!(e.content.ends_with('…'));
    }

    #[tokio::test]
    async fn write_log_inserts_row() {
        let pool = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let entry = LogEntry { target: "t_a".into(), content: "update ...".into(), result: "成功: 影响 1 行".into() };
        write_log(&mut conn, "lua:demo.lua", &entry, "（事务已回滚）").await;
        drop(conn);
        let row: (String, String, String, String) =
            sqlx::query_as("select operator, op_type, target, result from t_operation_log")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "lua:demo.lua");
        assert_eq!(row.1, "script_sql");
        assert_eq!(row.2, "t_a");
        assert!(row.3.ends_with("（事务已回滚）"));
    }

    /// 走真实 Lua 的端到端用例：覆盖 db.query / db.exec / 事务提交与回滚 / 遗弃事务 / 操作日志。
    /// `get_database()` 是进程级 OnceLock，因此全库只能有这一个用例初始化全局连接池。
    #[tokio::test]
    async fn lua_end_to_end_with_transactions_and_audit_log() {
        use collector_core::utils::database::{DatabaseConfig, init_database};
        let path = std::env::temp_dir().join(format!("collector-db-e2e-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let pool = init_database(DatabaseConfig {
            path: path.to_string_lossy().into(),
            max_connections: 2,
            min_connections: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        for ddl in [
            "CREATE TABLE t_a (id INTEGER PRIMARY KEY AUTOINCREMENT, v INTEGER)",
            "CREATE TABLE t_secret (id INTEGER PRIMARY KEY)",
            "CREATE TABLE t_operation_log (id INTEGER PRIMARY KEY AUTOINCREMENT, operator TEXT NOT NULL, \
             op_type TEXT NOT NULL, target TEXT NOT NULL, content TEXT NOT NULL, result TEXT NOT NULL, \
             created_at TIMESTAMP DEFAULT (datetime('now','localtime')))",
            "INSERT INTO t_a(v) VALUES (1)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }

        let lua = Lua::new();
        lua.globals()
            .set("db", create_db_table(&lua, perm(&[], &["t_a"]), "e2e.lua".into()).unwrap())
            .unwrap();
        lua.load(
            r#"
            local function v()
              local r = assert(db.query("select v from t_a where id = 1"))
              return r[1].v
            end
            -- 非事务写
            local r = assert(db.exec("update t_a set v = ? where id = 1", {10}))
            assert(r.changes == 1 and v() == 10)
            -- 越权：读未声明表 / 写未声明表
            local rows, e = db.query("select * from t_secret"); assert(rows == nil and e:find("t_secret"))
            local r, e = db.exec("insert into t_secret values (1)"); assert(r == nil and e:find("写权限"))
            -- 事务回滚
            local tx = assert(db.begin())
            assert(tx:exec("update t_a set v = 20 where id = 1"))
            assert(tx:query("select v from t_a where id = 1")[1].v == 20)
            assert(tx:rollback())
            assert(v() == 10)
            -- 结束后的事务不可再用
            local r, e = tx:exec("update t_a set v = 1"); assert(r == nil and e:find("已结束"))
            local r, e = tx:commit(); assert(r == nil and e:find("已结束"))
            -- 事务提交
            local tx = assert(db.begin())
            assert(tx:exec("update t_a set v = 30 where id = 1"))
            assert(tx:commit())
            assert(v() == 30)
            -- 事务内被拒绝的语句不影响事务
            local tx = assert(db.begin())
            local r, e = tx:exec("delete from t_secret"); assert(r == nil)
            assert(tx:exec("update t_a set v = 40 where id = 1"))
            assert(tx:commit())
            assert(v() == 40)
            -- 参数必须是数组
            local ok, err = pcall(db.query, "select 1", {a = 1}); assert(not ok)
            "#,
        )
        .exec_async()
        .await
        .unwrap();

        // 遗弃事务：丢掉 Lua 值并 GC 后应回滚，且连接池里不残留被占用/带 authorizer 的连接
        lua.load(
            r#"
            do
              local tx = assert(db.begin())
              assert(tx:exec("update t_a set v = 999 where id = 1"))
            end
            collectgarbage(); collectgarbage()
            "#,
        )
        .exec_async()
        .await
        .unwrap();
        let v: (i64,) = sqlx::query_as("select v from t_a where id = 1").fetch_one(&pool).await.unwrap();
        assert_eq!(v.0, 40, "遗弃的事务应已回滚");
        // 两条连接都能正常做不受限的操作（authorizer 未残留）
        let mut a = pool.acquire().await.unwrap();
        let mut b = pool.acquire().await.unwrap();
        sqlx::query("select * from t_secret").fetch_all(&mut *a).await.unwrap();
        sqlx::query("select * from t_secret").fetch_all(&mut *b).await.unwrap();
        drop((a, b));

        // 操作日志：每次 db.exec / tx:exec 都有记录，含被拒绝的和回滚的
        let logs: Vec<(String, String, String, String)> = sqlx::query_as(
            "select operator, target, content, result from t_operation_log order by id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(logs.iter().all(|l| l.0 == "lua:e2e.lua"));
        let find = |needle: &str| logs.iter().find(|l| l.2.contains(needle)).unwrap_or_else(|| panic!("缺日志: {needle} in {logs:?}"));
        let l = find("set v = ? where id = 1");
        assert!(l.1 == "t_a" && l.2.contains("params=[10]") && l.3.starts_with("成功"));
        assert!(find("insert into t_secret").3.starts_with("失败"));
        assert!(find("set v = 20").3.contains("事务已回滚"));
        assert!(find("set v = 30").3.starts_with("成功"));
        assert!(find("delete from t_secret").3.starts_with("失败"));
        // 遗弃的事务从未执行 commit/rollback，不产生日志，也不应改动数据（上面已断言 v == 40）
        assert!(logs.iter().all(|l| !l.2.contains("set v = 999")));
        drop(pool);
        let _ = std::fs::remove_file(&path);
    }
}
