use collector_core::{
    center::SharedPointCenter,
    core::point::{DataPoint, Val},
};
use salvo::{
    Depot, Request, Response, handler,
    http::StatusError,
    prelude::WebSocketUpgrade,
    websocket::{Message, WebSocket},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DevQueryLang {
    En,
    Zh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DevQueryParams {
    dev: String,
    lang: DevQueryLang,
}

#[handler]
pub async fn ws_handler(
    req: &mut Request,
    res: &mut Response,
    depot: &mut Depot,
) -> Result<(), StatusError> {
    let query = req
        .parse_queries::<DevQueryParams>()
        .map_err(|_| StatusError::bad_request())?;

    let center = depot
        .get::<SharedPointCenter>("center")
        .map_err(|_| StatusError::service_unavailable())?
        .clone();

    WebSocketUpgrade::new()
        .upgrade(req, res, move |mut ws| async move {
            handle_ws(&mut ws, center, query).await;
        })
        .await
}

#[derive(Debug, Clone, Serialize)]
struct Point<'a> {
    id: u32,
    key: &'static str,
    name: &'static str,
    value: &'a Val,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'static str>,
}

impl<'a> Point<'a> {
    fn from_data_point(data_point: &'a DataPoint, lang: DevQueryLang) -> Self {
        let name = match lang {
            DevQueryLang::Zh => data_point.name,
            DevQueryLang::En => data_point.translator.map_or(data_point.name, |t| t.en),
        };
        let mut status: Option<&'static str> = None;
        if let Some(sw) = data_point.status_word
            && let Ok(v) = u32::try_from(&data_point.value)
        {
            for (k, w) in sw.words.iter() {
                if *k == (v as u16) {
                    status = Some(match lang {
                        DevQueryLang::En => w.en,
                        DevQueryLang::Zh => w.zh,
                    });
                    break;
                }
            }
        }
        Point {
            id: data_point.id,
            key: data_point.key,
            name,
            value: &data_point.value,
            status,
            unit: data_point.unit,
        }
    }
}

async fn handle_ws(ws: &mut WebSocket, center: SharedPointCenter, query: DevQueryParams) {
    let rx = center.subscribe(&query.dev);
    if let Some(mut rx) = rx {
        loop {
            tokio::select! {
                // 监听数据更新
                result = rx.changed() => {
                    if result.is_err() {
                        // 发送器已关闭，退出循环
                        break;
                    }

                    let data = rx.borrow().clone();
                    let points = data
                        .iter()
                        .map(|p| Point::from_data_point(p, query.lang))
                        .collect::<Vec<_>>();

                    if let Ok(json) = serde_json::to_string(&points) {
                        // 如果发送失败，说明连接已断开
                        if ws.send(Message::text(json)).await.is_err() {
                            break;
                        }
                    }
                }

                // 监听客户端消息（包括断开事件）
                msg = ws.recv() => {
                    match msg {
                        // 客户端断开连接
                        None => break,
                        // 收到消息
                        Some(Ok(msg)) => {
                            // 检查是否是关闭消息
                            if msg.is_close() {
                                break;
                            }
                            // 收到 Ping，回复 Pong
                            if msg.is_ping() {
                                let data = msg.as_bytes();
                                if ws.send(Message::pong(data.to_vec())).await.is_err() {
                                    break;
                                }
                            }
                            // 其他消息忽略（可以根据需要处理）
                        }
                        // 接收错误，断开连接
                        Some(Err(_)) => break,
                    }
                }
            }
        }
    }
}
