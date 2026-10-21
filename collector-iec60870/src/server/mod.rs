//! IEC 104 子站（服务端）的最小实现，主要用于主站联调与集成测试。
//!
//! 行为：
//! - 收到站总召唤时，先回 ACT_CON，再把该 CA 下已配置的全部点逐个以 COT=响应站召唤上送，最后回 ACT_TERM；
//! - 收到控制命令（遥控/遥调/步位置）时：设置了 [`Slave::set_command_handler`] 则由回调决定回
//!   肯定还是否定 ACT_CON；否则记录下来供 [`Slave::take_commands`] 取走并回肯定 ACT_CON；
//! - 收到时钟同步命令时回 ACT_CON，但不修改系统时间；
//! - [`Slave::update`] / [`Slave::update_many`] 修改点值并以 COT=突发 主动上送，
//!   点可以带品质描述（QDS）与 CP56Time2a 时标。

use std::{
    collections::HashMap,
    ffi::{CString, c_void},
    os::raw::c_int,
    ptr,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    client::{DoublePointValue, Error, StepCommandValue},
    ffi,
};

/// 子站点的值，决定上送时使用的 ASDU 类型。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointValue {
    /// M_SP_NA_1
    Single(bool),
    /// M_DP_NA_1
    Double(DoublePointValue),
    /// M_ME_NA_1
    Normalized(f32),
    /// M_ME_NB_1
    Scaled(i32),
    /// M_ME_NC_1
    Short(f32),
}

/// 品质描述词（QDS）常用位，可按位或后填入 [`SlavePoint::quality`]。
pub mod quality {
    pub const GOOD: u8 = 0;
    pub const OVERFLOW: u8 = 1;
    pub const BLOCKED: u8 = 16;
    pub const SUBSTITUTED: u8 = 32;
    pub const NON_TOPICAL: u8 = 64;
    pub const INVALID: u8 = 128;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlavePoint {
    pub ca: i32,
    pub ioa: i32,
    pub value: PointValue,
    /// 品质描述词，见 [`quality`]。
    pub quality: u8,
    /// Unix 毫秒时间戳；`Some` 时使用带 CP56Time2a 时标的 ASDU 类型（`*_TB_1`/`*_TD_1`/`*_TE_1`/`*_TF_1`）。
    pub timestamp_ms: Option<u64>,
}

impl SlavePoint {
    /// 品质良好、不带时标的点。
    pub fn new(ca: i32, ioa: i32, value: PointValue) -> Self {
        Self {
            ca,
            ioa,
            value,
            quality: quality::GOOD,
            timestamp_ms: None,
        }
    }
}

/// 子站创建参数。
#[derive(Debug, Clone)]
pub struct SlaveOptions {
    pub port: u16,
    /// 监听的本地地址，`None` 表示 0.0.0.0。
    pub local_address: Option<String>,
    /// 最大同时连接数，`0` 使用库默认值。
    pub max_connections: i32,
    /// 多个主站同时在线时，每个连接各自独立收到突发上送（否则只有一个激活连接收到）。
    pub multi_master: bool,
    /// 突发上送队列长度。
    pub queue_size: i32,
    pub k: i32,
    pub w: i32,
    pub t1: i32,
    pub t2: i32,
    pub t3: i32,
}

impl SlaveOptions {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            local_address: None,
            max_connections: 0,
            multi_master: false,
            queue_size: 2000,
            k: 12,
            w: 8,
            t1: 15,
            t2: 10,
            t3: 20,
        }
    }
}

/// 子站收到的命令值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandValue {
    Single(bool),
    Double(DoublePointValue),
    Step(StepCommandValue),
    Normalized(f32),
    Scaled(i32),
    Short(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceivedCommand {
    pub ca: i32,
    pub ioa: i32,
    /// 是否为"选择"（两步命令的第一步）。
    pub select: bool,
    pub value: CommandValue,
}

/// 命令回调：返回 `true` 回肯定确认，`false` 回否定确认。
/// 在库的连接线程里同步调用，不能阻塞。
pub type CommandHandler = Arc<dyn Fn(&ReceivedCommand) -> bool + Send + Sync>;

#[derive(Default)]
struct SlaveState {
    points: Mutex<HashMap<(i32, i32), SlavePoint>>,
    commands: Mutex<Vec<ReceivedCommand>>,
    handler: Mutex<Option<CommandHandler>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Slave {
    inner: ffi::CS104_Slave,
    state: Arc<SlaveState>,
    /// 交给 C 回调的 `Arc<SlaveState>` 裸指针，Drop 时回收。
    callback_param: *const SlaveState,
    started: bool,
}

// SAFETY: CS104_Slave 内部有自己的线程与锁，接口可跨线程调用；共享状态由 Mutex 保护。
unsafe impl Send for Slave {}
// SAFETY: 同上；`update`/`update_many` 等 `&self` 方法只调用库的线程安全接口。
unsafe impl Sync for Slave {}

impl Slave {
    /// 创建监听 `port` 的子站（尚未启动），`points` 为总召唤时上送的全部点。
    pub fn new(port: u16, points: Vec<SlavePoint>) -> Result<Self, Error> {
        Self::with_options(SlaveOptions::new(port), points)
    }

    pub fn with_options(options: SlaveOptions, points: Vec<SlavePoint>) -> Result<Self, Error> {
        let state = Arc::new(SlaveState {
            points: Mutex::new(points.into_iter().map(|p| ((p.ca, p.ioa), p)).collect()),
            ..Default::default()
        });
        let callback_param = Arc::into_raw(Arc::clone(&state));
        let inner = unsafe { ffi::CS104_Slave_create(options.queue_size, 100) };
        if inner.is_null() {
            unsafe { drop(Arc::from_raw(callback_param)) };
            return Err(Error::ConnectFailed);
        }
        unsafe {
            ffi::CS104_Slave_setLocalPort(inner, c_int::from(options.port));
            if let Some(addr) = options
                .local_address
                .as_deref()
                .and_then(|a| CString::new(a).ok())
            {
                // 库内部会拷贝字符串
                ffi::CS104_Slave_setLocalAddress(inner, addr.as_ptr());
            }
            if options.max_connections > 0 {
                ffi::CS104_Slave_setMaxOpenConnections(inner, options.max_connections);
            }
            ffi::CS104_Slave_setServerMode(
                inner,
                if options.multi_master {
                    ffi::CS104_ServerMode_CS104_MODE_CONNECTION_IS_REDUNDANCY_GROUP
                } else {
                    ffi::CS104_ServerMode_CS104_MODE_SINGLE_REDUNDANCY_GROUP
                },
            );
            let apci = ffi::CS104_Slave_getConnectionParameters(inner);
            (*apci).k = options.k;
            (*apci).w = options.w;
            (*apci).t1 = options.t1;
            (*apci).t2 = options.t2;
            (*apci).t3 = options.t3;
            ffi::CS104_Slave_setInterrogationHandler(
                inner,
                Some(interrogation_trampoline),
                callback_param as *mut c_void,
            );
            ffi::CS104_Slave_setClockSyncHandler(
                inner,
                Some(clock_sync_trampoline),
                callback_param as *mut c_void,
            );
            ffi::CS104_Slave_setASDUHandler(
                inner,
                Some(asdu_trampoline),
                callback_param as *mut c_void,
            );
        }
        Ok(Self {
            inner,
            state,
            callback_param,
            started: false,
        })
    }

    /// 设置命令回调。设置后收到的命令不再进入 [`Slave::take_commands`] 队列。
    pub fn set_command_handler(
        &self,
        handler: impl Fn(&ReceivedCommand) -> bool + Send + Sync + 'static,
    ) {
        *lock(&self.state.handler) = Some(Arc::new(handler));
    }

    pub fn start(&mut self) {
        unsafe { ffi::CS104_Slave_start(self.inner) };
        self.started = true;
    }

    pub fn stop(&mut self) {
        if self.started {
            unsafe { ffi::CS104_Slave_stop(self.inner) };
            self.started = false;
        }
    }

    /// 当前已建立的主站连接数。
    pub fn open_connections(&self) -> i32 {
        unsafe { ffi::CS104_Slave_getOpenConnections(self.inner) }
    }

    /// 取走目前收到的全部命令。
    pub fn take_commands(&self) -> Vec<ReceivedCommand> {
        std::mem::take(&mut *lock(&self.state.commands))
    }

    /// 修改（不存在则新增）一个点的值，并以 COT=突发 向已激活的主站上送。
    pub fn update(&self, point: SlavePoint) {
        self.update_many(&[point]);
    }

    /// 批量修改点并以 COT=突发 上送。
    pub fn update_many(&self, points: &[SlavePoint]) {
        {
            let mut stored = lock(&self.state.points);
            for p in points {
                stored.insert((p.ca, p.ioa), *p);
            }
        }
        unsafe {
            let params = ffi::CS104_Slave_getAppLayerParameters(self.inner);
            for point in points {
                if let Some(asdu) = build_asdu(
                    params,
                    ffi::CS101_CauseOfTransmission_CS101_COT_SPONTANEOUS,
                    point,
                ) {
                    ffi::CS104_Slave_enqueueASDU(self.inner, asdu);
                    ffi::CS101_ASDU_destroy(asdu);
                }
            }
        }
    }
}

impl Drop for Slave {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                ffi::CS104_Slave_stop(self.inner);
            }
            // destroy 之后不会再有回调，之后才能回收回调参数
            ffi::CS104_Slave_destroy(self.inner);
            drop(Arc::from_raw(self.callback_param));
        }
    }
}

/// 为一个点构造只含一个信息对象的 ASDU，调用方负责 `CS101_ASDU_destroy`。
unsafe fn build_asdu(
    params: ffi::CS101_AppLayerParameters,
    cot: ffi::CS101_CauseOfTransmission,
    point: &SlavePoint,
) -> Option<ffi::CS101_ASDU> {
    unsafe {
        let asdu = ffi::CS101_ASDU_create(params, false, cot, 0, point.ca, false, false);
        if asdu.is_null() {
            return None;
        }
        let quality = point.quality;
        let ioa = point.ioa;
        // 时标对象在 create 内部被拷贝，局部变量即可
        let mut time = ffi::sCP56Time2a::default();
        let ts: Option<ffi::CP56Time2a> = point.timestamp_ms.map(|ms| {
            ffi::CP56Time2a_createFromMsTimestamp(&mut time, ms);
            &mut time as ffi::CP56Time2a
        });
        let io: ffi::InformationObject = match (point.value, ts) {
            (PointValue::Single(v), None) => {
                ffi::SinglePointInformation_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            (PointValue::Single(v), Some(t)) => {
                ffi::SinglePointWithCP56Time2a_create(ptr::null_mut(), ioa, v, quality, t)
                    as ffi::InformationObject
            }
            (PointValue::Double(v), None) => ffi::DoublePointInformation_create(
                ptr::null_mut(),
                ioa,
                v.to_raw() as ffi::DoublePointValue,
                quality,
            ) as ffi::InformationObject,
            (PointValue::Double(v), Some(t)) => ffi::DoublePointWithCP56Time2a_create(
                ptr::null_mut(),
                ioa,
                v.to_raw() as ffi::DoublePointValue,
                quality,
                t,
            ) as ffi::InformationObject,
            (PointValue::Normalized(v), None) => {
                ffi::MeasuredValueNormalized_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            (PointValue::Normalized(v), Some(t)) => {
                ffi::MeasuredValueNormalizedWithCP56Time2a_create(
                    ptr::null_mut(),
                    ioa,
                    v,
                    quality,
                    t,
                ) as ffi::InformationObject
            }
            (PointValue::Scaled(v), None) => {
                ffi::MeasuredValueScaled_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            (PointValue::Scaled(v), Some(t)) => {
                ffi::MeasuredValueScaledWithCP56Time2a_create(ptr::null_mut(), ioa, v, quality, t)
                    as ffi::InformationObject
            }
            (PointValue::Short(v), None) => {
                ffi::MeasuredValueShort_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            (PointValue::Short(v), Some(t)) => {
                ffi::MeasuredValueShortWithCP56Time2a_create(ptr::null_mut(), ioa, v, quality, t)
                    as ffi::InformationObject
            }
        };
        if io.is_null() {
            ffi::CS101_ASDU_destroy(asdu);
            return None;
        }
        // addInformationObject 会拷贝对象内容，原对象需自行释放
        let ok = ffi::CS101_ASDU_addInformationObject(asdu, io);
        ffi::InformationObject_destroy(io);
        if !ok {
            ffi::CS101_ASDU_destroy(asdu);
            return None;
        }
        Some(asdu)
    }
}

unsafe extern "C" fn interrogation_trampoline(
    parameter: *mut c_void,
    connection: ffi::IMasterConnection,
    asdu: ffi::CS101_ASDU,
    qoi: u8,
) -> bool {
    if parameter.is_null() || qoi != ffi::IEC60870_QOI_STATION as u8 {
        return false;
    }
    unsafe {
        let state = &*(parameter as *const SlaveState);
        let ca = ffi::CS101_ASDU_getCA(asdu);
        ffi::IMasterConnection_sendACT_CON(connection, asdu, false);
        let params = ffi::IMasterConnection_getApplicationLayerParameters(connection);
        let mut points: Vec<SlavePoint> = lock(&state.points)
            .values()
            .filter(|p| p.ca == ca)
            .copied()
            .collect();
        points.sort_by_key(|p| p.ioa);
        for point in &points {
            if let Some(out) = build_asdu(
                params,
                ffi::CS101_CauseOfTransmission_CS101_COT_INTERROGATED_BY_STATION,
                point,
            ) {
                ffi::IMasterConnection_sendASDU(connection, out);
                ffi::CS101_ASDU_destroy(out);
            }
        }
        ffi::IMasterConnection_sendACT_TERM(connection, asdu);
    }
    true
}

unsafe extern "C" fn asdu_trampoline(
    parameter: *mut c_void,
    connection: ffi::IMasterConnection,
    asdu: ffi::CS101_ASDU,
) -> bool {
    if parameter.is_null() {
        return false;
    }
    unsafe {
        let state = &*(parameter as *const SlaveState);
        if ffi::CS101_ASDU_getCOT(asdu) != ffi::CS101_CauseOfTransmission_CS101_COT_ACTIVATION {
            return false;
        }
        let io = ffi::CS101_ASDU_getElement(asdu, 0);
        if io.is_null() {
            return false;
        }
        let ca = ffi::CS101_ASDU_getCA(asdu);
        let ioa = ffi::InformationObject_getObjectAddress(io);
        let decoded = match ffi::CS101_ASDU_getTypeID(asdu) {
            ffi::IEC60870_5_TypeID_C_SC_NA_1 => {
                let c = io as ffi::SingleCommand;
                Some((
                    ffi::SingleCommand_isSelect(c),
                    CommandValue::Single(ffi::SingleCommand_getState(c)),
                ))
            }
            ffi::IEC60870_5_TypeID_C_DC_NA_1 => {
                let c = io as ffi::DoubleCommand;
                Some((
                    ffi::DoubleCommand_isSelect(c),
                    CommandValue::Double(DoublePointValue::from_raw(
                        ffi::DoubleCommand_getState(c) as ffi::DoublePointValue,
                    )),
                ))
            }
            ffi::IEC60870_5_TypeID_C_RC_NA_1 => {
                let c = io as ffi::StepCommand;
                let step = match ffi::StepCommand_getState(c) {
                    ffi::StepCommandValue_IEC60870_STEP_LOWER => StepCommandValue::Lower,
                    ffi::StepCommandValue_IEC60870_STEP_HIGHER => StepCommandValue::Higher,
                    ffi::StepCommandValue_IEC60870_STEP_INVALID_3 => StepCommandValue::Invalid3,
                    _ => StepCommandValue::Invalid0,
                };
                Some((ffi::StepCommand_isSelect(c), CommandValue::Step(step)))
            }
            ffi::IEC60870_5_TypeID_C_SE_NA_1 => {
                let c = io as ffi::SetpointCommandNormalized;
                Some((
                    ffi::SetpointCommandNormalized_isSelect(c),
                    CommandValue::Normalized(ffi::SetpointCommandNormalized_getValue(c)),
                ))
            }
            ffi::IEC60870_5_TypeID_C_SE_NB_1 => {
                let c = io as ffi::SetpointCommandScaled;
                Some((
                    ffi::SetpointCommandScaled_isSelect(c),
                    CommandValue::Scaled(ffi::SetpointCommandScaled_getValue(c)),
                ))
            }
            ffi::IEC60870_5_TypeID_C_SE_NC_1 => {
                let c = io as ffi::SetpointCommandShort;
                Some((
                    ffi::SetpointCommandShort_isSelect(c),
                    CommandValue::Short(ffi::SetpointCommandShort_getValue(c)),
                ))
            }
            _ => None,
        };
        ffi::InformationObject_destroy(io);
        let Some((select, value)) = decoded else {
            return false;
        };
        let cmd = ReceivedCommand {
            ca,
            ioa,
            select,
            value,
        };
        let handler = lock(&state.handler).clone();
        let accepted = match handler {
            // panic 不能穿过 FFI 边界，按拒绝处理
            Some(h) => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(&cmd))).unwrap_or(false)
            }
            None => {
                lock(&state.commands).push(cmd);
                true
            }
        };
        ffi::IMasterConnection_sendACT_CON(connection, asdu, !accepted);
    }
    true
}

/// 收到时钟同步命令：只回确认，不修改本机系统时间。
unsafe extern "C" fn clock_sync_trampoline(
    _parameter: *mut c_void,
    _connection: ffi::IMasterConnection,
    _asdu: ffi::CS101_ASDU,
    _new_time: ffi::CP56Time2a,
) -> bool {
    true
}
