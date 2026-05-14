use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use gpio_cdev::{AsyncLineEventHandle, Chip, EventRequestFlags, Line, LineRequestFlags};
use tokio::sync::{Mutex, watch};
use tokio::task::{self, JoinHandle};
use tokio::time;

use crate::{
    center::{DataCenterError, SharedPointCenter},
    config::{
        self, Device,
        gpio_conf::{Direction, GpioConfig, GpioConfigs},
    },
    core::point::{DataPoint, Val},
    dev::{DeviceError, Executable, Identifiable, Lifecycle, LifecycleState, state::SharedState},
};

pub struct GpioDev {
    id: String,
    center: SharedPointCenter,
    state: SharedState,
    configs: GpioConfigs,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    di_task: Mutex<Option<JoinHandle<()>>>,
    do_task: Mutex<Option<JoinHandle<()>>>,
}

impl GpioDev {
    pub fn new(dev: Device, center: SharedPointCenter) -> Result<Self, DeviceError> {
        let Some(id) = dev.id else {
            return Err(DeviceError::InvalidId);
        };
        let Some(configs) = dev.protocol_configs else {
            return Err(DeviceError::NotFoundConfigs(id));
        };
        let configs = match configs {
            config::ProtocolConfigs::Modbus(_) => {
                return Err(DeviceError::UnSupportedComType);
            }
            config::ProtocolConfigs::CAN(_) => {
                return Err(DeviceError::UnSupportedComType);
            }
            config::ProtocolConfigs::GPIO(gpio_configs) => gpio_configs,
            config::ProtocolConfigs::None => {
                return Err(DeviceError::NotFoundConfigs(id));
            }
        }
        .into_iter()
        .filter(|cfg| cfg.enable)
        .collect::<Vec<_>>();
        let state = SharedState::new(LifecycleState::New);
        let (stop_tx, stop_rx) = watch::channel(false);
        tracing::info!("加载{}配置成功!", id);
        Ok(Self {
            id,
            center,
            state,
            configs,
            stop_tx,
            stop_rx,
            di_task: Mutex::new(None),
            do_task: Mutex::new(None),
        })
    }

    /// 获取设备的生命周期状态
    /// # 返回值
    /// - `LifecycleState`: 设备的生命周期状态
    fn load_state(&self) -> LifecycleState {
        self.state.load()
    }

    /// 改变设备的生命周期状态
    /// # 参数
    /// - `from`: 当前状态
    /// - `to`: 目标状态
    /// # 返回值
    /// - `bool`: 是否成功改变状态
    fn cas_state(&self, from: LifecycleState, to: LifecycleState) -> bool {
        self.state.cas(from, to)
    }

    /// 存储设备的生命周期状态
    /// # 参数
    /// - `to`: 目标状态
    fn store_state(&self, to: LifecycleState) {
        self.state.store(&self.id, to);
    }
}

impl Identifiable for GpioDev {
    fn id(&self) -> &str {
        &self.id
    }
}

#[async_trait::async_trait]
impl Lifecycle for GpioDev {
    fn init(&self) -> Result<(), DeviceError> {
        //将状态从New转为Initializing
        if !self.cas_state(LifecycleState::New, LifecycleState::Initializing) {
            return Ok(());
        }
        self.store_state(LifecycleState::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        //尝试将状态改为启动中，如果失败则不动作
        let ok = self.cas_state(LifecycleState::Ready, LifecycleState::Starting)
            || self.cas_state(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<DataPoint>>(8);
        //将设备注册到消息中心
        match self.center.attach_downlink(&self.id, tx.clone()) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {
                self.center.detach_downlink(&self.id);
                if let Err(err) = self.center.attach_downlink(&self.id, tx) {
                    tracing::warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                    return Ok(());
                }
            }
            Err(err) => {
                tracing::warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                return Ok(());
            }
        }
        let gpio_conf_devs = create_gpio_devs(self.configs.clone())
            .map_err(|e| DeviceError::DevRuntimeError(Box::new(e)))?;

        // 重置停止信号
        let _ = self.stop_tx.send(false);

        // 清理旧任务
        let mut di_task_guard = self.di_task.lock().await;
        if let Some(handle) = di_task_guard.take() {
            handle.abort();
        }
        let mut do_task_guard = self.do_task.lock().await;
        if let Some(handle) = do_task_guard.take() {
            handle.abort();
        }

        // 启动 DI 监听任务
        let di_handle = {
            let center_clone = self.center.clone();
            let id = self.id.clone();
            let devs = gpio_conf_devs.clone();
            let stop_rx = self.stop_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_di(id.clone(), devs, center_clone, stop_rx).await {
                    tracing::error!("[{}] DI处理错误: {}", id, e);
                }
            })
        };
        *di_task_guard = Some(di_handle);
        drop(di_task_guard);

        // 启动 DO 控制任务
        let do_handle = {
            let id = self.id.clone();
            let stop_rx = self.stop_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_do(id.clone(), gpio_conf_devs, rx, stop_rx).await {
                    tracing::error!("[{}] DO处理错误: {}", id, e);
                }
            })
        };
        *do_task_guard = Some(do_handle);
        drop(do_task_guard);

        self.store_state(LifecycleState::Running);
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        // 发送停止信号
        let _ = self.stop_tx.send(true);

        let cur = self.load_state();
        match cur {
            LifecycleState::Stopped => return Ok(()),
            LifecycleState::New | LifecycleState::Ready => {
                self.store_state(LifecycleState::Stopped);
                self.center.detach_downlink(&self.id);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }

        // 从数据中心注销
        self.center.detach_downlink(&self.id);

        // 停止 DI 任务
        let mut di_task_guard = self.di_task.lock().await;
        if let Some(mut handle) = di_task_guard.take() {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    tracing::warn!("[{}] DI任务停止超时，强制中止", self.id);
                    handle.abort();
                }
                _ = &mut handle => {
                    tracing::info!("[{}] DI任务已停止", self.id);
                }
            }
        }

        // 停止 DO 任务
        let mut do_task_guard = self.do_task.lock().await;
        if let Some(mut handle) = do_task_guard.take() {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    tracing::warn!("[{}] DO任务停止超时，强制中止", self.id);
                    handle.abort();
                }
                _ = &mut handle => {
                    tracing::info!("[{}] DO任务已停止", self.id);
                }
            }
        }

        self.store_state(LifecycleState::Stopped);
        tracing::info!("[{}] GPIO设备已停止", self.id);
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.load_state()
    }
}

impl Executable for GpioDev {}

#[derive(Debug, Clone)]
struct GpioConfDev {
    config: GpioConfig,
    line: Line,
}

fn create_gpio_devs(configs: GpioConfigs) -> Result<Vec<GpioConfDev>, gpio_cdev::Error> {
    let mut map = HashMap::with_capacity(2);
    let mut gpio_devs = Vec::new();
    for conf in configs {
        map.entry(conf.chip).or_insert(Vec::new()).push(conf);
    }
    for (c, confs) in map {
        let mut chip = Chip::new(c)?;
        for it in confs {
            let line = chip.get_line(it.line as u32)?;
            gpio_devs.push(GpioConfDev { config: it, line });
        }
    }
    Ok(gpio_devs)
}

/// 处理数字输入 (DI) - 监听 GPIO 事件并上报
async fn handle_di(
    id: String,
    vec: Vec<GpioConfDev>,
    center: SharedPointCenter,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), gpio_cdev::Error> {
    let handles = vec
        .iter()
        .filter(|it| it.config.direction == Direction::DI)
        .map(
            |it| -> Result<(GpioConfig, AsyncLineEventHandle), gpio_cdev::Error> {
                let events = it.line.events(
                    LineRequestFlags::INPUT,
                    EventRequestFlags::BOTH_EDGES,
                    it.config.key,
                )?;
                let handle = AsyncLineEventHandle::new(events)?;
                Ok((it.config, handle))
            },
        )
        .collect::<Result<Vec<(GpioConfig, AsyncLineEventHandle)>, gpio_cdev::Error>>()?;

    if handles.is_empty() {
        tracing::info!("[{}] 没有DI类型的GPIO配置，DI监听任务退出", id);
        return Ok(());
    }

    tracing::info!("[{}] DI监听任务启动，监听{}个输入", id, handles.len());

    for (config, mut handle) in handles {
        let center = center.clone();
        let id = id.clone();
        let mut stop_rx_clone = stop_rx.clone();
        task::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx_clone.changed() => {
                        if *stop_rx_clone.borrow() {
                            tracing::info!("[{}] DI GPIO[{}]监听任务收到停止信号", id, config.key);
                            break;
                        }
                    }
                    event = handle.next() => {
                        match event {
                            Some(Ok(event)) => match event.event_type() {
                                gpio_cdev::EventType::RisingEdge => {
                                    center.ingest(id.as_str(), vec![config.to_data_point(1)]);
                                }
                                gpio_cdev::EventType::FallingEdge => {
                                    center.ingest(id.as_str(), vec![config.to_data_point(0)]);
                                }
                            },
                            Some(Err(err)) => {
                                tracing::error!("[{}] GPIO[{}]事件错误: {}", id, config.key, err);
                                break;
                            }
                            None => {
                                tracing::warn!("[{}] GPIO[{}]事件流结束", id, config.key);
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    // 等待停止信号
    let _ = stop_rx.changed().await;
    tracing::info!("[{}] DI监听任务退出", id);
    Ok(())
}

/// 处理数字输出 (DO) - 接收控制命令并设置 GPIO 输出
async fn handle_do(
    id: String,
    vec: Vec<GpioConfDev>,
    mut rx: tokio::sync::mpsc::Receiver<Vec<DataPoint>>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), gpio_cdev::Error> {
    use std::collections::HashMap;

    // 初始化所有 DO 类型的 GPIO 输出句柄
    let mut output_handles: HashMap<&'static str, gpio_cdev::LineHandle> = HashMap::new();

    for dev in vec.iter().filter(|it| it.config.direction == Direction::DO) {
        // 请求输出模式，初始值为 0
        match dev
            .line
            .request(LineRequestFlags::OUTPUT, 0, dev.config.key)
        {
            Ok(handle) => {
                tracing::info!(
                    "[{}] 初始化DO GPIO: {} (chip: {}, line: {})",
                    id,
                    dev.config.key,
                    dev.config.chip,
                    dev.config.line
                );
                output_handles.insert(dev.config.key, handle);
            }
            Err(e) => {
                tracing::error!("[{}] 初始化DO GPIO失败: {} - {}", id, dev.config.key, e);
            }
        }
    }

    if output_handles.is_empty() {
        tracing::info!("[{}] 没有DO类型的GPIO配置，DO控制任务退出", id);
        return Ok(());
    }

    tracing::info!(
        "[{}] DO控制任务启动，监听{}个输出",
        id,
        output_handles.len()
    );

    // 接收下行控制命令，同时监听停止信号
    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    tracing::info!("[{}] DO控制任务收到停止信号", id);
                    break;
                }
            }
            points = rx.recv() => {
                match points {
                    Some(points) => {
                        for point in points {
                            if let Some(handle) = output_handles.get_mut(point.key) {
                                // 将数据点的值转换为 u8
                                let value = match point.value {
                                    Val::U8(v) => v,
                                    Val::I8(v) => {
                                        if v != 0 {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::I16(v) => {
                                        if v != 0 {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::I32(v) => {
                                        if v != 0 {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::U16(v) => {
                                        if v != 0 {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::U32(v) => {
                                        if v != 0 {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::F64(v) => {
                                        if v.abs() > f64::EPSILON {
                                            1
                                        } else {
                                            0
                                        }
                                    }
                                    Val::List(_) => {
                                        tracing::warn!("[{}] GPIO[{}] 不支持List类型", id, point.key);
                                        continue;
                                    }
                                };

                                // 设置 GPIO 输出
                                if let Err(e) = handle.set_value(value) {
                                    tracing::error!("[{}] 设置GPIO[{}]输出失败: {}", id, point.key, e);
                                } else {
                                    tracing::debug!("[{}] 设置GPIO[{}]输出: {}", id, point.key, value);
                                }
                            } else {
                                tracing::warn!("[{}] 未找到GPIO配置: {}", id, point.key);
                            }
                        }
                    }
                    None => {
                        tracing::info!("[{}] DO控制通道已关闭", id);
                        break;
                    }
                }
            }
        }
    }

    tracing::info!("[{}] DO控制任务退出", id);
    Ok(())
}
