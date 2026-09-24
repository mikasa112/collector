//! 跨 VM 请求-响应验证用例：脚本 A 通过 event.request 向脚本 B 发请求并等待其响应；
//! 另外验证向不存在的目标发请求会很快报错，而不是死等到默认超时（5000ms）。
//! 用法：cargo run -p collector-engine --example cross_request_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-cross-request-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(
        &dir,
        "b.lua",
        r#"MOD = { name = "B" }
event.on_request("get_status", function(payload)
    return { status = "ok", echo = payload.n }
end)"#,
    )
    .await;

    write(
        &dir,
        "a.lua",
        r#"MOD = { name = "A", depends = { "b.lua" } }
task.spawn(function()
    local resp = event.request("B", "get_status", { n = 1 })
    log.info("A 收到响应: " .. json.encode(resp))
end)
task.spawn(function()
    local ok, err = pcall(event.request, "NoSuchMod", "x", {})
    log.info("A 请求不存在目标: ok=" .. tostring(ok) .. " err=" .. tostring(err))
end)"#,
    )
    .await;

    println!(">>> 期望看到 'A 收到响应: {{\"echo\":1,\"status\":\"ok\"}}'");
    println!(">>> 期望很快（不等 5000ms 默认超时）看到 'A 请求不存在目标: ok=false err=...目标模块 NoSuchMod 不存在或未运行...'");

    let shutdown = CancellationToken::new();
    let dir_clone = dir.clone();
    let shutdown_clone = shutdown.clone();
    let manager = ScriptManager::new(None, None);
    let join = tokio::spawn(async move {
        manager.run(&dir_clone, shutdown_clone).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    shutdown.cancel();
    let _ = join.await;
    println!(">>> ScriptManager 已正常退出");
    tokio::fs::remove_dir_all(&dir).await.ok();
}
