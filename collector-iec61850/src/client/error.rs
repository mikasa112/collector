use crate::ffi;

/// 控制操作失败时 IED 返回的 LastApplError。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlFailure {
    /// 原始 `ControlLastApplError` 值(0 表示无错误)。
    pub error: u32,
    /// 原始 `ControlAddCause` 值, 含义见 IEC 61850-7-2 AddCause。
    pub add_cause: u32,
    pub ctl_num: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("无效参数: {0}")]
    InvalidArgument(String),
    #[error("创建 IedConnection 失败")]
    CreateFailed,
    #[error("IEC 61850 客户端错误 {code} ({name})")]
    Client { code: u32, name: &'static str },
    #[error("无效的对象引用 {0}")]
    Reference(String),
    #[error("报告控制块 {0} 已被其他客户端占用")]
    RcbBusy(String),
    #[error("数据访问错误 {code} ({name})")]
    DataAccess { code: u32, name: &'static str },
    #[error("控制失败 (客户端错误 {client_code}, LastApplError {}, AddCause {})", .failure.error, .failure.add_cause)]
    Control {
        client_code: u32,
        failure: ControlFailure,
    },
    #[error("不支持的值类型: {0}")]
    UnsupportedValue(String),
}

impl Error {
    /// 把 `IedClientError` 转成 `Result`。
    pub(crate) fn check(code: ffi::IedClientError) -> Result<(), Error> {
        if code == ffi::IedClientError_IED_ERROR_OK {
            Ok(())
        } else {
            Err(Error::from_code(code))
        }
    }

    pub(crate) fn from_code(code: ffi::IedClientError) -> Error {
        Error::Client {
            code,
            name: code_name(code),
        }
    }

    pub(crate) fn data_access(code: u32) -> Error {
        // MMS DataAccessError, 见 ISO 9506-1。
        let name = match code {
            0 => "OBJECT_INVALIDATED",
            1 => "HARDWARE_FAULT",
            2 => "TEMPORARILY_UNAVAILABLE",
            3 => "OBJECT_ACCESS_DENIED",
            4 => "OBJECT_UNDEFINED",
            5 => "INVALID_ADDRESS",
            6 => "TYPE_UNSUPPORTED",
            7 => "TYPE_INCONSISTENT",
            8 => "OBJECT_ATTRIBUTE_INCONSISTENT",
            9 => "OBJECT_ACCESS_UNSUPPORTED",
            10 => "OBJECT_NON_EXISTENT",
            11 => "OBJECT_VALUE_INVALID",
            _ => "UNKNOWN",
        };
        Error::DataAccess { code, name }
    }

    /// 底层连接已断开或未建立, 调用方应当重连。
    pub fn is_connection_lost(&self) -> bool {
        let code = match self {
            Error::Client { code, .. } => *code,
            Error::Control { client_code, .. } => *client_code,
            _ => return false,
        };
        code == ffi::IedClientError_IED_ERROR_NOT_CONNECTED
            || code == ffi::IedClientError_IED_ERROR_CONNECTION_LOST
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Error::Client { code, .. } if *code == ffi::IedClientError_IED_ERROR_TIMEOUT)
    }
}

#[allow(non_upper_case_globals)]
fn code_name(code: ffi::IedClientError) -> &'static str {
    use ffi::*;
    match code {
        IedClientError_IED_ERROR_OK => "OK",
        IedClientError_IED_ERROR_NOT_CONNECTED => "NOT_CONNECTED",
        IedClientError_IED_ERROR_ALREADY_CONNECTED => "ALREADY_CONNECTED",
        IedClientError_IED_ERROR_CONNECTION_LOST => "CONNECTION_LOST",
        IedClientError_IED_ERROR_SERVICE_NOT_SUPPORTED => "SERVICE_NOT_SUPPORTED",
        IedClientError_IED_ERROR_CONNECTION_REJECTED => "CONNECTION_REJECTED",
        IedClientError_IED_ERROR_OUTSTANDING_CALL_LIMIT_REACHED => "OUTSTANDING_CALL_LIMIT_REACHED",
        IedClientError_IED_ERROR_USER_PROVIDED_INVALID_ARGUMENT => "USER_PROVIDED_INVALID_ARGUMENT",
        IedClientError_IED_ERROR_ENABLE_REPORT_FAILED_DATASET_MISMATCH => {
            "ENABLE_REPORT_FAILED_DATASET_MISMATCH"
        }
        IedClientError_IED_ERROR_OBJECT_REFERENCE_INVALID => "OBJECT_REFERENCE_INVALID",
        IedClientError_IED_ERROR_UNEXPECTED_VALUE_RECEIVED => "UNEXPECTED_VALUE_RECEIVED",
        IedClientError_IED_ERROR_TIMEOUT => "TIMEOUT",
        IedClientError_IED_ERROR_ACCESS_DENIED => "ACCESS_DENIED",
        IedClientError_IED_ERROR_OBJECT_DOES_NOT_EXIST => "OBJECT_DOES_NOT_EXIST",
        IedClientError_IED_ERROR_OBJECT_EXISTS => "OBJECT_EXISTS",
        IedClientError_IED_ERROR_OBJECT_ACCESS_UNSUPPORTED => "OBJECT_ACCESS_UNSUPPORTED",
        IedClientError_IED_ERROR_TYPE_INCONSISTENT => "TYPE_INCONSISTENT",
        IedClientError_IED_ERROR_TEMPORARILY_UNAVAILABLE => "TEMPORARILY_UNAVAILABLE",
        IedClientError_IED_ERROR_OBJECT_UNDEFINED => "OBJECT_UNDEFINED",
        IedClientError_IED_ERROR_INVALID_ADDRESS => "INVALID_ADDRESS",
        IedClientError_IED_ERROR_HARDWARE_FAULT => "HARDWARE_FAULT",
        IedClientError_IED_ERROR_TYPE_UNSUPPORTED => "TYPE_UNSUPPORTED",
        IedClientError_IED_ERROR_OBJECT_ATTRIBUTE_INCONSISTENT => "OBJECT_ATTRIBUTE_INCONSISTENT",
        IedClientError_IED_ERROR_OBJECT_VALUE_INVALID => "OBJECT_VALUE_INVALID",
        IedClientError_IED_ERROR_OBJECT_INVALIDATED => "OBJECT_INVALIDATED",
        IedClientError_IED_ERROR_MALFORMED_MESSAGE => "MALFORMED_MESSAGE",
        IedClientError_IED_ERROR_OBJECT_CONSTRAINT_CONFLICT => "OBJECT_CONSTRAINT_CONFLICT",
        IedClientError_IED_ERROR_SERVICE_NOT_IMPLEMENTED => "SERVICE_NOT_IMPLEMENTED",
        _ => "UNKNOWN",
    }
}
