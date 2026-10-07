use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use collector_core::utils::database::get_database;
use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;

use crate::{
    dao::project_info::ProjectInfoDao,
    handlers::project_info::UpdateProjectInfoParams,
    models::project_info::{
        KEY_NAME, KEY_RATED_ENERGY_KWH, KEY_RATED_POWER_KW, KEY_TITLE, KEY_VERSION, ProjectInfoView,
    },
    services::{ServiceError, ServiceResult},
};

/// 图标存放目录，与 data.db 一样相对进程工作目录
const LOGO_DIR: &str = "./logo";
const LOGO_STEM: &str = "logo";
pub const MAX_LOGO_BYTES: u64 = 512 * 1024;

/// 支持的图标格式：(扩展名, Content-Type, 文件头魔数)。不接受 SVG，避免内嵌脚本
const LOGO_FORMATS: &[(&str, &str, &[u8])] = &[
    ("png", "image/png", &[0x89, b'P', b'N', b'G']),
    ("jpg", "image/jpeg", &[0xFF, 0xD8, 0xFF]),
    ("ico", "image/x-icon", &[0x00, 0x00, 0x01, 0x00]),
];

pub struct LogoFile {
    pub path: PathBuf,
    pub content_type: &'static str,
}

pub struct ProjectInfoService {
    pool: SqlitePool,
}

fn logo_path(ext: &str) -> PathBuf {
    Path::new(LOGO_DIR).join(format!("{LOGO_STEM}.{ext}"))
}

/// 空串视为清除该字段
fn text_change(value: Option<String>) -> Option<Option<String>> {
    value.map(|v| {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    })
}

impl ProjectInfoService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    pub async fn get(&self) -> ServiceResult<ProjectInfoView> {
        let mut map: HashMap<String, String> = ProjectInfoDao::find_all(&self.pool)
            .await?
            .into_iter()
            .collect();
        let logo_url = match Self::find_logo().await {
            Some(logo) => {
                let version = tokio::fs::metadata(&logo.path)
                    .await
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Some(format!("/v1/project/logo?v={version}"))
            }
            None => None,
        };
        Ok(ProjectInfoView {
            name: map.remove(KEY_NAME),
            title: map.remove(KEY_TITLE),
            version: map.remove(KEY_VERSION),
            rated_power_kw: map.remove(KEY_RATED_POWER_KW).and_then(|v| v.parse().ok()),
            rated_energy_kwh: map
                .remove(KEY_RATED_ENERGY_KWH)
                .and_then(|v| v.parse().ok()),
            software_version: env!("CARGO_PKG_VERSION"),
            logo_url,
        })
    }

    /// 部分更新：未传的字段保持不变，文本字段传空串表示清除
    pub async fn update(
        &self,
        params: UpdateProjectInfoParams,
        updated_by: Option<String>,
    ) -> ServiceResult<()> {
        let mut changes: Vec<(&str, Option<String>)> = Vec::new();
        for (key, value) in [
            (KEY_NAME, params.name),
            (KEY_TITLE, params.title),
            (KEY_VERSION, params.version),
        ] {
            if let Some(change) = text_change(value) {
                changes.push((key, change));
            }
        }
        if let Some(v) = params.rated_power_kw {
            changes.push((KEY_RATED_POWER_KW, Some(v.to_string())));
        }
        if let Some(v) = params.rated_energy_kwh {
            changes.push((KEY_RATED_ENERGY_KWH, Some(v.to_string())));
        }
        if changes.is_empty() {
            return Err(ServiceError::InvalidParameter(
                "没有需要更新的字段".to_string(),
            ));
        }
        ProjectInfoDao::apply(&self.pool, &changes, updated_by.as_deref()).await?;
        Ok(())
    }

    /// 查找当前图标文件，不存在返回 None
    pub async fn find_logo() -> Option<LogoFile> {
        for (ext, content_type, _) in LOGO_FORMATS {
            let path = logo_path(ext);
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Some(LogoFile { path, content_type });
            }
        }
        None
    }

    /// 校验并保存上传的图标：按文件头判断真实格式，覆盖旧图标，其他扩展名的旧文件一并清理
    pub async fn save_logo(&self, uploaded: &Path, size: u64) -> ServiceResult<()> {
        if size == 0 {
            return Err(ServiceError::InvalidParameter("图标文件为空".to_string()));
        }
        if size > MAX_LOGO_BYTES {
            return Err(ServiceError::InvalidParameter(format!(
                "图标文件不能超过{}KB",
                MAX_LOGO_BYTES / 1024
            )));
        }
        let mut head = [0u8; 8];
        let n = tokio::fs::File::open(uploaded)
            .await?
            .read(&mut head)
            .await?;
        let ext = LOGO_FORMATS
            .iter()
            .find(|(_, _, magic)| head[..n].starts_with(magic))
            .map(|(ext, _, _)| *ext)
            .ok_or_else(|| {
                ServiceError::InvalidParameter("仅支持 png/jpg/ico 格式的图标".to_string())
            })?;

        tokio::fs::create_dir_all(LOGO_DIR).await?;
        let target = logo_path(ext);
        // 先写临时文件再改名，避免读取方拿到写了一半的图标
        let tmp = target.with_extension(format!("{ext}.tmp"));
        tokio::fs::copy(uploaded, &tmp).await?;
        tokio::fs::rename(&tmp, &target).await?;
        for (other, _, _) in LOGO_FORMATS.iter().filter(|(e, _, _)| *e != ext) {
            let _ = tokio::fs::remove_file(logo_path(other)).await;
        }
        Ok(())
    }

    pub async fn delete_logo(&self) -> ServiceResult<()> {
        let mut removed = false;
        for (ext, _, _) in LOGO_FORMATS {
            removed |= tokio::fs::remove_file(logo_path(ext)).await.is_ok();
        }
        if !removed {
            return Err(ServiceError::NotFound("尚未上传图标".to_string()));
        }
        Ok(())
    }
}
