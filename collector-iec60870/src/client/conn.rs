use std::{
    ffi::{CString, c_void},
    os::raw::c_int,
    ptr,
};

use crate::{
    client::{Asdu, DoublePointValue, Error, StepCommandValue},
    ffi,
};

#[derive(Debug, Clone, Copy)]
pub struct ApciParameters {
    pub k: i32,
    pub w: i32,
    pub t0: i32,
    pub t1: i32,
    pub t2: i32,
    pub t3: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionEvent {
    Opened,
    Closed,
    StartDtConfirmed,
    StopDtConfirmed,
    Failed,
}

impl ConnectionEvent {
    fn from_raw(raw: ffi::CS104_ConnectionEvent) -> Self {
        match raw {
            ffi::CS104_ConnectionEvent_CS104_CONNECTION_OPENED => Self::Opened,
            ffi::CS104_ConnectionEvent_CS104_CONNECTION_CLOSED => Self::Closed,
            ffi::CS104_ConnectionEvent_CS104_CONNECTION_STARTDT_CON_RECEIVED => {
                Self::StartDtConfirmed
            }
            ffi::CS104_ConnectionEvent_CS104_CONNECTION_STOPDT_CON_RECEIVED => {
                Self::StopDtConfirmed
            }
            _ => Self::Failed,
        }
    }
}

type AsduCallback = Box<dyn FnMut(i32, Asdu) -> bool + Send>;
type ConnectionCallback = Box<dyn FnMut(ConnectionEvent) + Send>;

pub struct Client {
    inner: ffi::CS104_Connection,
    asdu_handler: Option<Box<AsduCallback>>,
    connection_handler: Option<Box<ConnectionCallback>>,
}

impl Client {
    /// 仅创建连接对象，不建立 TCP 连接。APCI 参数、原发地址和回调都必须在 [`Client::connect`]
    /// 之前设置好，之后修改行为未定义（lib60870-C 文档要求）。
    pub fn new(host: impl AsRef<str>, port: u16) -> Result<Self, Error> {
        let host_cstring = CString::new(host.as_ref())?;
        let inner = unsafe { ffi::CS104_Connection_create(host_cstring.as_ptr(), port as c_int) };
        if inner.is_null() {
            return Err(Error::ConnectFailed);
        }
        Ok(Self {
            inner,
            asdu_handler: None,
            connection_handler: None,
        })
    }

    /// 设置 APCI 参数（K/W/T0-T3），必须在 [`Client::connect`] 之前调用。
    /// 直接修改底层结构体字段，而非调用 `setAPCIParameters`（后者要求调用方保证外部指针长期有效）。
    pub fn set_apci_parameters(&mut self, params: ApciParameters) {
        unsafe {
            let raw = ffi::CS104_Connection_getAPCIParameters(self.inner);
            (*raw).k = params.k;
            (*raw).w = params.w;
            (*raw).t0 = params.t0;
            (*raw).t1 = params.t1;
            (*raw).t2 = params.t2;
            (*raw).t3 = params.t3;
        }
    }

    /// 设置本机的原发地址（Originator Address），随时可调，不要求在 `connect` 之前。
    pub fn set_originator_address(&mut self, address: u8) {
        unsafe {
            ffi::CS104_Connection_setOriginatorAddress(self.inner, address);
        }
    }

    /// 注册收到 ASDU 时的回调，必须在 [`Client::connect`] 之前调用。
    /// `handler` 的返回值会原样传给底层库：返回 `false` 表示"未处理"，库会打印警告日志。
    pub fn set_asdu_received_handler<F>(&mut self, handler: F)
    where
        F: FnMut(i32, Asdu) -> bool + Send + 'static,
    {
        let boxed: Box<AsduCallback> = Box::new(Box::new(handler));
        let parameter = Box::into_raw(boxed);
        unsafe {
            ffi::CS104_Connection_setASDUReceivedHandler(
                self.inner,
                Some(asdu_trampoline),
                parameter as *mut c_void,
            );
            self.asdu_handler = Some(Box::from_raw(parameter));
        }
    }

    /// 注册连接状态变化（打开/关闭/STARTDT-STOPDT 确认/失败）的回调，必须在 [`Client::connect`] 之前调用。
    pub fn set_connection_event_handler<F>(&mut self, handler: F)
    where
        F: FnMut(ConnectionEvent) + Send + 'static,
    {
        let boxed: Box<ConnectionCallback> = Box::new(Box::new(handler));
        let parameter = Box::into_raw(boxed);
        unsafe {
            ffi::CS104_Connection_setConnectionHandler(
                self.inner,
                Some(connection_trampoline),
                parameter as *mut c_void,
            );
            self.connection_handler = Some(Box::from_raw(parameter));
        }
    }

    /// 阻塞地建立 TCP 连接并完成 IEC 104 启动握手，超时或失败返回 `Err`。
    /// 连接成功后不会自动启动数据传输，仍需调用 [`Client::start_data_transfer`]。
    pub fn connect(&mut self) -> Result<(), Error> {
        let ok = unsafe { ffi::CS104_Connection_connect(self.inner) };
        if ok {
            Ok(())
        } else {
            Err(Error::ConnectFailed)
        }
    }

    pub fn is_connected(&self) -> bool {
        unsafe { ffi::CS104_Connection_isConnected(self.inner) }
    }

    /// 发送 STARTDT，请求对端开始向本机传输数据（遥信/遥测等）。
    pub fn start_data_transfer(&mut self) {
        unsafe { ffi::CS104_Connection_sendStartDT(self.inner) };
    }

    /// 发送 STOPDT，请求对端停止向本机传输数据；连接本身保持打开。
    pub fn stop_data_transfer(&mut self) {
        unsafe { ffi::CS104_Connection_sendStopDT(self.inner) };
    }

    /// 关闭底层 TCP 连接（不销毁连接对象，理论上可再次 [`Client::connect`]）。
    pub fn disconnect(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                ffi::CS104_Connection_close(self.inner);
            }
        }
    }

    /// 发送总召唤命令（站级，QOI=20），COT 固定为 ACTIVATION。
    /// `ca` 是公共地址（Common Address），对端会随后以 COT=ACTIVATION_CON 确认，再逐个上报当前所有遥信/遥测值。
    pub fn send_interrogation_command(&mut self, ca: i32) -> Result<(), Error> {
        let ok = unsafe {
            ffi::CS104_Connection_sendInterrogationCommand(
                self.inner,
                ffi::CS101_CauseOfTransmission_CS101_COT_ACTIVATION,
                ca,
                ffi::IEC60870_QOI_STATION as u8,
            )
        };
        if ok {
            Ok(())
        } else {
            Err(Error::SendFailed("总召唤命令"))
        }
    }

    /// 发送时钟同步命令，`timestamp_ms` 为 Unix 毫秒时间戳，内部转换为 CP56Time2a 格式。
    pub fn send_clock_sync_command(&mut self, ca: i32, timestamp_ms: u64) -> Result<(), Error> {
        let mut time = ffi::sCP56Time2a {
            encodedValue: [0u8; 7],
        };
        let ok = unsafe {
            ffi::CP56Time2a_createFromMsTimestamp(&mut time, timestamp_ms);
            ffi::CS104_Connection_sendClockSyncCommand(self.inner, ca, &mut time)
        };
        if ok {
            Ok(())
        } else {
            Err(Error::SendFailed("时钟同步命令"))
        }
    }

    /// 发送单点遥控命令（分/合）。
    /// `select`: `true` 为"选择"（两步命令的第一步），`false` 为直接"执行"。
    /// `qu`: 输出方式限定词，0=无附加定义，1=短脉冲，2=长脉冲，3=持续输出。
    pub fn send_single_command(
        &mut self,
        ca: i32,
        ioa: i32,
        value: bool,
        select: bool,
        qu: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command = ffi::SingleCommand_create(ptr::null_mut(), ioa, value, select, qu);
            self.send_command_object(ca, command as ffi::InformationObject, "单点遥控命令")
        }
    }

    /// 发送双点遥控命令。`select`/`qu` 含义同 [`Client::send_single_command`]。
    pub fn send_double_command(
        &mut self,
        ca: i32,
        ioa: i32,
        value: DoublePointValue,
        select: bool,
        qu: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command =
                ffi::DoubleCommand_create(ptr::null_mut(), ioa, value.to_raw(), select, qu);
            self.send_command_object(ca, command as ffi::InformationObject, "双点遥控命令")
        }
    }

    /// 发送步位置命令（升/降）。`select`/`qu` 含义同 [`Client::send_single_command`]。
    pub fn send_step_command(
        &mut self,
        ca: i32,
        ioa: i32,
        value: StepCommandValue,
        select: bool,
        qu: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command = ffi::StepCommand_create(ptr::null_mut(), ioa, value.to_raw(), select, qu);
            self.send_command_object(ca, command as ffi::InformationObject, "步位置命令")
        }
    }

    /// 发送归一化设定值命令，`value` 为归一化值（-1.0 ~ 1.0）。
    /// `select` 含义同 [`Client::send_single_command`]；`ql` 为设定值限定词（0=默认，1=短脉冲，2=长脉冲，3=持续输出）。
    pub fn send_setpoint_command_normalized(
        &mut self,
        ca: i32,
        ioa: i32,
        value: f32,
        select: bool,
        ql: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command =
                ffi::SetpointCommandNormalized_create(ptr::null_mut(), ioa, value, select, ql);
            self.send_command_object(ca, command as ffi::InformationObject, "归一化设定值命令")
        }
    }

    /// 发送标度化设定值命令，`value` 为标度化整数值。`select`/`ql` 含义同 [`Client::send_setpoint_command_normalized`]。
    pub fn send_setpoint_command_scaled(
        &mut self,
        ca: i32,
        ioa: i32,
        value: i32,
        select: bool,
        ql: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command =
                ffi::SetpointCommandScaled_create(ptr::null_mut(), ioa, value, select, ql);
            self.send_command_object(ca, command as ffi::InformationObject, "标度化设定值命令")
        }
    }

    /// 发送短浮点设定值命令，`value` 为 IEEE 754 短浮点数。`select`/`ql` 含义同 [`Client::send_setpoint_command_normalized`]。
    pub fn send_setpoint_command_short(
        &mut self,
        ca: i32,
        ioa: i32,
        value: f32,
        select: bool,
        ql: i32,
    ) -> Result<(), Error> {
        unsafe {
            let command = ffi::SetpointCommandShort_create(ptr::null_mut(), ioa, value, select, ql);
            self.send_command_object(ca, command as ffi::InformationObject, "短浮点设定值命令")
        }
    }

    /// 发送一个已构造好的命令信息对象，并在发送后（无论成功与否）立即销毁它。
    unsafe fn send_command_object(
        &mut self,
        ca: i32,
        command: ffi::InformationObject,
        name: &'static str,
    ) -> Result<(), Error> {
        unsafe {
            if command.is_null() {
                return Err(Error::SendFailed(name));
            }
            let ok = ffi::CS104_Connection_sendProcessCommandEx(
                self.inner,
                ffi::CS101_CauseOfTransmission_CS101_COT_ACTIVATION,
                ca,
                command,
            );
            ffi::InformationObject_destroy(command);
            if ok {
                Ok(())
            } else {
                Err(Error::SendFailed(name))
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                ffi::CS104_Connection_close(self.inner);
                ffi::CS104_Connection_destroy(self.inner);
            }
        }
    }
}

unsafe extern "C" fn asdu_trampoline(
    parameter: *mut c_void,
    address: c_int,
    asdu: ffi::CS101_ASDU,
) -> bool {
    if parameter.is_null() {
        return false;
    }
    unsafe {
        let handler = &mut *(parameter as *mut AsduCallback);
        let asdu = Asdu::from_raw(asdu);
        handler(address, asdu)
    }
}

unsafe extern "C" fn connection_trampoline(
    parameter: *mut c_void,
    _connection: ffi::CS104_Connection,
    event: ffi::CS104_ConnectionEvent,
) {
    if parameter.is_null() {
        return;
    }
    unsafe {
        let handler = &mut *(parameter as *mut ConnectionCallback);
        handler(ConnectionEvent::from_raw(event));
    }
}
