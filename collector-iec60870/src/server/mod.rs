//! IEC 104 子站（服务端）的最小实现，主要用于主站联调与集成测试。
//!
//! 行为：
//! - 收到站总召唤时，先回 ACT_CON，再把已配置的全部点逐个以 COT=响应站召唤上送，最后回 ACT_TERM；
//! - 收到控制命令（遥控/遥调/步位置）时记录下来并回 ACT_CON；
//! - [`Slave::update`] 修改点值并以 COT=突发 主动上送。

use std::{
    ffi::c_void,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlavePoint {
    pub ca: i32,
    pub ioa: i32,
    pub value: PointValue,
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

#[derive(Default)]
struct SlaveState {
    points: Mutex<Vec<SlavePoint>>,
    commands: Mutex<Vec<ReceivedCommand>>,
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

impl Slave {
    /// 创建监听 `port` 的子站（尚未启动），`points` 为总召唤时上送的全部点。
    pub fn new(port: u16, points: Vec<SlavePoint>) -> Result<Self, Error> {
        let state = Arc::new(SlaveState {
            points: Mutex::new(points),
            commands: Mutex::new(Vec::new()),
        });
        let callback_param = Arc::into_raw(Arc::clone(&state));
        let inner = unsafe { ffi::CS104_Slave_create(100, 100) };
        if inner.is_null() {
            unsafe { drop(Arc::from_raw(callback_param)) };
            return Err(Error::ConnectFailed);
        }
        unsafe {
            ffi::CS104_Slave_setLocalPort(inner, c_int::from(port));
            ffi::CS104_Slave_setServerMode(
                inner,
                ffi::CS104_ServerMode_CS104_MODE_SINGLE_REDUNDANCY_GROUP,
            );
            ffi::CS104_Slave_setInterrogationHandler(
                inner,
                Some(interrogation_trampoline),
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
        {
            let mut points = lock(&self.state.points);
            match points
                .iter_mut()
                .find(|p| p.ca == point.ca && p.ioa == point.ioa)
            {
                Some(p) => p.value = point.value,
                None => points.push(point),
            }
        }
        unsafe {
            let params = ffi::CS104_Slave_getAppLayerParameters(self.inner);
            if let Some(asdu) = build_asdu(
                params,
                ffi::CS101_CauseOfTransmission_CS101_COT_SPONTANEOUS,
                &point,
            ) {
                ffi::CS104_Slave_enqueueASDU(self.inner, asdu);
                ffi::CS101_ASDU_destroy(asdu);
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
        let quality = ffi::IEC60870_QUALITY_GOOD as u8;
        let ioa = point.ioa;
        let io: ffi::InformationObject = match point.value {
            PointValue::Single(v) => {
                ffi::SinglePointInformation_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            PointValue::Double(v) => ffi::DoublePointInformation_create(
                ptr::null_mut(),
                ioa,
                v.to_raw() as ffi::DoublePointValue,
                quality,
            ) as ffi::InformationObject,
            PointValue::Normalized(v) => {
                ffi::MeasuredValueNormalized_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            PointValue::Scaled(v) => {
                ffi::MeasuredValueScaled_create(ptr::null_mut(), ioa, v, quality)
                    as ffi::InformationObject
            }
            PointValue::Short(v) => {
                ffi::MeasuredValueShort_create(ptr::null_mut(), ioa, v, quality)
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
        let points: Vec<SlavePoint> = lock(&state.points)
            .iter()
            .filter(|p| p.ca == ca)
            .copied()
            .collect();
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
        lock(&state.commands).push(ReceivedCommand {
            ca,
            ioa,
            select,
            value,
        });
        ffi::IMasterConnection_sendACT_CON(connection, asdu, false);
    }
    true
}
