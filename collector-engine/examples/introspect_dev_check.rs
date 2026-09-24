//! 运行时自省 API 验证用例：
//! - 脚本 A 用 `sys.list_scripts()` / `sys.status()` / `sys.status("B")` / `sys.status("NoSuchMod")`
//!   分别验证：脚本清单、查自己（本地直读，零死锁风险）、查别人（命令队列往返）、查不存在目标（快速报错）。
//! - Rust 侧：`ScriptManager::bus()` 在 `.run()` 消费掉 `self` 之前拿到一个存活的 `GlobalBus`，
//!   之后调 `bus.snapshot_all().await` 验证能批量拿到两个脚本的自省快照。
//! 用法：cargo run -p collector-engine --example introspect_dev_check
use collector_engine::mod_engine::ScriptManager;
use tokio_util::sync::CancellationToken;

async fn write(dir: &std::path::Path, name: &str, content: &str) {
    tokio::fs::write(dir.join(name), content).await.unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let dir = std::env::temp_dir().join(format!("collector-introspect-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    println!("脚本目录: {}", dir.display());

    write(
        &dir,
        "b.lua",
        r#"MOD = { name = "B" }
event.on("ping", function() end)
event.on_request("get_status", function(payload)
    return { status = "ok", echo = payload.n }
end)
hook.on("h", function() end)
timer.every(50, function() end)
task.spawn(function()
    while true do
        wait(1000)
    end
end)"#,
    )
    .await;

    write(
        &dir,
        "a.lua",
        r#"MOD = { name = "A", depends = { "b.lua" } }
task.spawn(function()
    wait(200)

    local scripts = sys.list_scripts()
    log.info("A 看到的脚本清单: " .. json.encode(scripts))

    local self_status = sys.status()
    log.info("A 查自己: " .. json.encode(self_status))

    local b_status = sys.status("B")
    log.info("A 查 B: " .. json.encode(b_status))

    local ok, err = pcall(sys.status, "NoSuchMod")
    log.info("A 查不存在目标: ok=" .. tostring(ok) .. " err=" .. tostring(err))
end)"#,
    )
    .await;

    println!(">>> 期望看到 'A 看到的脚本清单' 里包含 A 和 B");
    println!(">>> 期望看到 'A 查自己' 的 events/timers/coroutines 均为 0（A 自身没注册这些）");
    println!(">>> 期望看到 'A 查 B' 的 events/hooks_on/timers/coroutines 均非零");
    println!(">>> 期望很快看到 'A 查不存在目标: ok=false err=...目标模块 NoSuchMod 不存在或未运行...'");

    let shutdown = CancellationToken::new();
    let dir_clone = dir.clone();
    let shutdown_clone = shutdown.clone();
    let manager = ScriptManager::new(None, None);
    // 必须在 run(self, ...) 消费掉 manager 之前拿到 bus——这就是 Rust 侧的自省入口
    let bus = manager.bus();
    let join = tokio::spawn(async move {
        manager.run(&dir_clone, shutdown_clone).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let snapshots = bus.snapshot_all().await;
    println!(">>> Rust 侧 snapshot_all() 拿到 {} 个脚本的快照", snapshots.len());
    for s in &snapshots {
        println!(
            ">>> 脚本 {} ({}): events={} hooks_on={} timers={} coroutines={} mqtt_conns={}",
            s.name,
            s.path,
            s.snapshot.events.len(),
            s.snapshot.hooks_on.len(),
            s.snapshot.timers,
            s.snapshot.coroutines,
            s.snapshot.mqtt_conns
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    shutdown.cancel();
    let _ = join.await;
    println!(">>> ScriptManager 已正常退出");
    tokio::fs::remove_dir_all(&dir).await.ok();
}
