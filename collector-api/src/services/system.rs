use serde::Serialize;
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::services::{ServiceError, ServiceResult};

/// 与部署机上 systemd unit 文件的 `ExecStart`/`WorkingDirectory` 组合后指向的文件保持一致
pub(crate) const CONFIG_PATH: &str = "config/config.json";
pub(crate) const BACKUP_DIR: &str = "config/backups";
/// 需与部署机上实际的 systemd unit 名一致
pub(crate) const SYSTEMD_UNIT: &str = "collector.service";

const SYSTEMD_SERVICE: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const SYSTEMD_UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

pub struct SystemService {}

#[derive(Debug, Serialize)]
pub struct BackupInfo {
    pub name: String,
    pub created_at: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct UnitStatus {
    pub active_state: String,
}

impl SystemService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    /// 读取配置文件原文
    pub async fn read_config(&self) -> ServiceResult<String> {
        Ok(tokio::fs::read_to_string(CONFIG_PATH).await?)
    }

    /// 校验 + 备份旧配置 + 原子写新配置
    pub async fn update_config(&self, raw: String) -> ServiceResult<()> {
        let project: collector_core::config::Project = serde_json::from_str(&raw)?;

        for (dev_id, dev) in &project.devices {
            if let Some(file) = &dev.config.register_file
                && !tokio::fs::try_exists(file).await.unwrap_or(false)
            {
                return Err(ServiceError::invalid_parameter(format!(
                    "设备 {dev_id} 的 register_file 不存在: {file}"
                )));
            }
        }

        tokio::fs::create_dir_all(BACKUP_DIR).await?;
        let old = tokio::fs::read(CONFIG_PATH).await?;
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = format!("{BACKUP_DIR}/{ts}.json");
        tokio::fs::write(&backup_path, &old).await?;

        let tmp = format!("{CONFIG_PATH}.tmp");
        tokio::fs::write(&tmp, raw.as_bytes()).await?;
        tokio::fs::rename(&tmp, CONFIG_PATH).await?;

        Ok(())
    }

    /// 按文件名（时间戳）倒序列出所有备份
    pub async fn list_backups(&self) -> ServiceResult<Vec<BackupInfo>> {
        let mut backups = Vec::new();

        let mut entries = match tokio::fs::read_dir(BACKUP_DIR).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(backups),
            Err(e) => return Err(e.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !name.ends_with(".json") {
                continue;
            }
            let metadata = entry.metadata().await?;
            let created_at = metadata
                .modified()
                .ok()
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default();
            backups.push(BackupInfo {
                name,
                created_at,
                size: metadata.len(),
            });
        }

        backups.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(backups)
    }

    /// 将指定备份恢复为当前配置（会先对当前配置再打一份备份）
    pub async fn restore_backup(&self, name: &str) -> ServiceResult<()> {
        if name.contains('/') || name.contains("..") || !name.ends_with(".json") {
            return Err(ServiceError::invalid_parameter("非法的备份文件名"));
        }
        let path = format!("{BACKUP_DIR}/{name}");
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| ServiceError::not_found("备份不存在"))?;
        self.update_config(content).await
    }

    /// 触发 systemd 重启当前服务；实际重启动作延迟执行，让本次 HTTP 响应先返回给客户端
    pub async fn restart(&self) -> ServiceResult<()> {
        let conn = Connection::system()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        let manager = zbus::Proxy::new(&conn, SYSTEMD_SERVICE, SYSTEMD_PATH, SYSTEMD_MANAGER_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let _: Result<OwnedObjectPath, _> =
                manager.call("RestartUnit", &(SYSTEMD_UNIT, "replace")).await;
        });

        Ok(())
    }

    /// 查询 systemd 服务当前的 ActiveState
    pub async fn status(&self) -> ServiceResult<UnitStatus> {
        let conn = Connection::system()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        let manager = zbus::Proxy::new(&conn, SYSTEMD_SERVICE, SYSTEMD_PATH, SYSTEMD_MANAGER_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let unit_path: OwnedObjectPath = manager
            .call("LoadUnit", &(SYSTEMD_UNIT,))
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let unit = zbus::Proxy::new(&conn, SYSTEMD_SERVICE, unit_path.as_str(), SYSTEMD_UNIT_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let active_state: String = unit
            .get_property("ActiveState")
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        Ok(UnitStatus { active_state })
    }
}
