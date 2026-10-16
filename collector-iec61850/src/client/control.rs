use crate::{
    client::{ControlFailure, Error, conn::cstring, Connection},
    ffi,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ControlOptions {
    /// 置 Test 位, IED 只做校验不真正动作。
    pub test: bool,
    pub interlock_check: bool,
    pub synchro_check: bool,
    /// 定时控制的执行时刻, 毫秒(Unix epoch); 0 表示立即执行。
    pub oper_time_ms: u64,
}

struct ControlClient(ffi::ControlObjectClient);

impl Drop for ControlClient {
    fn drop(&mut self) {
        unsafe { ffi::ControlObjectClient_destroy(self.0) };
    }
}

struct Mms(*mut ffi::MmsValue);

impl Drop for Mms {
    fn drop(&mut self) {
        unsafe { ffi::MmsValue_delete(self.0) };
    }
}

impl Connection {
    /// 对控制对象(DO, 如 `IED1LD0/CSWI1.Pos`)下发遥控/遥调。
    ///
    /// 控制模型(direct/SBO, normal/enhanced)由 IED 的 `ctlModel` 决定, 这里自动选择:
    /// SBO normal 先 `select`, SBO enhanced 先 `selectWithValue`, 然后 `operate`。
    /// `value` 按 `ctlVal` 的类型转换: 布尔(非 0 为 true)、整型、无符号整型、浮点。
    /// APC(遥调, `ctlVal` 为 AnalogueValue 结构体)按 32 位浮点下发, 库会自动包装成结构体;
    /// 整型的 APC(`isIntegerNotFloat`)暂不支持。
    pub fn control(
        &self,
        object_ref: &str,
        value: f64,
        options: &ControlOptions,
    ) -> Result<(), Error> {
        let c_ref = cstring(object_ref)?;
        let client = unsafe { ffi::ControlObjectClient_create(c_ref.as_ptr(), self.inner) };
        if client.is_null() {
            return Err(Error::from_code(
                ffi::IedClientError_IED_ERROR_OBJECT_DOES_NOT_EXIST,
            ));
        }
        let client = ControlClient(client);

        let (model, ctl_type) = unsafe {
            (
                ffi::ControlObjectClient_getControlModel(client.0),
                ffi::ControlObjectClient_getCtlValType(client.0),
            )
        };
        let ctl_val = Mms(new_ctl_value(ctl_type, value)?);

        unsafe {
            ffi::ControlObjectClient_setTestMode(client.0, options.test);
            ffi::ControlObjectClient_setInterlockCheck(client.0, options.interlock_check);
            ffi::ControlObjectClient_setSynchroCheck(client.0, options.synchro_check);
        }

        let selected = match model {
            ffi::ControlModel_CONTROL_MODEL_STATUS_ONLY => {
                return Err(Error::InvalidArgument(format!(
                    "{object_ref} 的 ctlModel 是 status-only, 不可控制"
                )));
            }
            ffi::ControlModel_CONTROL_MODEL_SBO_NORMAL => unsafe {
                ffi::ControlObjectClient_select(client.0)
            },
            ffi::ControlModel_CONTROL_MODEL_SBO_ENHANCED => unsafe {
                ffi::ControlObjectClient_selectWithValue(client.0, ctl_val.0)
            },
            _ => true,
        };
        if !selected {
            return Err(control_error(&client));
        }

        let ok = unsafe { ffi::ControlObjectClient_operate(client.0, ctl_val.0, options.oper_time_ms) };
        if ok { Ok(()) } else { Err(control_error(&client)) }
    }
}

fn new_ctl_value(ctl_type: ffi::MmsType, value: f64) -> Result<*mut ffi::MmsValue, Error> {
    let v = unsafe {
        match ctl_type {
            ffi::MmsType_MMS_BOOLEAN => ffi::MmsValue_newBoolean(value != 0.0),
            ffi::MmsType_MMS_INTEGER => ffi::MmsValue_newIntegerFromInt32(value as i32),
            ffi::MmsType_MMS_UNSIGNED => ffi::MmsValue_newUnsignedFromUint32(value as u32),
            // APC 的 ctlVal 是结构体, libiec61850 允许直接传标量并自动包装。
            ffi::MmsType_MMS_FLOAT | ffi::MmsType_MMS_STRUCTURE => ffi::MmsValue_newFloat(value as f32),
            other => {
                return Err(Error::UnsupportedValue(format!(
                    "ctlVal 的 MMS 类型 {other} 暂不支持"
                )));
            }
        }
    };
    if v.is_null() {
        Err(Error::CreateFailed)
    } else {
        Ok(v)
    }
}

fn control_error(client: &ControlClient) -> Error {
    unsafe {
        let code = ffi::ControlObjectClient_getLastError(client.0);
        let appl = ffi::ControlObjectClient_getLastApplError(client.0);
        Error::Control {
            client_code: code,
            failure: ControlFailure {
                error: appl.error,
                add_cause: appl.addCause,
                ctl_num: appl.ctlNum,
            },
        }
    }
}
