//! 模块注册总纲功能的隔离验证用例：在临时目录下放两个脚本，
//! `_manifest.lua` 只列出其中一个，观察 ScriptManager 是否只启动被列出的脚本；
//! 随后编辑总纲验证热切换，删除总纲验证回退兼容模式。
//! 用法：cargo run -p collector-engine --example manifest_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-manifest-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(&dir, "a.lua", r#"MOD = { name = "A", description = "总纲已登记" }
log.info("A 已启动")"#).await;
    write(&dir, "b.lua", r#"MOD = { name = "B", description = "总纲未登记" }
log.info("B 已启动")"#).await;
    write(&dir, "_manifest.lua", r#"return { "a.lua" }"#).await;

    let shutdown = CancellationToken::new();
    let dir_clone = dir.clone();
    let shutdown_clone = shutdown.clone();
    let manager = ScriptManager::new(None, None);
    let join = tokio::spawn(async move {
        manager.run(&dir_clone, shutdown_clone).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    println!(">>> 阶段1完成：期望只看到 'A 已启动'，不应看到 'B 已启动'");

    println!(">>> 阶段2：把 b.lua 加入总纲，期望随后看到 'B 已启动'");
    write(&dir, "_manifest.lua", r#"return { "a.lua", "b.lua" }"#).await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    println!(">>> 阶段3：删除总纲，回退兼容模式（应无额外变化，两者已经都在跑）");
    tokio::fs::remove_file(dir.join("_manifest.lua")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    shutdown.cancel();
    let _ = join.await;
    tokio::fs::remove_dir_all(&dir).await.ok();
}
