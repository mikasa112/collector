//! 系统管理模块（配置读写/备份回滚/重启/状态）的隔离验证用例。
//! 在系统临时目录下起一个独立的工作目录，插入一条测试用户，然后起 API 服务，
//! 不碰仓库真实的 config/config.json，也不触碰真实数据库。
//! 用法：cargo run -p collector-api --example system_dev_server
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use collector_api::ApiApp;
use collector_core::shutdown::ShutdownManager;
use collector_core::utils::database::{DatabaseConfig, init_database};

const TEST_ACCOUNT: &str = "admin";
const TEST_PASSWORD: &str = "test1234";

fn main() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let dir = std::env::temp_dir().join(format!("collector-system-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        std::env::set_current_dir(&dir).unwrap();
        println!("工作目录: {}", dir.display());

        tokio::fs::create_dir_all("config").await.unwrap();
        tokio::fs::write("config/config.json", br#"{"devices":{}}"#)
            .await
            .unwrap();

        let pool = init_database(DatabaseConfig::default()).await.unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS t_user (
                id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                name TEXT,
                account TEXT NOT NULL UNIQUE,
                password TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at DATETIME DEFAULT (datetime('now', 'localtime')),
                updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
                deleted_at DATETIME
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(TEST_PASSWORD.as_bytes(), &salt)
            .unwrap()
            .to_string();
        sqlx::query("INSERT INTO t_user (account, password, role) VALUES (?, ?, 'admin')")
            .bind(TEST_ACCOUNT)
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();

        println!("测试账号: {TEST_ACCOUNT} / {TEST_PASSWORD}");
        println!("监听: http://127.0.0.1:19191");

        let shutdown = ShutdownManager::new();
        ApiApp::new("127.0.0.1".to_string(), 19191, None)
            .start(shutdown)
            .await;
    });
}
