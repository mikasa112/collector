use dashmap::DashMap;

use crate::{
    center::{Center, DataCenterError, Sender},
    core::point::{Point, PointId},
    dev::Identifiable,
};

pub struct DataCenter<T>
where
    T: Point,
{
    down_chan: DashMap<String, tokio::sync::mpsc::Sender<Vec<T>>>,
    latest: DashMap<String, DashMap<PointId, T>>,
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
    fn ingest<D: Identifiable + ?Sized>(&self, dev: &D, msg: impl IntoIterator<Item = T>) {
        use dashmap::mapref::entry::Entry as DashEntry;

        let points = self.latest.entry(dev.id().to_owned()).or_default();
        for p in msg {
            let p_id = p.id();
            let new_val = p.value();
            match points.entry(p_id) {
                DashEntry::Vacant(v) => {
                    v.insert(p);
                }
                DashEntry::Occupied(mut o) => {
                    if o.get().value() != new_val {
                        o.insert(p);
                    }
                }
            }
        }
    }

    async fn dispatch<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        msg: Vec<T>,
    ) -> Result<(), DataCenterError> {
        let dev_id = dev.id();
        let sender = {
            let r = self
                .down_chan
                .get(dev_id)
                .ok_or(DataCenterError::NotFoundDevError(dev_id.to_owned()))?;
            r.clone()
        };
        sender.send(msg).await.map_err(Into::into)
    }

    fn snapshot<D: Identifiable + ?Sized>(&self, dev: &D) -> Option<Vec<T>> {
        let guard = self.latest.get(dev.id())?;
        let iter = guard.iter().map(|v| *v.value());
        Some(iter.collect())
    }

    fn read<D: Identifiable + ?Sized>(&self, dev: &D, key: u64) -> Option<T> {
        let guard = self.latest.get(dev.id())?;
        guard.get(&key).map(|v| *v.value())
    }

    fn with_read<D, F, R>(&self, dev: &D, key: u64, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&T) -> R,
    {
        let guard = self.latest.get(dev.id())?;
        let point = guard.value().get(&key)?;
        Some(f(point.value()))
    }

    fn with_snapshot<D, F, R>(&self, dev: &D, f: F) -> Option<R>
    where
        D: Identifiable + ?Sized,
        F: FnOnce(&DashMap<u64, T>) -> R,
    {
        let guard = self.latest.get(dev.id())?;
        Some(f(guard.value()))
    }

    fn attach<D: Identifiable + ?Sized>(
        &self,
        dev: &D,
        ch: Sender<T>,
    ) -> Result<(), DataCenterError> {
        use dashmap::mapref::entry::Entry as DashEntry; // 用于 entry API
        match self.down_chan.entry(dev.id().to_owned()) {
            DashEntry::Vacant(v) => {
                v.insert(ch);
                Ok(())
            }
            DashEntry::Occupied(_) => Err(DataCenterError::DevHasRegister(dev.id().to_owned())),
        }
    }

    fn detach<D: Identifiable + ?Sized>(&self, dev: &D) {
        self.down_chan.remove(dev.id());
    }
}
