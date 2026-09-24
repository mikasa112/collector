//! 脚本死循环/超时保护验证用例：event.on 处理器里写一个 `while true do end` 死循环，
//! 验证全局指令钩子能中断它并报错，而不会卡死整个引擎/进程。
//! 用法：cargo run -p collector-engine --example runaway_script_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-runaway-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(
        &dir,
        "runaway.lua",
        r#"MOD = { name = "Runaway" }
event.on("boom", function()
    log.info("[boom] 开始死循环")
    while true do end
end)
timer.after(100, function()
    event.emit("boom", {})
end)"#,
    )
    .await;

    println!(">>> 期望看到 '[boom] 开始死循环'，随后（约 200ms 后）出现一条超时警告日志，进程不应卡死");

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
    println!(">>> ScriptManager 已正常退出（未卡死）");
    tokio::fs::remove_dir_all(&dir).await.ok();
}
