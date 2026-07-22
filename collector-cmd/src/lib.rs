use std::sync::Arc;

use clap::Parser;
use collector_api::ApiApp;
use collector_core::center::DataCenter;
use collector_core::center::SharedPointCenter;
use collector_core::config;
use collector_core::dev::can_bus::SharedCanBus;
use collector_core::dev::manager::DevManager;
use collector_core::dock::modbus::ModbusServer;
use collector_core::dock::mqtt::client::MqttClient;
use collector_core::runtime::core::get_runtime;
use collector_core::shutdown::ShutdownManager;
use collector_core::utils::database::close_database;
use collector_core::utils::database::{DatabaseConfig, init_database};
use collector_engine::emu::core::Emu;
use collector_engine::mod_engine::ScriptManager;
use tokio::sync::Mutex;
use tracing::error;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

#[inline]
pub fn init_tracing() -> Vec<tracing_appender::non_blocking::WorkerGuard> {
    let _ = LogTracer::builder().init();

    let mut guards = Vec::new();

    //API 模块日志
    let api_appender = tracing_appender::rolling::daily("logs", "api");
    let (non_blocking_api, guard_api) = tracing_appender::non_blocking(api_appender);
    guards.push(guard_api);

    let engine_appender = tracing_appender::rolling::daily("logs", "engine");
    let (non_blocking_engine, guard_engine) = tracing_appender::non_blocking(engine_appender);
    guards.push(guard_engine);

    let collector_appender = tracing_appender::rolling::daily("logs", "collector");
    let (non_blocking_collector, guard_collector) =
        tracing_appender::non_blocking(collector_appender);
    guards.push(guard_collector);

    // 控制台输出层
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_timer(fmt::time::ChronoLocal::rfc_3339())
        .with_level(true)
        .with_writer(std::io::stdout)
        .with_filter(EnvFilter::new("info,zbus=off"));

    // API 模块文件层 - 只记录 collector_api 模块的日志
    let api_layer = fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_ansi(false)
        .with_writer(non_blocking_api)
        .with_filter(EnvFilter::new("collector_api=debug"));

    // 收集器模块文件层 - 只记录 collector_core 模块的日志
    let collector_layer = fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_ansi(false)
        .with_writer(non_blocking_collector)
        .with_filter(EnvFilter::new("collector_core=debug"));

    // 引擎模块文件层 - 只记录 collector_engine 模块的日志
    let engine_layer = fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_ansi(false)
        .with_writer(non_blocking_engine)
        .with_filter(EnvFilter::new("collector_engine=debug"));

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let collector = Registry::default()
        .with(ErrorLayer::default())
        .with(env_filter)
        .with(api_layer)
        .with(collector_layer)
        .with(engine_layer)
        .with(fmt_layer);
    tracing::subscriber::set_global_default(collector).expect("Tracing collect error");
    guards
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, value_name = "collector配置文件")]
    config: String,
}

pub async fn cmd() {
    let args = Args::parse();
    match config::Configuration::new(args.config).await {
        Ok(mut p) => {
            p.load_device_configs().await;

            // 创建统一的关闭管理器
            let shutdown = ShutdownManager::new();

            // 数据库连接池需要在设备管理器（含虚拟设备引擎）启动前初始化好，
            // 否则引擎里依赖数据库的策略（如计划曲线）会因为连接池还未就绪而报错
            let _sql_pool = init_database(DatabaseConfig::default())
                .await
                .expect("数据库初始化失败");
            let _runtime = get_runtime()
                .await
                .map_err(|e| tracing::error!("EMU运行时配置错误: {}", e));
            let center: SharedPointCenter = Arc::new(DataCenter::new(32));
            let can_bus = SharedCanBus::default();
            let mqtt_client = match MqttClient::from_project(&mut p.project, center.clone()) {
                Ok(client) => client,
                Err(err) => {
                    error!("failed to initialize mqtt client: {}", err);
                    None
                }
            };
            let mut manager = DevManager::new(p.project.devices, center.clone(), can_bus.clone());
            let emu = Emu::new(center.clone()).await;
            manager
                .add_device(Arc::new(Mutex::new(Box::new(emu))))
                .await;
            manager.start_all().await;

            // 启动北向 Modbus TCP 服务器
            if let (Some(host), Some(port), Some(conf)) = (
                p.project.north_modbus_host.as_deref(),
                p.project.north_modbus_port,
                p.project.north_modbus_conf.as_deref(),
            ) {
                let addr = format!("{}:{}", host, port);
                match addr.parse() {
                    Ok(addr) => match ModbusServer::new(conf, addr, center.clone()) {
                        Ok(server) => {
                            tokio::spawn(server.start(shutdown.clone()));
                        }
                        Err(e) => error!("北向Modbus配置加载失败: {}", e),
                    },
                    Err(e) => error!("北向Modbus地址解析失败 {}: {}", addr, e),
                }
            }

            // 启动 API 服务器
            let api_server = ApiApp::new(
                p.project
                    .http_ip
                    .clone()
                    .unwrap_or_else(|| "0.0.0.0".to_string()),
                p.project.http_port.unwrap_or(9091),
                center.clone(),
            );

            tokio::spawn(api_server.start(shutdown.clone()));

            // 启动脚本模组引擎
            let override_store = mqtt_client.as_ref().map(|c| c.override_store.clone());
            let script_manager = ScriptManager::new(center.clone(), override_store, Some(can_bus));
            let script_token = shutdown.child_token();
            tokio::spawn(async move {
                if let Err(err) = script_manager.run("lua_scripts", script_token).await {
                    error!("脚本模组引擎异常: {}", err);
                }
            });

            // 在后台监听关闭信号
            tokio::spawn(shutdown.clone().listen_shutdown_signal());

            // 等待关闭信号
            shutdown.wait_for_shutdown().await;

            // 优雅关闭所有组件
            manager.stop_all().await;
            close_database().await;
            if let Some(client) = mqtt_client.as_ref()
                && let Err(err) = client.stop().await
            {
                error!("failed to stop mqtt client: {}", err);
            }
        }
        Err(e) => {
            error!("{}", e);
        }
    }
}
