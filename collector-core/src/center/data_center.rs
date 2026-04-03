use std::{
    collections::HashMap,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use dashmap::DashMap;
use tracing::warn;

use crate::{
    center::{DataCenterError, DownlinkSender, PointCenter},
    core::point::{DataPoint, Point, PointId},
};

pub struct DataCenter {
    downlinks: DashMap<String, DownlinkSender>,
    devices: DashMap<String, Arc<RwLock<DeviceCache>>>,
}

impl DataCenter {
    pub fn new(dev_len: usize) -> Self {
        Self {
            downlinks: DashMap::with_capacity(dev_len),
            devices: DashMap::with_capacity(dev_len),
        }
    }

    fn get_or_create_device(&self, dev_id: &str) -> Arc<RwLock<DeviceCache>> {
        self.devices
            .entry(dev_id.to_owned())
            .or_insert_with(|| Arc::new(RwLock::new(DeviceCache::default())))
            .clone()
    }

    pub fn dev_ids(&self) -> Vec<String> {
        self.devices.iter().map(|it| it.key().to_owned()).collect()
    }

    fn read_cache<'a>(
        device: &'a Arc<RwLock<DeviceCache>>,
        dev_id: &str,
    ) -> RwLockReadGuard<'a, DeviceCache> {
        match device.read() {
            Ok(cache) => cache,
            Err(err) => {
                warn!("[{}] device cache read lock poisoned, recovering", dev_id);
                err.into_inner()
            }
        }
    }

    fn write_cache<'a>(
        device: &'a Arc<RwLock<DeviceCache>>,
        dev_id: &str,
    ) -> RwLockWriteGuard<'a, DeviceCache> {
        match device.write() {
            Ok(cache) => cache,
            Err(err) => {
                warn!("[{}] device cache write lock poisoned, recovering", dev_id);
                err.into_inner()
            }
        }
    }
}

struct DeviceCache {
    latest_by_id: HashMap<PointId, DataPoint>,
    snapshot: Arc<[DataPoint]>,
    version: u64,
    snapshot_version: u64,
}

impl Default for DeviceCache {
    fn default() -> Self {
        Self {
            latest_by_id: HashMap::new(),
            snapshot: Arc::from([]),
            version: 0,
            snapshot_version: 0,
        }
    }
}

#[async_trait::async_trait]
impl PointCenter for DataCenter {
    fn ingest(&self, dev_id: &str, points: Vec<DataPoint>) {
        let device = self.get_or_create_device(dev_id);
        let mut cache = Self::write_cache(&device, dev_id);

        let mut changed = false;

        for point in points {
            let point_id = point.id();
            let new_value = point.value().clone();

            match cache.latest_by_id.get(&point_id) {
                Some(old) if old.value() == &new_value => {}
                _ => {
                    cache.latest_by_id.insert(point_id, point);
                    changed = true;
                }
            }
        }

        if changed {
            cache.version = cache.version.wrapping_add(1);
        }
    }

    async fn dispatch(&self, dev_id: &str, points: Vec<DataPoint>) -> Result<(), DataCenterError> {
        let sender = {
            let sender = self
                .downlinks
                .get(dev_id)
                .ok_or(DataCenterError::NotFoundDevError(dev_id.to_owned()))?;
            sender.clone()
        };

        sender.send(points).await.map_err(Into::into)
    }

    fn read(&self, dev_id: &str, point_id: PointId) -> Option<DataPoint> {
        let device = self.devices.get(dev_id)?;
        let cache = Self::read_cache(&device, dev_id);
        cache.latest_by_id.get(&point_id).cloned()
    }

    fn read_many(&self, dev_id: &str, point_ids: &[PointId]) -> Vec<DataPoint> {
        let Some(device) = self.devices.get(dev_id) else {
            return Vec::new();
        };

        let cache = Self::read_cache(&device, dev_id);

        point_ids
            .iter()
            .filter_map(|point_id| cache.latest_by_id.get(point_id).cloned())
            .collect()
    }

    fn read_all(&self, dev_id: &str) -> Arc<[DataPoint]> {
        let Some(device) = self.devices.get(dev_id) else {
            return Arc::from([]);
        };

        {
            let cache = Self::read_cache(&device, dev_id);
            if cache.snapshot_version == cache.version {
                return cache.snapshot.clone();
            }
        }

        let mut cache = Self::write_cache(&device, dev_id);

        if cache.snapshot_version != cache.version {
            let mut points: Vec<DataPoint> = cache.latest_by_id.values().cloned().collect();
            points.sort_by_key(|point| point.id);
            cache.snapshot = Arc::from(points);
            cache.snapshot_version = cache.version;
        }

        cache.snapshot.clone()
    }

    fn dev_ids(&self) -> Vec<String> {
        self.devices.iter().map(|it| it.key().to_owned()).collect()
    }

    fn attach_downlink(&self, dev_id: &str, tx: DownlinkSender) -> Result<(), DataCenterError> {
        use dashmap::mapref::entry::Entry as DashEntry;

        match self.downlinks.entry(dev_id.to_owned()) {
            DashEntry::Vacant(v) => {
                v.insert(tx);
                Ok(())
            }
            DashEntry::Occupied(_) => Err(DataCenterError::DevHasRegister(dev_id.to_owned())),
        }
    }

    fn detach_downlink(&self, dev_id: &str) {
        self.downlinks.remove(dev_id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::DataCenter;
    use crate::{
        center::PointCenter,
        core::point::{DataPoint, Val},
    };

    fn point(id: u32, value: u8) -> DataPoint {
        DataPoint {
            id,
            name: "p",
            value: Val::U8(value),
        }
    }

    #[test]
    fn read_all_returns_sorted_snapshot() {
        let center = DataCenter::new(1);
        center.ingest("dev-1", vec![point(2, 2), point(1, 1), point(3, 3)]);

        let snapshot = center.read_all("dev-1");
        let ids: Vec<u32> = snapshot.iter().map(|point| point.id).collect();

        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn read_all_reuses_snapshot_when_cache_unchanged() {
        let center = DataCenter::new(1);
        center.ingest("dev-1", vec![point(1, 1), point(2, 2)]);

        let first = center.read_all("dev-1");
        let second = center.read_all("dev-1");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn read_all_rebuilds_snapshot_after_value_change() {
        let center = DataCenter::new(1);
        center.ingest("dev-1", vec![point(1, 1)]);

        let first = center.read_all("dev-1");

        center.ingest("dev-1", vec![point(1, 2)]);

        let second = center.read_all("dev-1");

        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second[0].value, Val::U8(2));
    }

    #[test]
    fn ingest_same_value_does_not_invalidate_snapshot() {
        let center = DataCenter::new(1);
        center.ingest("dev-1", vec![point(1, 1)]);

        let first = center.read_all("dev-1");

        center.ingest("dev-1", vec![point(1, 1)]);

        let second = center.read_all("dev-1");

        assert!(Arc::ptr_eq(&first, &second));
    }
}
