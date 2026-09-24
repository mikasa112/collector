//! 模组化增强四项能力的隔离验证用例：钩子/替换、依赖声明与加载顺序、
//! 版本兼容检查、持久化存档。
//! 用法：cargo run -p collector-engine --example hook_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

async fn run_manager(dir: &std::path::Path, millis: u64) {
    let shutdown = CancellationToken::new();
    let dir_clone = dir.to_path_buf();
    let shutdown_clone = shutdown.clone();
    let manager = ScriptManager::new(None, None);
    let join = tokio::spawn(async move {
        manager.run(&dir_clone, shutdown_clone).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    shutdown.cancel();
    let _ = join.await;
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-hook-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    // A：验证 hook.on / hook.override（同 VM 内插件钩子机制本身）
    write(
        &dir,
        "a.lua",
        r#"MOD = { name = "A" }
hook.on("greet", function(payload)
    log.info("[A] on 处理器看到 " .. tostring(payload.msg))
end)
hook.override("greet", function(payload)
    return { msg = "overridden:" .. payload.msg }
end)
local result = hook.emit("greet", { msg = "hi" })
log.info("[A] emit 结果 = " .. tostring(result.msg))"#,
    )
    .await;

    // B：依赖 a.lua，验证依赖声明与加载顺序
    write(
        &dir,
        "b.lua",
        r#"MOD = { name = "B", depends = { "a.lua" } }
log.info("B 已启动")"#,
    )
    .await;

    // C：声明不兼容的 api_version，验证版本兼容检查（应被拒绝加载，不受总纲影响）
    write(
        &dir,
        "c.lua",
        r#"MOD = { name = "C", api_version = 999 }
log.info("C 已启动")"#,
    )
    .await;

    // D：验证持久化存档跨重启保留
    write(
        &dir,
        "d.lua",
        r#"MOD = { name = "D" }
local count = (save.get("counter") or 0) + 1
save.set("counter", count)
log.info("D 存档计数 = " .. tostring(count))"#,
    )
    .await;

    write(&dir, "_manifest.lua", r#"return { "a.lua", "c.lua", "d.lua" }"#).await;

    println!(
        ">>> 阶段1：期望 A 的 override 生效日志('[A] emit 结果 = overridden:hi')；C 因 api_version 不兼容被跳过；\
         B 因缺少依赖 a.lua(未注册)被跳过；D 存档计数=1"
    );
    run_manager(&dir, 800).await;

    println!(">>> 阶段2：把 b.lua 加入总纲并重启，期望 B 随后启动（依赖 a.lua 已注册），D 存档计数=2");
    write(
        &dir,
        "_manifest.lua",
        r#"return { "a.lua", "b.lua", "c.lua", "d.lua" }"#,
    )
    .await;
    run_manager(&dir, 800).await;

    println!(">>> 阶段3：再次重启 ScriptManager（同一目录），期望 D 存档计数=3（跨重启持久化）");
    run_manager(&dir, 800).await;

    tokio::fs::remove_dir_all(&dir).await.ok();
}
