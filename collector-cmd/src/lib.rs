use clap::Parser;
use tracing::error;
use tracing::level_filters::LevelFilter;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

pub mod config;

pub fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let _ = LogTracer::builder().init();
    let file_appender = tracing_appender::rolling::daily("logs", "collector");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        .with_timer(fmt::time::ChronoLocal::rfc_3339())
        .with_level(true)
        .with_writer(std::io::stdout)
        .with_filter(LevelFilter::INFO);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::CLOSE)
        // 移除输出内容中的 颜色或其它格式相关转义字符
        .with_ansi(false)
        .with_writer(non_blocking)
        // 日志等级过滤
        .with_filter(LevelFilter::INFO);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let collector = Registry::default()
        .with(ErrorLayer::default())
        .with(env_filter)
        .with(file_layer)
        .with(fmt_layer);
    tracing::subscriber::set_global_default(collector).expect("Tracing collect error");
    guard
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
        }
        Err(e) => {
            error!("{}", e);
        }
    }
}
