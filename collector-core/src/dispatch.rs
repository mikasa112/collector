//! # 下发拦截扩展点
//!
//! `DataCenter` 只负责数据的摄入/查询/下发/订阅这些通用数据流转，不应该
//! 知道“有功功率”“EMU”这类具体业务概念。任何需要在下发前检查或修改
//! 下发点位的业务规则，都应实现 [`DispatchInterceptor`] 并通过
//! `DataCenter::register_dispatch_interceptor` 注册进来，而不是写进
//! `DataCenter` 本身。

use std::sync::{Arc, RwLock};

use crate::core::point::{DataPoint, DownDataPoint, PointId};

/// 供拦截器读取当前数据点值（例如读取 EMU 许可、SOC 等），
/// 解耦拦截器对 `DataCenter` 具体类型的依赖，便于独立测试。
pub trait PointReader {
    fn read(&self, dev_id: &str, point_id: PointId) -> Option<DataPoint>;
}

/// 下发前拦截器
///
/// 在数据点被送往下行通道之前有机会检查/改写它们，例如按业务规则钳位、
/// 拒绝或改写某些点位的值。具体拦截哪个点、依据什么规则完全由实现自行决定，
/// `DataCenter` 只负责按注册顺序依次调用。
pub trait DispatchInterceptor: Send + Sync {
    /// 拦截器名称，用于日志标注
    fn name(&self) -> &'static str;

    fn intercept(&self, dev_id: &str, points: &mut [DownDataPoint], reader: &dyn PointReader);
}

/// 北向下发门禁
///
/// 北向协议（如 IEC 104 子站）收到上级命令后，在真正 `dispatch` 之前询问门禁，
/// 返回 `false` 即直接回否定确认。与 [`DispatchInterceptor`] 不同：拦截器只能改写点值，
/// 无法拒绝命令；门禁只作用于北向来源，本地 API 下发不经过它。
/// 典型用途：协调控制器处于"就地"或"退出"时拒绝上级设定值。
pub trait NorthboundGate: Send + Sync {
    fn allow(&self, dev_id: &str, point: &DownDataPoint) -> bool;
}

static NORTHBOUND_GATES: RwLock<Vec<Arc<dyn NorthboundGate>>> = RwLock::new(Vec::new());

pub fn register_northbound_gate(gate: Arc<dyn NorthboundGate>) {
    NORTHBOUND_GATES
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push(gate);
}

/// 所有门禁都放行才返回 `true`。
pub fn northbound_allows(dev_id: &str, point: &DownDataPoint) -> bool {
    NORTHBOUND_GATES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .all(|g| g.allow(dev_id, point))
}
