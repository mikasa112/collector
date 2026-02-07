use std::sync::OnceLock;

use crate::{
    center::data_center::{DataCenter, Entry},
    core::point::Point,
    dev::Identifiable,
};

pub mod data_center;

pub type Sender<T> = tokio::sync::mpsc::Sender<Vec<T>>;

#[async_trait::async_trait]
pub trait Center<T>
where
    T: Point + Send + Sync,
{
    fn ingest(&self, dev: &impl Identifiable, msg: impl IntoIterator<Item = T>);
    async fn dispatch(&self, dev: &impl Identifiable, msg: Vec<T>) -> Result<(), DataCenterError>;
    fn snapshot(&self, dev: &impl Identifiable) -> Option<Vec<T>>;
    fn read(&self, dev: &impl Identifiable, key: &str) -> Option<T>;
    fn attach(&self, dev: &impl Identifiable, ch: Sender<T>) -> Result<(), DataCenterError>;
    fn detach(&self, dev: &impl Identifiable);
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

impl<T> From<tokio::sync::mpsc::error::SendError<Vec<T>>> for DataCenterError {
    fn from(value: tokio::sync::mpsc::error::SendError<Vec<T>>) -> Self {
        DataCenterError::SendError(value.to_string())
    }
}

static CENTER: OnceLock<DataCenter<Entry>> = OnceLock::new();

pub fn global_center() -> &'static DataCenter<Entry> {
    CENTER.get_or_init(|| DataCenter::new(32))
}
