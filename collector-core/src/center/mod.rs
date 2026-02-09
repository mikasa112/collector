use std::sync::OnceLock;

use dashmap::DashMap;

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
    fn ingest<D: Identifiable + ?Sized>(&self, dev: &D, msg: impl IntoIterator<Item = T>);
    async fn dispatch<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        msg: Vec<T>,
    ) -> Result<(), DataCenterError>;
    fn snapshot<D: Identifiable + ?Sized>(&self, dev: &D) -> Option<Vec<T>>;
    fn read<D: Identifiable + ?Sized>(&self, dev: &D, key: &str) -> Option<T>;
    fn with_read<D, F, R>(&self, dev: &D, key: &str, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&T) -> R;
    fn with_snapshot<D, F, R>(&self, dev: &D, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&DashMap<String, T>) -> R;
    fn attach<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        ch: Sender<T>,
    ) -> Result<(), DataCenterError>;
    fn detach<D: Identifiable + ?Sized>(&self, dev: &D);
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
