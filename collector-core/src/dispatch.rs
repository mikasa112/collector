//! # 下发拦截扩展点
//!
//! `DataCenter` 只负责数据的摄入/查询/下发/订阅这些通用数据流转，不应该
//! 知道“有功功率”“EMU”这类具体业务概念。任何需要在下发前检查或修改
//! 下发点位的业务规则，都应实现 [`DispatchInterceptor`] 并通过
//! `DataCenter::register_dispatch_interceptor` 注册进来，而不是写进
//! `DataCenter` 本身。

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
