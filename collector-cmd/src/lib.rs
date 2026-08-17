use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
use collector_core::utils::eg25::{Eg25Info, Eg25Poller};
use collector_core::utils::taos::init_taos;
use collector_engine::emu::core::Emu;
use collector_engine::mod_engine::ScriptManager;
use tokio::sync::{Mutex, watch};
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

const EG25_SERIAL_PATH: &str = "/dev/ttyUSB2";
const EG25_BAUD_RATE: u32 = 115200;
const EG25_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// 按配置初始化 MQTT 客户端，未启用或初始化失败时返回 `None`
fn init_mqtt_client(
    project: &mut config::Project,
    center: SharedPointCenter,
) -> Option<MqttClient> {
    if !project.mqtt_enable.unwrap_or(false) {
        return None;
    }
    match MqttClient::from_project(project, center) {
        Ok(client) => client,
        Err(err) => {
            error!("failed to initialize mqtt client: {}", err);
            None
        }
    }
}

/// 构建设备管理器；若启用虚拟设备引擎（EMU），额外初始化数据库并挂载虚拟设备
async fn build_dev_manager(
    devices: HashMap<String, config::Device>,
    center: SharedPointCenter,
    data_center: &Arc<DataCenter>,
    can_bus: SharedCanBus,
    emu_enable: bool,
) -> DevManager {
    let mut manager = DevManager::new(devices, center.clone(), can_bus);

    if emu_enable {
        data_center.set_emu_enable(true);
        // 数据库连接池需要在设备管理器（含虚拟设备引擎）启动前初始化好，
        // 否则引擎里依赖数据库的策略（如计划曲线）会因为连接池还未就绪而报错
        let _sql_pool = init_database(DatabaseConfig::default())
            .await
            .expect("数据库初始化失败");
        init_taos().await.expect("taos数据库初始化失败");
        if let Err(e) = get_runtime().await {
            tracing::error!("EMU运行时配置错误: {}", e);
        }
        let emu = Emu::new(center.clone()).await;
        manager
            .add_device(Arc::new(Mutex::new(Box::new(emu))))
            .await;
    }

    manager
}

/// 启动移远 EG25-GL 4G 模块状态采集（独立于 DataCenter，直接通过 WebSocket 推送）
fn start_eg25_poller(shutdown: &ShutdownManager) -> watch::Receiver<Eg25Info> {
    Eg25Poller::spawn(
        EG25_SERIAL_PATH.to_string(),
        EG25_BAUD_RATE,
        EG25_POLL_INTERVAL,
        shutdown.child_token(),
    )
}

/// 按配置启动北向 Modbus TCP 服务器（未配置齐全时跳过）
fn start_north_modbus_server(
    host: Option<&str>,
    port: Option<u16>,
    conf: Option<&str>,
    center: SharedPointCenter,
    shutdown: ShutdownManager,
) {
    let (Some(host), Some(port), Some(conf)) = (host, port, conf) else {
        return;
    };
    let addr = format!("{}:{}", host, port);
    match addr.parse() {
        Ok(addr) => match ModbusServer::new(conf, addr, center) {
            Ok(server) => {
                tokio::spawn(server.start(shutdown));
            }
            Err(e) => error!("北向Modbus配置加载失败: {}", e),
        },
        Err(e) => error!("北向Modbus地址解析失败 {}: {}", addr, e),
    }
}

/// 启动 HTTP/WebSocket API 服务器
fn start_api_server(
    ip: String,
    port: u16,
    center: SharedPointCenter,
    eg25_rx: Option<watch::Receiver<Eg25Info>>,
    shutdown: ShutdownManager,
) {
    let api_server = ApiApp::new(ip, port, center, eg25_rx);
    tokio::spawn(api_server.start(shutdown));
}

/// 启动 Lua 脚本模组引擎
fn start_script_engine(
    center: SharedPointCenter,
    mqtt_client: Option<&MqttClient>,
    can_bus: SharedCanBus,
    shutdown: &ShutdownManager,
) {
    let override_store = mqtt_client.map(|c| c.override_store.clone());
    let script_manager = ScriptManager::new(center, override_store, Some(can_bus));
    let script_token = shutdown.child_token();
    tokio::spawn(async move {
        if let Err(err) = script_manager.run("lua_scripts", script_token).await {
            error!("脚本模组引擎异常: {}", err);
        }
    });
}

/// 优雅关闭所有组件
async fn shutdown_all(mut manager: DevManager, mqtt_client: Option<MqttClient>) {
    manager.stop_all().await;
    close_database().await;
    if let Some(client) = mqtt_client.as_ref()
        && let Err(err) = client.stop().await
    {
        error!("failed to stop mqtt client: {}", err);
    }
}

pub async fn cmd() {
    let args = Args::parse();
    let mut project = match config::Configuration::new(args.config).await {
        Ok(p) => p,
        Err(e) => {
            error!("{}", e);
            return;
        }
    };
    project.load_device_configs().await;

    // 创建统一的关闭管理器
    let shutdown = ShutdownManager::new();

    let emu_enable = project.project.emu_enable.unwrap_or(false);
    let http_ip = project
        .project
        .http_ip
        .clone()
        .unwrap_or_else(|| "0.0.0.0".to_string());
    let http_port = project.project.http_port.unwrap_or(9091);
    let north_modbus_host = project.project.north_modbus_host.clone();
    let north_modbus_port = project.project.north_modbus_port;
    let north_modbus_conf = project.project.north_modbus_conf.clone();

    let data_center = Arc::new(DataCenter::new(32));
    let center: SharedPointCenter = data_center.clone();
    let can_bus = SharedCanBus::default();

    let mqtt_client = init_mqtt_client(&mut project.project, center.clone());

    let devices = std::mem::take(&mut project.project.devices);
    let mut manager = build_dev_manager(
        devices,
        center.clone(),
        &data_center,
        can_bus.clone(),
        emu_enable,
    )
    .await;
    manager.start_all().await;

    let eg25_rx = start_eg25_poller(&shutdown);

    start_north_modbus_server(
        north_modbus_host.as_deref(),
        north_modbus_port,
        north_modbus_conf.as_deref(),
        center.clone(),
        shutdown.clone(),
    );

    start_api_server(
        http_ip,
        http_port,
        center.clone(),
        Some(eg25_rx),
        shutdown.clone(),
    );

    start_script_engine(center.clone(), mqtt_client.as_ref(), can_bus, &shutdown);

    // 在后台监听关闭信号
    tokio::spawn(shutdown.clone().listen_shutdown_signal());

    // 等待关闭信号
    shutdown.wait_for_shutdown().await;

    shutdown_all(manager, mqtt_client).await;
}
