//! 跨顶层脚本事件总线（event.emit）与卸载生命周期钩子（on_unload）验证用例。
//! a.lua 订阅 "ping" 事件与 "on_unload" 钩子；b.lua 启动后 event.emit("ping", ...) 广播。
//! 用法：cargo run -p collector-engine --example cross_vm_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-cross-vm-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(
        &dir,
        "a.lua",
        r#"MOD = { name = "A" }
event.on("ping", function(v)
    log.info("[A] 收到跨 VM 广播: " .. tostring(v.n))
end)
hook.on("on_unload", function()
    log.info("[A] on_unload 钩子触发")
end)"#,
    )
    .await;

    write(
        &dir,
        "b.lua",
        r#"MOD = { name = "B" }
-- 延迟广播，确保其它顶层脚本（如 a.lua）已完成加载并在总线登记
timer.after(200, function()
    event.emit("ping", { n = 1 })
    log.info("[B] 已广播 ping")
end)"#,
    )
    .await;

    println!(">>> 期望看到 '[A] 收到跨 VM 广播: 1'（证明跨 VM 投递成功）");
    println!(">>> 关闭阶段期望看到 '[A] on_unload 钩子触发'（先于资源清理）");

    let shutdown = CancellationToken::new();
    let dir_clone = dir.clone();
    let shutdown_clone = shutdown.clone();
    let manager = ScriptManager::new(None, None);
    let join = tokio::spawn(async move {
        manager.run(&dir_clone, shutdown_clone).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    shutdown.cancel();
    let _ = join.await;
    tokio::fs::remove_dir_all(&dir).await.ok();
}
