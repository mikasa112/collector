//! 测试用的模拟 IED(`server` feature)。
//!
//! 用真实 PCS(瑞山 RS100)的 ICD 生成的静态模型起一个本地 MMS 服务端: 同样的 LD/LN、数据集、
//! 报告控制块和控制对象。模型由 `build.rs` 调 `genmodel.jar` 从 `tests/data/INPOWER_MMS_1.5.icd`
//! 现场生成(需要本机有 java)。
//!
//! 它验证的是"协议实现与该 ICD 的模型自洽", 不能代替真机: 真机的 RCB 实际命名、
//! 是否校验 AP-Title 等仍需现场确认。

use std::{
    ffi::{CString, c_void},
    net::TcpListener,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicI64, Ordering},
    },
};

use crate::ffi;

/// 模拟 PCS 的真实 LD 名: IED 名 + ldInst。
pub const PCS_LD: &str = "PCS0001MONT";

/// 默认挂了控制处理函数的控制对象, 与 `PcsSimulator::calls` 的下标对应。
pub const CTL_SPC: usize = 0; // ctlGAPC1.SPCSO6, 布尔, 回读 stVal
pub const CTL_APC: usize = 1; // setGAPC1.StrVal6, 浮点, 回读 mxVal.f

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// 控制对象的服务端行为: 收到命令后把值写回对应的回读属性, 和真实设备一样。
struct CtlTarget {
    server: ffi::IedServer,
    feedback: *mut ffi::DataAttribute,
    is_bool: bool,
    calls: AtomicI64,
}

unsafe extern "C" fn ctl_handler(
    _action: ffi::ControlAction,
    parameter: *mut c_void,
    ctl_val: *mut ffi::MmsValue,
    _test: bool,
) -> ffi::ControlHandlerResult {
    unsafe {
        let t = &*(parameter as *const CtlTarget);
        if t.is_bool {
            ffi::IedServer_updateBooleanAttributeValue(
                t.server,
                t.feedback,
                ffi::MmsValue_getBoolean(ctl_val),
            );
        } else {
            // APC 的 ctlVal 是 AnalogueValue 结构体 {f}
            let v = if ffi::MmsValue_getType(ctl_val) == ffi::MmsType_MMS_STRUCTURE {
                ffi::MmsValue_getElement(ctl_val, 0)
            } else {
                ctl_val
            };
            ffi::IedServer_updateFloatAttributeValue(
                t.server,
                t.feedback,
                ffi::MmsValue_toFloat(v),
            );
        }
        t.calls.fetch_add(1, Ordering::SeqCst);
    }
    ffi::ControlHandlerResult_CONTROL_RESULT_OK
}

/// `genmodel` 生成的是进程内唯一的全局静态模型, 同一时刻只能有一个服务端在上面运行,
/// 所以同一个进程里的测试必须串行: `start` 会等上一个模拟器销毁。
static SERIAL: Mutex<()> = Mutex::new(());

/// 本机 `127.0.0.1` 上的模拟 PCS。注意静态模型的值会跨模拟器保留, 测试要显式复位自己用到的值。
pub struct PcsSimulator {
    server: ffi::IedServer,
    pub port: u16,
    // 元素必须装箱: 服务端持有指向 CtlTarget 的裸指针, Vec 扩容时不能移动它们。
    #[allow(clippy::vec_box)]
    targets: Vec<Box<CtlTarget>>,
    // 放在最后: 服务端销毁(Drop::drop)之后才释放锁。
    _guard: MutexGuard<'static, ()>,
}

unsafe impl Send for PcsSimulator {}

impl PcsSimulator {
    /// 在随机空闲端口上启动。
    pub fn start() -> PcsSimulator {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        Self::start_on(port)
    }

    /// 在指定端口启动(用于模拟设备重启后原地恢复)。
    pub fn start_on(port: u16) -> PcsSimulator {
        let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let server = ffi::IedServer_create(&raw mut ffi::pcs);
            let mut sim = PcsSimulator {
                server,
                port,
                targets: Vec::new(),
                _guard: guard,
            };
            sim.add_target("ctlGAPC1.SPCSO6", "stVal", true);
            sim.add_target("setGAPC1.StrVal6", "mxVal.f", false);
            ffi::IedServer_start(server, port as i32);
            assert!(ffi::IedServer_isRunning(server), "模拟 PCS 启动失败");
            sim
        }
    }

    fn node(&self, path: &str) -> *mut ffi::ModelNode {
        let n = unsafe {
            ffi::IedModel_getModelNodeByObjectReference(
                &raw mut ffi::pcs,
                c(&format!("{PCS_LD}/{path}")).as_ptr(),
            )
        };
        assert!(!n.is_null(), "模型里没有 {path}");
        n
    }

    fn add_target(&mut self, ctl: &str, feedback: &str, is_bool: bool) {
        let fb = self.node(&format!("{ctl}.{feedback}")) as *mut ffi::DataAttribute;
        let mut t = Box::new(CtlTarget {
            server: self.server,
            feedback: fb,
            is_bool,
            calls: AtomicI64::new(0),
        });
        let param = &mut *t as *mut CtlTarget as *mut c_void;
        unsafe {
            ffi::IedServer_setControlHandler(
                self.server,
                self.node(ctl) as *mut ffi::DataObject,
                Some(ctl_handler),
                param,
            )
        };
        self.targets.push(t);
    }

    /// 第 `idx`(`CTL_SPC`/`CTL_APC`)个控制对象被成功执行的次数。
    pub fn calls(&self, idx: usize) -> i64 {
        self.targets[idx].calls.load(Ordering::SeqCst)
    }

    /// 设置 `path`(相对 LD, 如 `MMXU1.TotW.mag.f`)处的浮点属性。
    pub fn set_float(&self, path: &str, v: f32) {
        let da = self.node(path) as *mut ffi::DataAttribute;
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateFloatAttributeValue(self.server, da, v);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }

    /// 设置 `path`(品质属性 `q`)的品质, `quality` 用 libiec61850 的整数编码:
    /// 有效性 0 良好 / 1 保留 / 2 无效 / 3 可疑, 细节位从 bit2 起(4 溢出 ... 128 旧数据 ...)。
    pub fn set_quality(&self, path: &str, quality: u16) {
        let da = self.node(path) as *mut ffi::DataAttribute;
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateQuality(self.server, da, quality);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }

    /// 设置 `path`(时标属性 `t`)的 UTC 时间, 毫秒。
    pub fn set_time(&self, path: &str, ms: u64) {
        let da = self.node(path) as *mut ffi::DataAttribute;
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateUTCTimeAttributeValue(self.server, da, ms);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }

    /// 设置 `path` 处的布尔属性。
    pub fn set_bool(&self, path: &str, v: bool) {
        let da = self.node(path) as *mut ffi::DataAttribute;
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateBooleanAttributeValue(self.server, da, v);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }
}

impl Drop for PcsSimulator {
    fn drop(&mut self) {
        unsafe {
            ffi::IedServer_stop(self.server);
            ffi::IedServer_destroy(self.server);
        }
    }
}
