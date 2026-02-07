use dashmap::DashMap;
use serde::Serialize;

use crate::{
    center::{Center, DataCenterError},
    core::point::{Point, Val},
    dev::Identifiable,
};

#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct Entry {
    pub key: String,
    pub value: Val,
}

impl Point for Entry {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn value(&self) -> Val {
        self.value
    }
}

pub struct DataCenter<T>
where
    T: Point,
{
    down_chan: DashMap<String, tokio::sync::mpsc::Sender<Vec<T>>>,
    latest: DashMap<String, DashMap<String, T>>,
}

impl<T> DataCenter<T>
where
    T: Point,
{
    pub fn new(dev_len: usize) -> Self {
        Self {
            down_chan: DashMap::with_capacity(dev_len),
            latest: DashMap::with_capacity(dev_len),
        }
    }
}

#[async_trait::async_trait]
impl<T> Center<T> for DataCenter<T>
where
    T: Point,
{
    fn ingest(&self, dev: &impl Identifiable, msg: impl IntoIterator<Item = T>) {
        let dev_id = dev.id();
        let points = self.latest.entry(dev_id).or_default();
        for p in msg {
            let key = p.key();
            let new_val = p.value();
            let need_update = points
                .get(&key)
                .map(|old| {
                    let old_t = old.value();
                    old_t.value() != new_val
                })
                .unwrap_or(true);
            if need_update {
                points.insert(key, p);
            }
        }
    }

    async fn dispatch(&self, dev: &impl Identifiable, msg: Vec<T>) -> Result<(), DataCenterError> {
        let sender = {
            let r = self
                .down_chan
                .get(dev.id().as_str())
                .ok_or(DataCenterError::NotFoundDevError(dev.id()))?;
            r.clone()
        };
        sender.send(msg).await.map_err(Into::into)
    }

    fn snapshot(&self, dev: &impl Identifiable) -> Option<Vec<T>> {
        let guard = self.latest.get(&dev.id())?;
        let iter = guard.iter().map(|v| v.value().clone());
        Some(iter.collect())
    }

    fn read(&self, dev: &impl Identifiable, key: &str) -> Option<T> {
        let guard = self.latest.get(&dev.id())?;
        guard.get(key).map(|v| v.value().clone())
    }

    fn attach(
        &self,
        dev: &impl Identifiable,
        ch: tokio::sync::mpsc::Sender<Vec<T>>,
    ) -> Result<(), DataCenterError> {
        use dashmap::mapref::entry::Entry as DashEntry; // 用于 entry API
        match self.down_chan.entry(dev.id()) {
            DashEntry::Vacant(v) => {
                v.insert(ch);
                Ok(())
            }
            DashEntry::Occupied(_) => Err(DataCenterError::DevHasRegister(dev.id())),
        }
    }

    fn detach(&self, dev: &impl Identifiable) {
        self.down_chan.remove(&dev.id());
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Default)]
    struct TestDev {}
    impl Identifiable for TestDev {
        fn id(&self) -> String {
            "BCU".to_string()
        }
    }

    #[test]
    fn test_ingest() {
        let center: DataCenter<Entry> = DataCenter::new(12);
        let dev = TestDev::default();
        let a = center.snapshot(&dev);
        assert!(a.is_none());
        center.ingest(
            &dev,
            vec![Entry {
                key: String::from("SOH"),
                value: Val::F32(100.0),
            }],
        );
        let b = center.snapshot(&dev);
        assert!(b.is_some());
        assert_eq!(b.unwrap()[0].value, Val::F32(100.0));
    }

    #[tokio::test]
    async fn test_dispatch() {
        let center: DataCenter<Entry> = DataCenter::new(12);
        let dev = TestDev::default();
        let a = center.snapshot(&dev);
        assert!(a.is_none());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        center.attach(&dev, tx).unwrap();
        let _ = center
            .dispatch(
                &dev,
                vec![Entry {
                    key: String::from("SOC"),
                    value: Val::F32(84.3),
                }],
            )
            .await;
        center.ingest(
            &dev,
            vec![Entry {
                key: String::from("SOH"),
                value: Val::F32(100.0),
            }],
        );
        let b = center.snapshot(&dev);
        assert!(b.is_some());
        assert_eq!(b.unwrap()[0].value, Val::F32(100.0));
        let c = rx.recv().await;
        assert!(c.is_some());
        assert_eq!(c.unwrap()[0].value, Val::F32(84.3));
        center.detach(&dev);
    }
}
