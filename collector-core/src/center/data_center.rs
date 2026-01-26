use dashmap::DashMap;
use serde::Serialize;
use std::collections::HashMap;

use crate::{
    center::{Center, Error},
    core::{
        point::{Point, Val},
        Identifiable,
    },
};

#[derive(Debug, Serialize, Clone, Copy)]
pub struct Entry {
    pub key: &'static str,
    pub value: Val,
}

impl Point for Entry {
    fn key(&self) -> &'static str {
        self.key
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
    latest: DashMap<String, HashMap<&'static str, T>>,
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
        let mut points = self.latest.entry(dev_id).or_default();
        for p in msg {
            points.insert(p.key(), p);
        }
    }

    async fn dispatch(&self, dev: &impl Identifiable, msg: Vec<T>) -> Result<(), Error> {
        let sender = {
            let r = self
                .down_chan
                .get(dev.id().as_str())
                .ok_or(Error::NotFoundDevError(dev.id()))?;
            r.clone()
        };
        sender.send(msg).await.map_err(Into::into)
    }

    fn snapshot(&self, dev: &impl Identifiable) -> Option<Vec<T>> {
        let guard = self.latest.get(&dev.id())?;
        let iter = guard.values().copied();
        Some(iter.collect())
    }

    fn read(&self, dev: &impl Identifiable, key: &str) -> Option<T> {
        let guard = self.latest.get(&dev.id())?;
        guard.get(key).copied()
    }

    fn attach(
        &self,
        dev: &impl Identifiable,
        ch: tokio::sync::mpsc::Sender<Vec<T>>,
    ) -> Result<(), Error> {
        use dashmap::mapref::entry::Entry as DashEntry; // 用于 entry API
        match self.down_chan.entry(dev.id()) {
            DashEntry::Vacant(v) => {
                v.insert(ch);
                Ok(())
            }
            DashEntry::Occupied(_) => Err(Error::DevHasRegister(dev.id())),
        }
    }

    fn detach(&self, dev: impl Identifiable) {
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
                key: "SOH",
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
                    key: "SOC",
                    value: Val::F32(84.3),
                }],
            )
            .await;
        center.ingest(
            &dev,
            vec![Entry {
                key: "SOH",
                value: Val::F32(100.0),
            }],
        );
        let b = center.snapshot(&dev);
        assert!(b.is_some());
        assert_eq!(b.unwrap()[0].value, Val::F32(100.0));
        let c = rx.recv().await;
        assert!(c.is_some());
        assert_eq!(c.unwrap()[0].value, Val::F32(84.3));
        center.detach(dev);
    }
}
