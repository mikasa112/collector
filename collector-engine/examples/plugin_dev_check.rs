//! `plugin.list` 自动发现机制的隔离验证用例：在临时目录下放一个顶层脚本 main.lua，
//! 及其 plugins/ 子目录下 p1.lua、p2.lua 与一个 "_" 前缀禁用的 _p3.lua，
//! 观察 main.lua 通过 plugin.list("plugins") 自动发现并 require 前两者，跳过被禁用的 _p3.lua。
//! 用法：cargo run -p collector-engine --example plugin_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(path, content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-plugin-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(
        &dir,
        "main.lua",
        r#"MOD = { name = "Main" }
for _, m in ipairs(plugin.list("plugins")) do
    local ok, err = pcall(require, m)
    if not ok then
        log.error("加载插件失败 [" .. m .. "]: " .. tostring(err))
    else
        log.info("插件已加载: " .. m)
    end
end"#,
    )
    .await;
    write(&dir, "plugins/p1.lua", r#"log.info("[p1] 已加载")"#).await;
    write(&dir, "plugins/p2.lua", r#"log.info("[p2] 已加载")"#).await;
    write(&dir, "plugins/_p3.lua", r#"log.info("[p3] 不应加载")"#).await;

    println!(">>> 期望看到 '插件已加载: plugins.p1' 与 'plugins.p2'，不应看到任何 p3 相关日志");

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
