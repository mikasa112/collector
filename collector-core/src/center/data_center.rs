//! # DataCenter - 数据中心模块
//!
//! 这是一个高性能的数据采集中心，用于管理多个设备的数据点（DataPoint）。
//!
//! ## 核心功能
//!
//! 1. **数据摄入（Ingest）**：接收并缓存来自各个设备的数据点
//! 2. **数据查询（Read）**：支持单点查询、批量查询和全量查询
//! 3. **数据下发（Dispatch）**：将控制指令下发到设备
//! 4. **数据订阅（Subscribe）**：实时推送数据变化通知
//!
//! ## 架构设计
//!
//! ```text
//! DataCenter
//! ├── devices: DashMap<DeviceId, DeviceCache>  // 设备缓存映射
//! │   └── DeviceCache
//! │       ├── latest_by_id: HashMap           // 最新数据点索引
//! │       ├── snapshot: Arc<[DataPoint]>      // 排序后的快照（零拷贝）
//! │       ├── version: u64                    // 数据版本号
//! │       ├── snapshot_version: u64           // 快照版本号
//! │       └── update_tx: watch::Sender        // 数据更新通知发送器
//! └── downlinks: DashMap<DeviceId, Sender>    // 下行通道映射
//! ```
//!
//! ## 性能优化
//!
//! - **并发安全**：使用 DashMap 实现无锁并发访问
//! - **读写分离**：使用 RwLock 优化读多写少场景
//! - **快照缓存**：避免重复排序和克隆
//! - **零拷贝**：使用 Arc 共享数据
//! - **变化检测**：只在数据实际变化时更新版本号和推送通知

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use ahash::AHashMap;

use dashmap::DashMap;
use tokio::sync::watch;
use tracing::warn;

use crate::{
    center::{DataCenterError, DownlinkSender, PointCenter},
    core::point::{DataPoint, DownDataPoint, PointId},
};

/// 数据中心主结构
///
/// 负责管理多个设备的数据点缓存和下行通道
pub struct DataCenter {
    /// 下行通道映射：设备ID -> 下行数据发送器
    /// 用于将控制指令下发到设备
    downlinks: DashMap<String, DownlinkSender>,

    /// 设备缓存映射：设备ID -> 设备缓存
    /// 使用 Arc<RwLock> 实现多线程安全的读写访问
    devices: DashMap<String, Arc<RwLock<DeviceCache>>>,
}

impl DataCenter {
    /// 创建新的数据中心实例
    ///
    /// # 参数
    /// * `dev_len` - 预期的设备数量，用于预分配容量
    pub fn new(dev_len: usize) -> Self {
        Self {
            downlinks: DashMap::with_capacity(dev_len),
            devices: DashMap::with_capacity(dev_len),
        }
    }

    /// 获取或创建设备缓存
    ///
    /// 如果设备不存在，会自动创建一个新的缓存
    fn get_or_create_device(&self, dev_id: &str) -> Arc<RwLock<DeviceCache>> {
        self.devices
            .entry(dev_id.to_owned())
            .or_insert_with(|| Arc::new(RwLock::new(DeviceCache::default())))
            .clone()
    }

    /// 获取所有设备ID列表
    pub fn dev_ids(&self) -> Vec<String> {
        self.devices.iter().map(|it| it.key().to_owned()).collect()
    }

    /// 获取设备缓存的读锁
    ///
    /// 如果锁被污染（poisoned），会自动恢复并记录警告
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

    /// 获取设备缓存的写锁
    ///
    /// 如果锁被污染（poisoned），会自动恢复并记录警告
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

/// 设备缓存结构
///
/// 存储单个设备的所有数据点，使用双层缓存策略优化性能
struct DeviceCache {
    /// 数据点索引：PointId -> DataPoint
    /// 用于快速查询单个或多个数据点
    latest_by_id: AHashMap<PointId, DataPoint>,

    /// Key 到 PointId 的索引
    /// 用于通过 key 快速查找数据点
    by_key: AHashMap<&'static str, PointId>,

    /// Name 到 PointId 的索引
    /// 用于通过 name 快速查找数据点
    by_name: AHashMap<&'static str, PointId>,

    /// 排序后的数据点快照（按 PointId 排序）
    /// 使用 Arc 实现零拷贝共享
    snapshot: Arc<[DataPoint]>,

    /// 数据版本号
    /// 每次数据变化时递增，用于检测数据是否更新
    version: u64,

    /// 快照版本号
    /// 记录快照对应的数据版本，用于判断快照是否需要重建
    snapshot_version: u64,

    /// 数据更新通知发送器
    /// 用于向订阅者推送数据变化通知
    update_tx: Option<watch::Sender<Arc<[DataPoint]>>>,
}

impl Default for DeviceCache {
    fn default() -> Self {
        Self {
            latest_by_id: AHashMap::new(),
            by_key: AHashMap::new(),
            by_name: AHashMap::new(),
            snapshot: Arc::from([]),
            version: 0,
            snapshot_version: 0,
            update_tx: None,
        }
    }
}

#[async_trait::async_trait]
impl PointCenter for DataCenter {
    /// 摄入数据点
    ///
    /// 接收来自设备的数据点，更新缓存并通知订阅者
    ///
    /// # 性能优化
    /// - 只在数据实际变化时更新版本号
    /// - 只在有订阅者时才构建快照
    /// - 使用值比较避免无效更新
    fn ingest(&self, dev_id: &str, points: Vec<DataPoint>) {
        let device = self.get_or_create_device(dev_id);
        let mut cache = Self::write_cache(&device, dev_id);

        let mut changed = false;

        // 遍历所有数据点，只更新值发生变化的点
        for point in points {
            let point_id = point.id;
            let new_value = point.value.clone();

            match cache.latest_by_id.get(&point_id) {
                // 如果值相同，跳过更新
                Some(old) if old.value == new_value => {}
                // 如果值不同或点不存在，更新缓存
                _ => {
                    // 更新索引
                    cache.by_key.insert(point.key, point_id);
                    cache.by_name.insert(point.name, point_id);
                    cache.latest_by_id.insert(point_id, point);
                    changed = true;
                }
            }
        }

        if changed {
            // 递增版本号
            cache.version = cache.version.wrapping_add(1);
            // 通过借用快速拿到 tx 并克隆，随后立即释放对 cache 的不可变借用
            let active_tx = cache.update_tx.as_ref().and_then(|tx| {
                if tx.receiver_count() == 0 {
                    None // 没订阅者了
                } else {
                    Some(tx.clone()) // 还有订阅者，克隆一个通道发送端
                }
            });
            // 根据 active_tx 的状态来分流
            match active_tx {
                None => {
                    // 进到这里有两种可能：
                    // a) 本来 update_tx 就是 None
                    // b) receiver_count 为 0
                    // 如果原本有 tx 但没订阅者了，顺手把它抹掉清理掉
                    if cache.update_tx.is_some() {
                        cache.update_tx = None;
                    }
                }
                Some(tx) => {
                    // 此时 tx 是一个独立的变量，与 cache 没有任何借用瓜葛了！
                    // 我们可以安全地以可变借用访问 cache 里的所有字段
                    let mut points: Vec<DataPoint> = cache.latest_by_id.values().cloned().collect();
                    points.sort_by_key(|point| point.id);
                    let snapshot: Arc<[DataPoint]> = Arc::from(points.into_boxed_slice());

                    // 更新缓存快照（尽情修改，不会报错）
                    cache.snapshot = snapshot.clone();
                    cache.snapshot_version = cache.version;

                    // 发送更新
                    let _ = tx.send(snapshot);
                }
            }
        }
    }

    /// 下发数据点到设备
    ///
    /// 将控制指令通过下行通道直接转发给设备驱动，由驱动负责解析 PointRef。
    async fn dispatch(
        &self,
        dev_id: &str,
        points: Vec<DownDataPoint>,
    ) -> Result<(), DataCenterError> {
        let sender = self
            .downlinks
            .get(dev_id)
            .ok_or_else(|| DataCenterError::NotFoundDevError(dev_id.to_owned()))?
            .clone();
        sender.send(points).await.map_err(Into::into)
    }

    /// 读取单个数据点
    ///
    /// # 返回
    /// - `Some(DataPoint)` - 如果数据点存在
    /// - `None` - 如果设备或数据点不存在
    fn read(&self, dev_id: &str, point_id: PointId) -> Option<DataPoint> {
        let device = self.devices.get(dev_id)?;
        let cache = Self::read_cache(&device, dev_id);
        cache.latest_by_id.get(&point_id).cloned()
    }

    fn read_by_key(&self, dev_id: &str, key: &str) -> Option<DataPoint> {
        let device = self.devices.get(dev_id)?;
        let cache = Self::read_cache(&device, dev_id);
        let point_id = cache.by_key.get(key).copied()?;
        cache.latest_by_id.get(&point_id).cloned()
    }

    /// 批量读取多个数据点
    ///
    /// # 返回
    /// 存在的数据点列表（不存在的点会被过滤掉）
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

    /// 读取设备的所有数据点
    ///
    /// # 性能优化
    /// - 使用快照缓存避免重复排序
    /// - 先尝试读锁，只在需要时才升级为写锁
    /// - 返回 Arc 实现零拷贝共享
    ///
    /// # 返回
    /// 按 PointId 排序的数据点快照
    fn read_all(&self, dev_id: &str) -> Arc<[DataPoint]> {
        let Some(device) = self.devices.get(dev_id) else {
            return Arc::from([]);
        };

        // 先尝试使用读锁获取快照
        {
            let cache = Self::read_cache(&device, dev_id);
            if cache.snapshot_version == cache.version {
                return cache.snapshot.clone();
            }
        }

        // 快照过期，需要重建
        let mut cache = Self::write_cache(&device, dev_id);

        if cache.snapshot_version != cache.version {
            let mut points: Vec<DataPoint> = cache.latest_by_id.values().cloned().collect();
            points.sort_by_key(|point| point.id);
            cache.snapshot = Arc::from(points.into_boxed_slice());
            cache.snapshot_version = cache.version;
        }

        cache.snapshot.clone()
    }

    /// 获取所有设备ID列表
    fn dev_ids(&self) -> Vec<String> {
        self.devices.iter().map(|it| it.key().to_owned()).collect()
    }

    /// 附加下行通道
    ///
    /// 为设备注册一个下行数据发送器，用于接收控制指令
    ///
    /// # 错误
    /// - `DevHasRegister` - 如果设备已经注册了下行通道
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

    /// 分离下行通道
    ///
    /// 移除设备的下行数据发送器
    fn detach_downlink(&self, dev_id: &str) {
        self.downlinks.remove(dev_id);
    }

    /// 订阅指定设备的数据更新
    ///
    /// 返回一个 watch::Receiver，当设备数据有更新时会收到新的快照。
    /// 只在数据实际变化时才会推送，避免重复通知。
    ///
    /// # 参数
    /// * `dev_id` - 设备ID
    ///
    /// # 返回
    /// - `Some(Receiver)` - 订阅成功，返回接收器
    /// - `None` - 设备不存在
    ///
    /// # 使用示例
    /// ```rust,ignore
    /// let mut rx = center.subscribe("device-1").expect("设备不存在");
    ///
    /// // 监听数据更新
    /// while rx.changed().await.is_ok() {
    ///     let snapshot = rx.borrow_and_update();
    ///     // 处理更新的数据
    /// }
    /// ```
    fn subscribe(&self, dev_id: &str) -> Option<watch::Receiver<Arc<[DataPoint]>>> {
        let device = self.devices.get(dev_id)?;
        let mut cache = Self::write_cache(&device, dev_id);

        // 如果还没有 sender，创建一个
        if cache.update_tx.is_none() {
            // 确保 snapshot 是最新的
            if cache.snapshot_version != cache.version {
                let mut points: Vec<DataPoint> = cache.latest_by_id.values().cloned().collect();
                points.sort_by_key(|point| point.id);
                cache.snapshot = Arc::from(points.into_boxed_slice());
                cache.snapshot_version = cache.version;
            }

            let (tx, _rx) = watch::channel(cache.snapshot.clone());
            cache.update_tx = Some(tx);
        }

        Some(cache.update_tx.as_ref().unwrap().subscribe())
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
            key: "p",
            translator: None,
            warn_bits: None,
            status_word: None,
            unit: None,
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
