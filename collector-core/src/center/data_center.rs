use dashmap::DashMap;

use crate::{
    center::{Center, DataCenterError, Sender},
    core::point::{Item, Point, PointId},
    dev::Identifiable,
};

pub struct DataCenter<T, I>
where
    T: Point,
    I: Item,
{
    down_chan: DashMap<String, tokio::sync::mpsc::Sender<Vec<T>>>,
    latest: DashMap<String, DashMap<PointId, T>>,
    items: DashMap<String, DashMap<PointId, I>>,
}

impl<T, I> DataCenter<T, I>
where
    T: Point,
    I: Item,
{
    pub fn new(dev_len: usize) -> Self {
        Self {
            down_chan: DashMap::with_capacity(dev_len),
            latest: DashMap::with_capacity(dev_len),
            items: DashMap::with_capacity(dev_len),
        }
    }
}

#[async_trait::async_trait]
impl<T, I> Center<T, I> for DataCenter<T, I>
where
    T: Point,
    I: Item,
{
    fn load<D: Identifiable + ?Sized>(&self, dev: &D, record: impl IntoIterator<Item = I>) {
        let records = self.items.entry(dev.id()).or_default();
        for p in record {
            records.insert(p.id(), p);
        }
    }

    fn ingest<D: Identifiable + ?Sized>(&self, dev: &D, msg: impl IntoIterator<Item = T>) {
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

    async fn dispatch<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        msg: Vec<T>,
    ) -> Result<(), DataCenterError> {
        let sender = {
            let r = self
                .down_chan
                .get(dev.id().as_str())
                .ok_or(DataCenterError::NotFoundDevError(dev.id()))?;
            r.clone()
        };
        sender.send(msg).await.map_err(Into::into)
    }

    fn snapshot<D: Identifiable + ?Sized>(&self, dev: &D) -> Option<Vec<T>> {
        let guard = self.latest.get(&dev.id())?;
        let iter = guard.iter().map(|v| v.value().clone());
        Some(iter.collect())
    }

    fn read<D: Identifiable + ?Sized>(&self, dev: &D, key: u64) -> Option<T> {
        let guard = self.latest.get(&dev.id())?;
        guard.get(&key).map(|v| v.value().clone())
    }

    fn with_read<D, F, R>(&self, dev: &D, key: u64, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&T) -> R,
    {
        let guard = self.latest.get(&dev.id())?;
        let point = guard.value().get(&key)?;
        Some(f(point.value()))
    }

    fn with_snapshot<D, F, R>(&self, dev: &D, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&DashMap<u64, T>) -> R,
    {
        let guard = self.latest.get(&dev.id())?;
        Some(f(guard.value()))
    }

    fn attach<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        ch: Sender<T>,
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

    fn detach<D: Identifiable + ?Sized>(&self, dev: &D) {
        self.down_chan.remove(&dev.id());
    }
}
