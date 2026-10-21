use std::{
    ffi::{CStr, CString, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{Arc, Mutex},
};

use crate::{
    client::{DataSetLayout, Error, Fc, ObjectRef, TypeSpec, Value},
    ffi,
};

/// 触发条件(TrgOps)。
pub const TRG_DATA_CHANGED: u32 = ffi::TRG_OPT_DATA_CHANGED;
pub const TRG_QUALITY_CHANGED: u32 = ffi::TRG_OPT_QUALITY_CHANGED;
pub const TRG_DATA_UPDATE: u32 = ffi::TRG_OPT_DATA_UPDATE;
pub const TRG_INTEGRITY: u32 = ffi::TRG_OPT_INTEGRITY;
pub const TRG_GI: u32 = ffi::TRG_OPT_GI;

/// 报告里某个条目被包含的原因(ReasonForInclusion)。
pub const REASON_DATA_CHANGE: u32 = ffi::IEC61850_REASON_DATA_CHANGE;
pub const REASON_QUALITY_CHANGE: u32 = ffi::IEC61850_REASON_QUALITY_CHANGE;
pub const REASON_DATA_UPDATE: u32 = ffi::IEC61850_REASON_DATA_UPDATE;
pub const REASON_INTEGRITY: u32 = ffi::IEC61850_REASON_INTEGRITY;
pub const REASON_GI: u32 = ffi::IEC61850_REASON_GI;

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// MMS 建立连接(含 ACSE/COTP)的超时, 毫秒。
    pub connect_timeout_ms: u32,
    /// 单次 MMS 请求的超时, 毫秒。
    pub request_timeout_ms: u32,
    /// ISO 层地址参数; `None` 使用库默认值。
    pub iso: Option<IsoParams>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 10_000,
            request_timeout_ms: 5_000,
            iso: None,
        }
    }
}

/// ISO 层(ACSE/表示层/会话层/传输层)的地址参数, 数值取自 ICD 的 `<Address>` 段。
///
/// `remote_*` 是对端(IED)的地址, 即 ICD 里该 IED 的值; `local_*` 是本机(采集器)的地址。
/// 没填的项保持库的默认值:
/// TSEL `00 01`、SSEL `00 01`、PSEL `00 00 00 01`,
/// 对端 AP-Title `1.1.1.999.1` + AE 限定符 12, 本机 AP-Title `1.1.1.999` + AE 限定符 12。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IsoParams {
    /// 空格或点分隔均可, 如 `"1 3 9999 106"` 或 `"1.3.9999.106"`。
    pub remote_ap_title: Option<String>,
    pub remote_ae_qualifier: Option<i32>,
    /// 十六进制字节串, 如 `"00 00 00 01"`。
    pub remote_psel: Option<String>,
    pub remote_ssel: Option<String>,
    pub remote_tsel: Option<String>,
    pub local_ap_title: Option<String>,
    pub local_ae_qualifier: Option<i32>,
    pub local_psel: Option<String>,
    pub local_ssel: Option<String>,
    pub local_tsel: Option<String>,
}

const DEFAULT_TSEL: &[u8] = &[0, 1];
const DEFAULT_SSEL: &[u8] = &[0, 1];
const DEFAULT_PSEL: &[u8] = &[0, 0, 0, 1];
const DEFAULT_AE_QUALIFIER: i32 = 12;

impl IsoParams {
    /// 解析十六进制字节串, 允许空格、冒号、`-` 分隔, 如 `"00 01"`、`"0001"`、`"00:01"`。
    pub fn parse_selector(s: &str) -> Result<Vec<u8>, Error> {
        let hex: String = s
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, ':' | '-'))
            .collect();
        if hex.is_empty() || !hex.len().is_multiple_of(2) || !hex.is_ascii() {
            return Err(Error::InvalidArgument(format!("无效的选择器: {s:?}")));
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&hex[i..i + 2], 16)
                    .map_err(|_| Error::InvalidArgument(format!("无效的选择器: {s:?}")))
            })
            .collect()
    }

    fn normalize_ap_title(s: &str) -> Result<CString, Error> {
        let dotted = s
            .split(|c: char| c == '.' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if dotted.len() < 2 || dotted.iter().any(|p| p.parse::<u32>().is_err()) {
            return Err(Error::InvalidArgument(format!("无效的 AP-Title: {s:?}")));
        }
        cstring(&dotted.join("."))
    }

    fn selectors(
        psel: &Option<String>,
        ssel: &Option<String>,
        tsel: &Option<String>,
    ) -> Result<(ffi::PSelector, ffi::SSelector, ffi::TSelector), Error> {
        fn get(v: &Option<String>, default: &[u8]) -> Result<Vec<u8>, Error> {
            v.as_deref()
                .map(IsoParams::parse_selector)
                .unwrap_or_else(|| Ok(default.to_vec()))
        }
        let (p, s, t) = (
            get(psel, DEFAULT_PSEL)?,
            get(ssel, DEFAULT_SSEL)?,
            get(tsel, DEFAULT_TSEL)?,
        );
        let mut psel = ffi::PSelector::default();
        let mut ssel = ffi::SSelector::default();
        let mut tsel = ffi::TSelector::default();
        if p.len() > psel.value.len() || s.len() > ssel.value.len() || t.len() > tsel.value.len() {
            return Err(Error::InvalidArgument(format!(
                "选择器过长(PSEL 最长 {}、SSEL 最长 {}、TSEL 最长 {} 字节)",
                psel.value.len(),
                ssel.value.len(),
                tsel.value.len()
            )));
        }
        psel.size = p.len() as u8;
        psel.value[..p.len()].copy_from_slice(&p);
        ssel.size = s.len() as u8;
        ssel.value[..s.len()].copy_from_slice(&s);
        tsel.size = t.len() as u8;
        tsel.value[..t.len()].copy_from_slice(&t);
        Ok((psel, ssel, tsel))
    }

    /// 校验全部参数(选择器、AP-Title)但不连接, 供配置加载阶段提前报错。
    pub fn validate(&self) -> Result<(), Error> {
        Self::selectors(&self.remote_psel, &self.remote_ssel, &self.remote_tsel)?;
        Self::selectors(&self.local_psel, &self.local_ssel, &self.local_tsel)?;
        for ap in [&self.remote_ap_title, &self.local_ap_title]
            .into_iter()
            .flatten()
        {
            Self::normalize_ap_title(ap)?;
        }
        Ok(())
    }

    /// 先校验全部参数再写入, 出错时不会留下写了一半的状态。
    fn apply(&self, iso: ffi::IsoConnectionParameters) -> Result<(), Error> {
        let remote = Self::selectors(&self.remote_psel, &self.remote_ssel, &self.remote_tsel)?;
        let local = Self::selectors(&self.local_psel, &self.local_ssel, &self.local_tsel)?;
        let remote_ap = self
            .remote_ap_title
            .as_deref()
            .map(Self::normalize_ap_title)
            .transpose()?;
        let local_ap = self
            .local_ap_title
            .as_deref()
            .map(Self::normalize_ap_title)
            .transpose()?;
        unsafe {
            ffi::IsoConnectionParameters_setRemoteAddresses(iso, remote.0, remote.1, remote.2);
            ffi::IsoConnectionParameters_setLocalAddresses(iso, local.0, local.1, local.2);
            if let Some(ap) = &remote_ap {
                ffi::IsoConnectionParameters_setRemoteApTitle(
                    iso,
                    ap.as_ptr(),
                    self.remote_ae_qualifier.unwrap_or(DEFAULT_AE_QUALIFIER),
                );
            }
            if let Some(ap) = &local_ap {
                ffi::IsoConnectionParameters_setLocalApTitle(
                    iso,
                    ap.as_ptr(),
                    self.local_ae_qualifier.unwrap_or(DEFAULT_AE_QUALIFIER),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Closed,
    Connecting,
    Connected,
    Closing,
}

impl ConnectionState {
    fn from_raw(raw: ffi::IedConnectionState) -> Self {
        match raw {
            ffi::IedConnectionState_IED_STATE_CONNECTING => Self::Connecting,
            ffi::IedConnectionState_IED_STATE_CONNECTED => Self::Connected,
            ffi::IedConnectionState_IED_STATE_CLOSING => Self::Closing,
            _ => Self::Closed,
        }
    }
}

/// 报告控制块(RCB)的订阅参数。
#[derive(Debug, Clone)]
pub struct ReportConfig {
    /// 指定数据集引用(`LD/LN$DataSet` 或 `LD/LN.DataSet`); `None` 则沿用 IED 里已配置的数据集。
    pub data_set: Option<String>,
    /// 触发条件, 取 `TRG_*` 的按位或。
    pub trg_ops: u32,
    /// 完整性周期, 毫秒; 只有 `trg_ops` 含 `TRG_INTEGRITY` 时才有意义。
    pub intg_pd_ms: Option<u32>,
    /// 缓冲时间, 毫秒。
    pub buf_tm_ms: Option<u32>,
    /// 使能后立即触发一次总召(GI)。
    pub general_interrogation: bool,
    /// 仅对 BRCB 有效: 使能时清空 IED 里的缓冲。`false` 时会补发断线期间缓存的事件。
    pub purge_buffer: bool,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            data_set: None,
            trg_ops: TRG_DATA_CHANGED | TRG_QUALITY_CHANGED | TRG_GI,
            intg_pd_ms: None,
            buf_tm_ms: None,
            general_interrogation: true,
            purge_buffer: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReportEntry {
    /// 数据集成员下标。
    pub index: usize,
    /// 数据引用, 仅当 IED 在 OptFlds 里带了 data-reference 时才有。
    pub data_ref: Option<String>,
    /// `REASON_*` 的按位或。
    pub reason: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub rcb_ref: String,
    pub rpt_id: String,
    pub data_set: Option<String>,
    pub seq_num: Option<u16>,
    /// 报告时标, 毫秒(Unix epoch)。
    pub timestamp_ms: Option<u64>,
    pub conf_rev: Option<u32>,
    pub buffer_overflow: bool,
    pub entries: Vec<ReportEntry>,
}

impl Report {
    /// # Safety
    /// `report` 为回调里收到的有效 `ClientReport`, 仅在回调期间有效。
    unsafe fn from_raw(report: ffi::ClientReport) -> Report {
        unsafe {
            let has_reason = ffi::ClientReport_hasReasonForInclusion(report);
            let has_ref = ffi::ClientReport_hasDataReference(report);
            let values = ffi::ClientReport_getDataSetValues(report);

            let mut entries = Vec::new();
            if !values.is_null() {
                let n = ffi::MmsValue_getArraySize(values) as usize;
                for i in 0..n {
                    let reason = if has_reason {
                        ffi::ClientReport_getReasonForInclusion(report, i as _) as u32
                    } else {
                        0
                    };
                    let elem = ffi::MmsValue_getElement(values, i as _);
                    // 有 reason 时以 reason 判断是否包含; 没有则退化为元素非空。
                    let included = if has_reason {
                        reason != ffi::IEC61850_REASON_NOT_INCLUDED
                    } else {
                        !elem.is_null()
                    };
                    if !included {
                        continue;
                    }
                    entries.push(ReportEntry {
                        index: i,
                        data_ref: if has_ref {
                            opt_string(ffi::ClientReport_getDataReference(report, i as _))
                        } else {
                            None
                        },
                        reason,
                        value: Value::from_mms(elem),
                    });
                }
            }

            Report {
                rcb_ref: opt_string(ffi::ClientReport_getRcbReference(report)).unwrap_or_default(),
                rpt_id: opt_string(ffi::ClientReport_getRptId(report)).unwrap_or_default(),
                data_set: if ffi::ClientReport_hasDataSetName(report) {
                    opt_string(ffi::ClientReport_getDataSetName(report))
                } else {
                    None
                },
                seq_num: ffi::ClientReport_hasSeqNum(report)
                    .then(|| ffi::ClientReport_getSeqNum(report)),
                timestamp_ms: ffi::ClientReport_hasTimestamp(report)
                    .then(|| ffi::ClientReport_getTimestamp(report)),
                conf_rev: ffi::ClientReport_hasConfRev(report)
                    .then(|| ffi::ClientReport_getConfRev(report)),
                buffer_overflow: ffi::ClientReport_hasBufOvfl(report)
                    && ffi::ClientReport_getBufOvfl(report),
                entries,
            }
        }
    }
}

unsafe fn opt_string(p: *const std::os::raw::c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

pub(crate) fn cstring(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::InvalidArgument(format!("字符串含有 NUL: {s:?}")))
}

type StateCallback = Box<dyn Fn(ConnectionState) + Send + Sync>;
type ReportCallback = Box<dyn Fn(&Report) + Send + Sync>;

/// 与一台 IED 的 MMS 连接(libiec61850 的 `IedConnection`)。
///
/// 报告与状态回调在 libiec61850 自己的接收线程里执行, 不要在回调中做阻塞操作。
pub struct Connection {
    pub(crate) inner: ffi::IedConnection,
    // 回调闭包必须活到 `IedConnection_destroy` 之后(destroy 会等接收线程退出,
    // 期间还可能触发 Closed 回调), 所以只在 Drop 里统一释放, 不随取消订阅释放。
    // 元素必须装箱: C 侧持有指向闭包的裸指针, Vec 扩容时不能移动闭包本身。
    #[allow(clippy::vec_box)]
    state_handlers: Mutex<Vec<Box<StateCallback>>>,
    #[allow(clippy::vec_box)]
    report_handlers: Mutex<Vec<Box<ReportCallback>>>,
    // LD 列表在连接期间不变, 缓存起来给 `resolve_ld` 用。
    ld_cache: Mutex<Option<Vec<String>>>,
}

// SAFETY: libiec61850 的 IedConnection 内部持有接收线程和互斥锁, 同步服务调用可以
// 在任意线程发起; 回调闭包要求 `Send + Sync`。
unsafe impl Send for Connection {}
unsafe impl Sync for Connection {}

impl Connection {
    pub fn new(options: ConnectOptions) -> Result<Connection, Error> {
        let inner = unsafe { ffi::IedConnection_create() };
        if inner.is_null() {
            return Err(Error::CreateFailed);
        }
        unsafe {
            ffi::IedConnection_setConnectTimeout(inner, options.connect_timeout_ms);
            ffi::IedConnection_setRequestTimeout(inner, options.request_timeout_ms);
        }
        // 先构造出 `Connection`, 后面任何一步出错都由 Drop 负责销毁 `inner`。
        let conn = Connection {
            inner,
            state_handlers: Mutex::new(Vec::new()),
            report_handlers: Mutex::new(Vec::new()),
            ld_cache: Mutex::new(None),
        };
        if let Some(iso) = &options.iso {
            let params = unsafe {
                ffi::MmsConnection_getIsoConnectionParameters(ffi::IedConnection_getMmsConnection(
                    inner,
                ))
            };
            if params.is_null() {
                return Err(Error::CreateFailed);
            }
            iso.apply(params)?;
        }
        Ok(conn)
    }

    /// 注册连接状态变化回调, 建议在 `connect` 之前调用。
    pub fn on_state_changed<F>(&self, handler: F)
    where
        F: Fn(ConnectionState) + Send + Sync + 'static,
    {
        let boxed: Box<StateCallback> = Box::new(Box::new(handler));
        let param = &*boxed as *const StateCallback as *mut c_void;
        self.state_handlers.lock().unwrap().push(boxed);
        unsafe {
            ffi::IedConnection_installStateChangedHandler(
                self.inner,
                Some(state_trampoline),
                param,
            );
        }
    }

    /// 阻塞建立连接。
    pub fn connect(&self, host: &str, port: u16) -> Result<(), Error> {
        let host = cstring(host)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        unsafe { ffi::IedConnection_connect(self.inner, &mut err, host.as_ptr(), i32::from(port)) };
        Error::check(err)
    }

    pub fn close(&self) {
        unsafe { ffi::IedConnection_close(self.inner) };
    }

    pub fn state(&self) -> ConnectionState {
        ConnectionState::from_raw(unsafe { ffi::IedConnection_getState(self.inner) })
    }

    pub fn is_connected(&self) -> bool {
        self.state() == ConnectionState::Connected
    }

    // ---- 浏览模型 ----

    /// 从 IED 读取完整的设备模型, 后续浏览结果才会走缓存。
    pub fn load_model(&self) -> Result<(), Error> {
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        unsafe { ffi::IedConnection_getDeviceModelFromServer(self.inner, &mut err) };
        Error::check(err)
    }

    /// 逻辑设备(LD)列表。
    pub fn logical_devices(&self) -> Result<Vec<String>, Error> {
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let list = unsafe { ffi::IedConnection_getLogicalDeviceList(self.inner, &mut err) };
        Error::check(err)?;
        Ok(unsafe { take_list(list) })
    }

    /// 某个 LD 下的逻辑节点(LN)名。
    pub fn logical_nodes(&self, ld: &str) -> Result<Vec<String>, Error> {
        let ld = cstring(ld)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let list = unsafe {
            ffi::IedConnection_getLogicalDeviceDirectory(self.inner, &mut err, ld.as_ptr())
        };
        Error::check(err)?;
        Ok(unsafe { take_list(list) })
    }

    /// 某个 LN(`LD/LN`)下的数据对象(DO)名。
    pub fn data_objects(&self, ln_ref: &str) -> Result<Vec<String>, Error> {
        let ln = cstring(ln_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let list = unsafe {
            ffi::IedConnection_getLogicalNodeDirectory(
                self.inner,
                &mut err,
                ln.as_ptr(),
                ffi::ACSIClass_ACSI_CLASS_DATA_OBJECT,
            )
        };
        Error::check(err)?;
        Ok(unsafe { take_list(list) })
    }

    /// 某个 DO/DA 下带 FC 后缀的子项, 形如 `stVal[ST]`。
    pub fn data_attributes(&self, data_ref: &str) -> Result<Vec<String>, Error> {
        let r = cstring(data_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let list =
            unsafe { ffi::IedConnection_getDataDirectoryFC(self.inner, &mut err, r.as_ptr()) };
        Error::check(err)?;
        Ok(unsafe { take_list(list) })
    }

    /// 把点表里的 LD 名对应到设备上真实的 LD 名: 先完全匹配, 否则按后缀匹配
    /// (如 `MONT` 对应 `PCS0001MONT`), 后缀匹配必须唯一。
    pub fn resolve_ld(&self, ld: &str) -> Result<String, Error> {
        let mut cache = self.ld_cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(self.logical_devices()?);
        }
        let lds = cache.as_ref().unwrap();
        if lds.iter().any(|l| l == ld) {
            return Ok(ld.to_string());
        }
        let hits: Vec<&String> = lds.iter().filter(|l| l.ends_with(ld)).collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(Error::Reference(format!(
                "设备上没有 LD {ld:?}, 现有: {lds:?}"
            ))),
            many => Err(Error::Reference(format!("LD {ld:?} 匹配到多个: {many:?}"))),
        }
    }

    /// `resolve_ld` 作用在一个引用上。
    pub fn resolve_ref(&self, r: &ObjectRef) -> Result<ObjectRef, Error> {
        Ok(r.clone().with_ld(self.resolve_ld(&r.ld)?))
    }

    /// 读取一个对象的类型结构, 引用必须带 FC。
    pub fn type_spec(&self, r: &ObjectRef) -> Result<TypeSpec, Error> {
        let fc =
            r.fc.ok_or_else(|| Error::Reference(format!("{r} 缺少 FC, 无法查询类型")))?;
        let c_ref = cstring(&r.to_dotted())?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let spec = unsafe {
            ffi::IedConnection_getVariableSpecification(
                self.inner,
                &mut err,
                c_ref.as_ptr(),
                fc.raw(),
            )
        };
        Error::check(err)?;
        if spec.is_null() {
            return Err(Error::from_code(
                ffi::IedClientError_IED_ERROR_OBJECT_DOES_NOT_EXIST,
            ));
        }
        let out = unsafe { TypeSpec::from_mms(spec) };
        unsafe { ffi::MmsVariableSpecification_destroy(spec) };
        Ok(out)
    }

    /// 数据集的成员引用(带 FC), 按数据集顺序, 与报告里的元素下标一一对应。
    pub fn data_set_members(&self, data_set_ref: &str) -> Result<Vec<ObjectRef>, Error> {
        let c_ref = cstring(data_set_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let list = unsafe {
            ffi::IedConnection_getDataSetDirectory(
                self.inner,
                &mut err,
                c_ref.as_ptr(),
                ptr::null_mut(),
            )
        };
        Error::check(err)?;
        unsafe { take_list(list) }
            .iter()
            .map(|m| m.parse())
            .collect()
    }

    /// 准备好一个数据集的布局(成员引用 + 类型结构), 供报告回调里展开 DO 级成员。
    /// 必须在订阅之前调用, 不能在报告回调里调用。
    pub fn prepare_data_set(&self, data_set_ref: &str) -> Result<DataSetLayout, Error> {
        let members = self
            .data_set_members(data_set_ref)?
            .into_iter()
            .map(|m| {
                let spec = self.type_spec(&m)?;
                Ok((m, spec))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(DataSetLayout::new(data_set_ref.to_string(), members))
    }

    // ---- 读写 ----

    /// 读一个数据对象或数据属性, 例如 `("IED1LD0/MMXU1.TotW.mag.f", Fc::Mx)`。
    pub fn read(&self, reference: &str, fc: Fc) -> Result<Value, Error> {
        let r = cstring(reference)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let v =
            unsafe { ffi::IedConnection_readObject(self.inner, &mut err, r.as_ptr(), fc.raw()) };
        Error::check(err)?;
        let value = unsafe { Value::from_mms(v) };
        if !v.is_null() {
            unsafe { ffi::MmsValue_delete(v) };
        }
        // 对象级的访问失败(如对象不存在)不走 `error`, 而是返回一个 DataAccessError 值。
        if let Value::AccessError(code) = value {
            return Err(Error::data_access(code));
        }
        Ok(value)
    }

    /// 读整个数据集, 按数据集成员顺序返回。轮询兜底使用, 实时数据优先用报告。
    pub fn read_data_set(&self, data_set_ref: &str) -> Result<Vec<Value>, Error> {
        let r = cstring(data_set_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let ds = unsafe {
            ffi::IedConnection_readDataSetValues(self.inner, &mut err, r.as_ptr(), ptr::null_mut())
        };
        Error::check(err)?;
        if ds.is_null() {
            return Ok(Vec::new());
        }
        let out = unsafe {
            let values = ffi::ClientDataSet_getValues(ds);
            let n = if values.is_null() {
                0
            } else {
                ffi::MmsValue_getArraySize(values) as usize
            };
            let out = (0..n)
                .map(|i| Value::from_mms(ffi::MmsValue_getElement(values, i as _)))
                .collect();
            ffi::ClientDataSet_destroy(ds);
            out
        };
        Ok(out)
    }

    /// 写一个标量属性(定值等)。支持 `Bool`/`Int`/`Uint`/`Float`/`String`;
    /// 整数按 32 位, 浮点按 32 位发送。
    pub fn write(&self, reference: &str, fc: Fc, value: &Value) -> Result<(), Error> {
        let r = cstring(reference)?;
        let mms = match value {
            Value::Bool(b) => unsafe { ffi::MmsValue_newBoolean(*b) },
            Value::Int(v) => {
                let v = i32::try_from(*v)
                    .map_err(|_| Error::InvalidArgument(format!("整数超出 32 位范围: {v}")))?;
                unsafe { ffi::MmsValue_newIntegerFromInt32(v) }
            }
            Value::Uint(v) => {
                let v = u32::try_from(*v).map_err(|_| {
                    Error::InvalidArgument(format!("无符号整数超出 32 位范围: {v}"))
                })?;
                unsafe { ffi::MmsValue_newUnsignedFromUint32(v) }
            }
            Value::Float(v) => unsafe { ffi::MmsValue_newFloat(*v as f32) },
            Value::String(s) => {
                let s = cstring(s)?;
                unsafe { ffi::MmsValue_newVisibleString(s.as_ptr()) }
            }
            other => return Err(Error::UnsupportedValue(format!("{other:?}"))),
        };
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        unsafe {
            ffi::IedConnection_writeObject(self.inner, &mut err, r.as_ptr(), fc.raw(), mms);
            ffi::MmsValue_delete(mms);
        }
        Error::check(err)
    }

    // ---- 报告 ----

    /// 订阅一个报告控制块(URCB 或 BRCB), `rcb_ref` 形如 `IED1LD0/LLN0.RP.rcb01`
    /// (BRCB 为 `.BR.`)。流程: 读 RCB → 装回调 → 配置 → 使能 → 可选 GI。
    ///
    /// `handler` 在库的接收线程里被调用。
    pub fn subscribe_report<F>(
        &self,
        rcb_ref: &str,
        config: &ReportConfig,
        handler: F,
    ) -> Result<(), Error>
    where
        F: Fn(&Report) + Send + Sync + 'static,
    {
        let c_ref = cstring(rcb_ref)?;
        let c_ds = config.data_set.as_deref().map(cstring).transpose()?;

        let rcb = self.get_rcb(&c_ref)?;
        let buffered = unsafe { ffi::ClientReportControlBlock_isBuffered(rcb.0) };

        // URCB 被其他客户端预留、或任一类 RCB 已被使能, 都说明正被别的客户端使用;
        // 此时去改配置只会失败, 还可能干扰对方, 所以直接报占用。
        let busy = unsafe {
            ffi::ClientReportControlBlock_getRptEna(rcb.0)
                || (!buffered && ffi::ClientReportControlBlock_getResv(rcb.0))
        };
        if busy {
            return Err(Error::RcbBusy(rcb_ref.to_string()));
        }

        let boxed: Box<ReportCallback> = Box::new(Box::new(handler));
        let param = &*boxed as *const ReportCallback as *mut c_void;
        self.report_handlers.lock().unwrap().push(boxed);

        unsafe {
            ffi::IedConnection_installReportHandler(
                self.inner,
                c_ref.as_ptr(),
                ffi::ClientReportControlBlock_getRptId(rcb.0),
                Some(report_trampoline),
                param,
            );
        }

        let result = self.configure_and_enable(&rcb, buffered, config, c_ds.as_deref());
        if result.is_err() {
            unsafe { ffi::IedConnection_uninstallReportHandler(self.inner, c_ref.as_ptr()) };
        }
        result
    }

    /// 读 RCB 当前绑定的数据集引用。
    pub fn rcb_data_set(&self, rcb_ref: &str) -> Result<String, Error> {
        let c_ref = cstring(rcb_ref)?;
        let rcb = self.get_rcb(&c_ref)?;
        unsafe { opt_string(ffi::ClientReportControlBlock_getDataSetReference(rcb.0)) }
            .ok_or_else(|| Error::Reference(format!("{rcb_ref} 没有绑定数据集")))
    }

    /// 展开 RCB 引用。`LD/LN.RP.name`(URCB)或 `LD/LN.BR.name`(BRCB), `name` 末尾可以带 `*`:
    /// IED 里可复用的 RCB 通常有多个实例(`urcbAin01`~`urcbAin16`), 每个客户端占一个,
    /// 通配会列出设备上所有以该前缀开头的实例。没有 `*` 时原样返回(LD 已按后缀匹配展开)。
    pub fn resolve_rcbs(&self, pattern: &str) -> Result<Vec<String>, Error> {
        let norm = pattern.trim().replace('$', ".");
        let (head, kind, name) = [".RP.", ".BR."]
            .iter()
            .find_map(|k| {
                norm.split_once(k)
                    .map(|(h, n)| (h.to_string(), &k[1..3], n.to_string()))
            })
            .ok_or_else(|| {
                Error::Reference(format!(
                    "{pattern:?} 不是 RCB 引用, 期望 LD/LN.RP.name 或 LD/LN.BR.name"
                ))
            })?;
        let (ld, ln) = head
            .split_once('/')
            .ok_or_else(|| Error::Reference(format!("{pattern:?} 缺少 '/'")))?;
        let head = format!("{}/{}", self.resolve_ld(ld)?, ln);
        let Some(prefix) = name.strip_suffix('*') else {
            return Ok(vec![format!("{head}.{kind}.{name}")]);
        };

        let c_ln = cstring(&head)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let class = if kind == "BR" {
            ffi::ACSIClass_ACSI_CLASS_BRCB
        } else {
            ffi::ACSIClass_ACSI_CLASS_URCB
        };
        let list = unsafe {
            ffi::IedConnection_getLogicalNodeDirectory(self.inner, &mut err, c_ln.as_ptr(), class)
        };
        Error::check(err)?;
        let mut names: Vec<String> = unsafe { take_list(list) }
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect();
        names.sort();
        if names.is_empty() {
            return Err(Error::Reference(format!(
                "设备上没有匹配 {pattern:?} 的 RCB"
            )));
        }
        Ok(names
            .into_iter()
            .map(|n| format!("{head}.{kind}.{n}"))
            .collect())
    }

    /// 依次尝试候选 RCB, 订阅第一个没被其他客户端占用的, 返回选中的 RCB 引用。
    /// 候选通常来自 `resolve_rcbs`。全部被占用时返回 `Error::RcbBusy`。
    pub fn subscribe_report_any(
        &self,
        candidates: &[String],
        config: &ReportConfig,
        handler: Arc<dyn Fn(&Report) + Send + Sync>,
    ) -> Result<String, Error> {
        let mut last = None;
        for rcb in candidates {
            let h = handler.clone();
            match self.subscribe_report(rcb, config, move |r| h(r)) {
                Ok(()) => return Ok(rcb.clone()),
                Err(e @ Error::RcbBusy(_)) => last = Some(e),
                Err(Error::Client { code, .. })
                    if code == ffi::IedClientError_IED_ERROR_ACCESS_DENIED
                        || code == ffi::IedClientError_IED_ERROR_TEMPORARILY_UNAVAILABLE =>
                {
                    last = Some(Error::RcbBusy(rcb.clone()));
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| Error::RcbBusy(candidates.join(","))))
    }

    fn configure_and_enable(
        &self,
        rcb: &Rcb,
        buffered: bool,
        config: &ReportConfig,
        data_set: Option<&CStr>,
    ) -> Result<(), Error> {
        unsafe {
            // 之前的会话可能把 RCB 留在使能状态, 此时无法改配置; 尽力先关掉, 忽略失败。
            ffi::ClientReportControlBlock_setRptEna(rcb.0, false);
            let mut ignored = ffi::IedClientError_IED_ERROR_OK;
            ffi::IedConnection_setRCBValues(
                self.inner,
                &mut ignored,
                rcb.0,
                ffi::RCB_ELEMENT_RPT_ENA,
                true,
            );

            let mut opt_flds = ffi::RPT_OPT_SEQ_NUM
                | ffi::RPT_OPT_TIME_STAMP
                | ffi::RPT_OPT_REASON_FOR_INCLUSION
                | ffi::RPT_OPT_DATA_SET
                | ffi::RPT_OPT_DATA_REFERENCE
                | ffi::RPT_OPT_BUFFER_OVERFLOW
                | ffi::RPT_OPT_CONF_REV;
            if buffered {
                opt_flds |= ffi::RPT_OPT_ENTRY_ID;
            }
            ffi::ClientReportControlBlock_setOptFlds(rcb.0, opt_flds as _);
            ffi::ClientReportControlBlock_setTrgOps(rcb.0, config.trg_ops as _);
            let mut mask = ffi::RCB_ELEMENT_OPT_FLDS | ffi::RCB_ELEMENT_TRG_OPS;

            if !buffered {
                ffi::ClientReportControlBlock_setResv(rcb.0, true);
                mask |= ffi::RCB_ELEMENT_RESV;
            }
            if let Some(ds) = data_set {
                ffi::ClientReportControlBlock_setDataSetReference(rcb.0, ds.as_ptr());
                mask |= ffi::RCB_ELEMENT_DATSET;
            }
            if let Some(ms) = config.intg_pd_ms {
                ffi::ClientReportControlBlock_setIntgPd(rcb.0, ms);
                mask |= ffi::RCB_ELEMENT_INTG_PD;
            }
            if let Some(ms) = config.buf_tm_ms {
                ffi::ClientReportControlBlock_setBufTm(rcb.0, ms);
                mask |= ffi::RCB_ELEMENT_BUF_TM;
            }
            if buffered && config.purge_buffer {
                ffi::ClientReportControlBlock_setPurgeBuf(rcb.0, true);
                mask |= ffi::RCB_ELEMENT_PURGE_BUF;
            }

            let mut err = ffi::IedClientError_IED_ERROR_OK;
            ffi::IedConnection_setRCBValues(self.inner, &mut err, rcb.0, mask, true);
            Error::check(err)?;

            ffi::ClientReportControlBlock_setRptEna(rcb.0, true);
            ffi::IedConnection_setRCBValues(
                self.inner,
                &mut err,
                rcb.0,
                ffi::RCB_ELEMENT_RPT_ENA,
                true,
            );
            Error::check(err)?;

            if config.general_interrogation {
                ffi::ClientReportControlBlock_setGI(rcb.0, true);
                ffi::IedConnection_setRCBValues(
                    self.inner,
                    &mut err,
                    rcb.0,
                    ffi::RCB_ELEMENT_GI,
                    true,
                );
                Error::check(err)?;
            }
        }
        Ok(())
    }

    /// 对已使能的 RCB 再触发一次总召。
    pub fn trigger_gi(&self, rcb_ref: &str) -> Result<(), Error> {
        let c_ref = cstring(rcb_ref)?;
        let rcb = self.get_rcb(&c_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        unsafe {
            ffi::ClientReportControlBlock_setGI(rcb.0, true);
            ffi::IedConnection_setRCBValues(self.inner, &mut err, rcb.0, ffi::RCB_ELEMENT_GI, true);
        }
        Error::check(err)
    }

    /// 停用 RCB 并移除本地回调。回调闭包内存保留到连接销毁。
    pub fn unsubscribe_report(&self, rcb_ref: &str) -> Result<(), Error> {
        let c_ref = cstring(rcb_ref)?;
        unsafe { ffi::IedConnection_uninstallReportHandler(self.inner, c_ref.as_ptr()) };
        let rcb = self.get_rcb(&c_ref)?;
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        unsafe {
            ffi::ClientReportControlBlock_setRptEna(rcb.0, false);
            ffi::IedConnection_setRCBValues(
                self.inner,
                &mut err,
                rcb.0,
                ffi::RCB_ELEMENT_RPT_ENA,
                true,
            );
        }
        Error::check(err)
    }

    fn get_rcb(&self, c_ref: &CStr) -> Result<Rcb, Error> {
        let mut err = ffi::IedClientError_IED_ERROR_OK;
        let rcb = unsafe {
            ffi::IedConnection_getRCBValues(self.inner, &mut err, c_ref.as_ptr(), ptr::null_mut())
        };
        Error::check(err)?;
        if rcb.is_null() {
            return Err(Error::from_code(ffi::IedClientError_IED_ERROR_UNKNOWN));
        }
        Ok(Rcb(rcb))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // destroy 会关闭连接并等待接收线程退出; 回调闭包在此之后随字段释放。
        unsafe { ffi::IedConnection_destroy(self.inner) };
    }
}

struct Rcb(ffi::ClientReportControlBlock);

impl Drop for Rcb {
    fn drop(&mut self) {
        unsafe { ffi::ClientReportControlBlock_destroy(self.0) };
    }
}

/// 取出 `LinkedList<char*>` 内容并释放链表(及其中的字符串)。
unsafe fn take_list(list: ffi::LinkedList) -> Vec<String> {
    let mut out = Vec::new();
    if list.is_null() {
        return out;
    }
    unsafe {
        // 链表头是哑节点, 数据从 next 开始。
        let mut cur = ffi::iec61850_LinkedList_getNext(list);
        while !cur.is_null() {
            let data = ffi::iec61850_LinkedList_getData(cur) as *const std::os::raw::c_char;
            if let Some(s) = opt_string(data) {
                out.push(s);
            }
            cur = ffi::iec61850_LinkedList_getNext(cur);
        }
        ffi::iec61850_LinkedList_destroy(list);
    }
    out
}

unsafe extern "C" fn state_trampoline(
    parameter: *mut c_void,
    _connection: ffi::IedConnection,
    new_state: ffi::IedConnectionState,
) {
    // 不能让 panic 穿过 FFI 边界。
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let cb = unsafe { &*(parameter as *const StateCallback) };
        cb(ConnectionState::from_raw(new_state));
    }));
}

unsafe extern "C" fn report_trampoline(parameter: *mut c_void, report: ffi::ClientReport) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let cb = unsafe { &*(parameter as *const ReportCallback) };
        let report = unsafe { Report::from_raw(report) };
        cb(&report);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso_of(conn: &Connection) -> ffi::sIsoConnectionParameters {
        unsafe {
            let p = ffi::MmsConnection_getIsoConnectionParameters(
                ffi::IedConnection_getMmsConnection(conn.inner),
            );
            *p
        }
    }

    #[test]
    fn iso_params_reach_the_library() {
        let iso = IsoParams {
            remote_ap_title: Some("1 3 9999 106".into()),
            remote_ae_qualifier: Some(33),
            remote_psel: Some("00 00 00 01".into()),
            remote_ssel: Some("00 02".into()),
            remote_tsel: Some("00 03".into()),
            local_ae_qualifier: Some(7), // 没给 AP-Title, 不应生效
            ..Default::default()
        };
        let conn = Connection::new(ConnectOptions {
            iso: Some(iso),
            ..Default::default()
        })
        .unwrap();
        let p = iso_of(&conn);
        // OID 1.3.9999.106 的 BER: 2B CE 0F 6A
        assert_eq!(
            &p.remoteApTitle[..p.remoteApTitleLen as usize],
            &[0x2B, 0xCE, 0x0F, 0x6A]
        );
        assert_eq!(p.remoteAEQualifier, 33);
        assert_eq!(
            (p.remotePSelector.size, &p.remotePSelector.value[..4]),
            (4, &[0, 0, 0, 1][..])
        );
        assert_eq!(
            (p.remoteSSelector.size, &p.remoteSSelector.value[..2]),
            (2, &[0, 2][..])
        );
        assert_eq!(
            (p.remoteTSelector.size, &p.remoteTSelector.value[..2]),
            (2, &[0, 3][..])
        );
        // 本机侧保持库默认
        assert_eq!(p.localAEQualifier, 12);
        assert_eq!(
            (p.localTSelector.size, &p.localTSelector.value[..2]),
            (2, &[0, 1][..])
        );
    }

    #[test]
    fn defaults_untouched_without_iso() {
        let conn = Connection::new(ConnectOptions::default()).unwrap();
        let p = iso_of(&conn);
        assert_eq!(p.remoteAEQualifier, 12);
        assert_eq!(p.remotePSelector.size, 4);
    }
}
