use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::at::AtCommand;

const AT_TIMEOUT: Duration = Duration::from_secs(2);
const REOPEN_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Default)]
pub struct Eg25Info {
    pub connected: bool,
    pub registered: bool,
    pub network_type: Option<String>,
    pub rssi: Option<i32>,
    pub rsrp: Option<i32>,
    pub operator: Option<String>,
}

pub struct Eg25Poller;

impl Eg25Poller {
    /// 启动一个后台任务，周期性通过串口向 EG25-GL 发送 AT 指令采集状态，
    /// 并通过返回的 `watch::Receiver` 广播最新结果。
    pub fn spawn(
        serial_path: String,
        baud_rate: u32,
        interval: Duration,
        shutdown: CancellationToken,
    ) -> watch::Receiver<Eg25Info> {
        let (tx, rx) = watch::channel(Eg25Info::default());

        tokio::spawn(async move {
            'reconnect: loop {
                if shutdown.is_cancelled() {
                    break;
                }

                let mut at = match AtCommand::open(&serial_path, baud_rate) {
                    Ok(at) => at,
                    Err(err) => {
                        warn!("打开 EG25-GL 串口 {} 失败: {}", serial_path, err);
                        let _ = tx.send(Eg25Info::default());
                        tokio::select! {
                            _ = shutdown.cancelled() => break 'reconnect,
                            _ = tokio::time::sleep(REOPEN_DELAY) => continue 'reconnect,
                        }
                    }
                };

                let mut ticker = tokio::time::interval(interval);
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break 'reconnect,
                        _ = ticker.tick() => {
                            match poll_once(&mut at).await {
                                Ok(info) => {
                                    let _ = tx.send(info);
                                }
                                Err(err) => {
                                    debug!("EG25-GL AT 指令通信失败: {}", err);
                                    let _ = tx.send(Eg25Info::default());
                                    tokio::select! {
                                        _ = shutdown.cancelled() => break 'reconnect,
                                        _ = tokio::time::sleep(REOPEN_DELAY) => break,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        rx
    }
}

async fn poll_once(at: &mut AtCommand) -> Result<Eg25Info, String> {
    let cereg = timeout(AT_TIMEOUT, at.command("AT+CEREG?"))
        .await
        .map_err(|_| "AT+CEREG? 超时".to_string())?
        .map_err(|e| e.to_string())?;
    let qcsq = timeout(AT_TIMEOUT, at.command("AT+QCSQ"))
        .await
        .map_err(|_| "AT+QCSQ 超时".to_string())?
        .map_err(|e| e.to_string())?;
    let cops = timeout(AT_TIMEOUT, at.command("AT+COPS?"))
        .await
        .map_err(|_| "AT+COPS? 超时".to_string())?
        .map_err(|e| e.to_string())?;

    let registered = extract_line(&cereg, "+CEREG:")
        .and_then(|body| body.split(',').nth(1))
        .and_then(|stat| stat.trim().parse::<u8>().ok())
        .map(|stat| stat == 1 || stat == 5)
        .unwrap_or(false);

    let mut network_type = None;
    let mut rssi = None;
    let mut rsrp = None;
    if let Some(body) = extract_line(&qcsq, "+QCSQ:") {
        let fields: Vec<&str> = body.split(',').map(str::trim).collect();
        network_type = fields.first().map(|s| s.trim_matches('"').to_string());
        rssi = fields.get(1).and_then(|s| s.parse::<i32>().ok());
        rsrp = fields.get(2).and_then(|s| s.parse::<i32>().ok());
    }

    let operator = extract_line(&cops, "+COPS:").and_then(|body| {
        let start = body.find('"')?;
        let rest = &body[start + 1..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    });

    Ok(Eg25Info {
        connected: true,
        registered,
        network_type,
        rssi,
        rsrp,
        operator,
    })
}

fn extract_line<'a>(resp: &'a str, prefix: &str) -> Option<&'a str> {
    resp.lines()
        .find(|line| line.starts_with(prefix))
        .map(|line| line[prefix.len()..].trim())
}
