use std::sync::Arc;

use crate::core::point::{DataPoint, PointId};

pub mod data_center;

pub use data_center::DataCenter;

pub type DownlinkSender = tokio::sync::mpsc::Sender<Vec<DataPoint>>;
pub type SharedPointCenter = Arc<dyn PointCenter>;

#[async_trait::async_trait]
pub trait PointCenter: Send + Sync {
    fn ingest(&self, dev_id: &str, points: Vec<DataPoint>);

    async fn dispatch(&self, dev_id: &str, points: Vec<DataPoint>) -> Result<(), DataCenterError>;

    fn read(&self, dev_id: &str, point_id: PointId) -> Option<DataPoint>;

    fn read_many(&self, dev_id: &str, point_ids: &[PointId]) -> Vec<DataPoint>;

    fn read_all(&self, dev_id: &str) -> Arc<[DataPoint]>;

    fn dev_ids(&self) -> Vec<String>;

    fn attach_downlink(&self, dev_id: &str, tx: DownlinkSender) -> Result<(), DataCenterError>;

    fn detach_downlink(&self, dev_id: &str);
}

#[derive(Debug, thiserror::Error)]
pub enum DataCenterError {
    #[error("通道发送时错误: {0}")]
    SendError(String),
    #[error("找不到名为{0}的设备")]
    NotFoundDevError(String),
    #[error("{0}设备已经注册")]
    DevHasRegister(String),
}

impl From<tokio::sync::mpsc::error::SendError<Vec<DataPoint>>> for DataCenterError {
    fn from(value: tokio::sync::mpsc::error::SendError<Vec<DataPoint>>) -> Self {
        DataCenterError::SendError(value.to_string())
    }
}
