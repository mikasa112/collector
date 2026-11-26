//! Lua `db` API 隔离验证：用 data.db 的临时副本，检验表权限、只读/写分离、事务、操作日志。
//! 用法：cargo run -p collector-engine --example db_script_check
use collector_core::utils::database::{DatabaseConfig, init_database};
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let dir = std::env::temp_dir().join(format!("collector-db-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let db_path = dir.join("test.db");
    tokio::fs::copy("data.db", &db_path).await.expect("需在仓库根目录运行");
    let pool = init_database(DatabaseConfig {
        path: db_path.to_string_lossy().into(),
        ..Default::default()
    })
    .await
    .unwrap();

    tokio::fs::write(
        dir.join("_manifest.lua"),
        r#"return {
  { file = "t.lua", db = { read = {"t_plan_curve_master"}, write = {"t_plan_curve_detail", "t_alarm"} } },
  "nodecl.lua",
}"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.join("nodecl.lua"),
        r#"MOD = { name = "nodecl" }
task.spawn(function()
  local r, e = db.query("select 1 as x"); log.info("nodecl select 1 => " .. tostring(r and r[1].x) .. " err=" .. tostring(e))
  local r, e = db.query("select * from t_alarm limit 1"); log.info("nodecl 读未声明表(应拒绝) => " .. tostring(r) .. " err=" .. tostring(e))
end)"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.join("t.lua"),
        r#"MOD = { name = "t" }
local function show(tag, a, b) log.info(tag .. " => " .. json.encode(a) .. " | err=" .. tostring(b)) end
task.spawn(function()
  show("读已声明表", db.query("select id, curve_name, priority, valid_start_date from t_plan_curve_master limit 2"))
  show("params 绑定", db.query("select count(*) as n from t_plan_curve_detail where curve_id = ?", {10}))
  show("读未声明表(应拒绝)", db.query("select * from t_user"))
  show("子查询里的未声明表(应拒绝)", db.query("select * from t_plan_curve_master where id in (select id from t_user)"))
  show("query 里写(应拒绝)", db.query("delete from t_plan_curve_detail"))
  show("写未授权表(应拒绝)", db.exec("update t_plan_curve_master set priority = 9"))
  show("DDL(应拒绝)", db.exec("drop table t_alarm"))
  show("PRAGMA(应拒绝)", db.exec("pragma writable_schema = 1"))
  show("手写事务(应拒绝)", db.exec("begin"))
  show("多语句只执行首条", db.exec("update t_plan_curve_detail set power_value = power_value where id = 129; drop table t_alarm"))
  show("写已授权表", db.exec("update t_plan_curve_detail set soc_limit = ? where id = ?", {88.5, 129}))
  show("回读", db.query("select id, power_value, soc_limit from t_plan_curve_detail where id = 129"))
  -- 事务回滚
  local tx = db.begin()
  show("tx 写", tx:exec("update t_plan_curve_detail set soc_limit = 11 where id = 129"))
  show("tx 内读", tx:query("select soc_limit from t_plan_curve_detail where id = 129"))
  show("tx 回滚", tx:rollback())
  show("回滚后读", db.query("select soc_limit from t_plan_curve_detail where id = 129"))
  show("tx 结束后再用(应报错)", tx:exec("update t_plan_curve_detail set soc_limit = 1"))
  -- 事务提交
  local tx2 = db.begin()
  tx2:exec("update t_plan_curve_detail set soc_limit = 77 where id = 129")
  show("tx 提交", tx2:commit())
  show("提交后读", db.query("select soc_limit from t_plan_curve_detail where id = 129"))
  -- 忘记 commit 的事务：被丢弃后应回滚且连接池不被污染
  do local tx3 = db.begin(); tx3:exec("update t_plan_curve_detail set soc_limit = 5 where id = 129") end
  collectgarbage()
  wait(0.5)
  show("遗弃事务后读(应仍为77)", db.query("select soc_limit from t_plan_curve_detail where id = 129"))
end)"#,
    )
    .await
    .unwrap();

    let shutdown = CancellationToken::new();
    let (d, s) = (dir.clone(), shutdown.clone());
    let join = tokio::spawn(async move {
        ScriptManager::new(None, None).run(&d, s).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // 热更新权限：收回 hot.lua 的写权限后，运行中的脚本必须被重启并立即失去权限
    let read_hot = || {
        let s = std::fs::read_to_string(dir.join(".data/hot.json")).unwrap_or_default();
        serde_json::from_str::<serde_json::Value>(&s).unwrap_or_default()["r"].as_str().unwrap_or("").to_string()
    };
    let manifest = |write: &str| format!(
        r#"return {{ {{ file = "t.lua", db = {{ read = {{"t_plan_curve_master"}}, write = {{"t_plan_curve_detail", "t_alarm"}} }} }}, "nodecl.lua", {{ file = "hot.lua", db = {{ write = {{{write}}} }} }} }}"#
    );
    tokio::fs::write(dir.join("hot.lua"), r#"MOD = { name = "hot" }
task.spawn(function()
  local r, e = db.exec("update t_plan_curve_detail set soc_limit = soc_limit where id = 129")
  save.set("r", r and "ok" or e)
end)"#).await.unwrap();
    tokio::fs::write(dir.join("_manifest.lua"), manifest("\"t_plan_curve_detail\"")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(read_hot(), "ok", "授权时应能写");
    tokio::fs::write(dir.join("_manifest.lua"), manifest("")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after = read_hot();
    assert!(after.contains("无权限"), "收回权限后应立即生效，实际: {after}");
    println!(">>> 热更新权限收回已生效: {after}");
    shutdown.cancel();
    let _ = join.await;

    println!("\n=== t_operation_log ===");
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "select operator, target, content, result, op_type from t_operation_log where operator like 'lua:%' order by id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for r in rows {
        println!("{} | {} | {} | {}", r.0, r.1, r.2, r.3);
    }
    let still: (i64,) = sqlx::query_as("select count(*) from sqlite_master where name='t_alarm'").fetch_one(&pool).await.unwrap();
    println!("t_alarm 仍存在: {}", still.0 == 1);
    tokio::fs::remove_dir_all(&dir).await.ok();
}
