use std::sync::LazyLock;

use crate::core::point::DownDataPoint;

pub mod data_center;

pub use data_center::DataCenter;

pub type DownlinkSender = tokio::sync::mpsc::Sender<Vec<DownDataPoint>>;

static DATA_CENTER: LazyLock<DataCenter> = LazyLock::new(|| DataCenter::new(64));

pub fn data_center() -> &'static DataCenter {
    &DATA_CENTER
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

impl From<tokio::sync::mpsc::error::SendError<Vec<DownDataPoint>>> for DataCenterError {
    fn from(value: tokio::sync::mpsc::error::SendError<Vec<DownDataPoint>>) -> Self {
        DataCenterError::SendError(value.to_string())
    }
}
