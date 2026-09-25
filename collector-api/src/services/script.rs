use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use tokio::fs;

use crate::services::system::CONFIG_PATH;
use crate::services::{ServiceError, ServiceResult};

const DEFAULT_SCRIPT_DIR: &str = "lua_scripts";

#[derive(Debug, Serialize)]
pub struct ScriptEntry {
    /// 相对脚本根目录的路径，统一用 `/` 分隔
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
}

pub struct ScriptService {
    root: PathBuf,
}

impl ScriptService {
    pub async fn new() -> ServiceResult<Self> {
        Ok(Self {
            root: PathBuf::from(script_dir_from_config().await),
        })
    }

    /// 递归列出脚本目录下所有 .lua 文件与子目录（隐藏文件/目录跳过）
    pub async fn list_tree(&self) -> ServiceResult<Vec<ScriptEntry>> {
        let mut entries = Vec::new();
        collect_entries(&self.root, &self.root, &mut entries).await?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    pub async fn read_file(&self, rel_path: &str) -> ServiceResult<String> {
        let path = self.resolve(rel_path)?;
        fs::read_to_string(&path)
            .await
            .map_err(|_| ServiceError::not_found("脚本文件不存在"))
    }

    /// 新建脚本：目标文件必须不存在
    pub async fn create_file(&self, rel_path: &str, content: &str) -> ServiceResult<()> {
        let path = self.resolve(rel_path)?;
        if fs::try_exists(&path).await.unwrap_or(false) {
            return Err(ServiceError::already_exists("脚本文件已存在"));
        }
        self.write(&path, content).await
    }

    /// 保存脚本：覆盖写入，目标文件需已存在
    pub async fn save_file(&self, rel_path: &str, content: &str) -> ServiceResult<()> {
        let path = self.resolve(rel_path)?;
        if !fs::try_exists(&path).await.unwrap_or(false) {
            return Err(ServiceError::not_found("脚本文件不存在"));
        }
        self.write(&path, content).await
    }

    pub async fn delete_file(&self, rel_path: &str) -> ServiceResult<()> {
        let path = self.resolve(rel_path)?;
        fs::remove_file(&path)
            .await
            .map_err(|_| ServiceError::not_found("脚本文件不存在"))
    }

    async fn write(&self, path: &Path, content: &str) -> ServiceResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        fs::write(&tmp, content.as_bytes()).await?;
        fs::rename(&tmp, path).await?;
        Ok(())
    }

    /// 校验相对路径不越界、限定 .lua 扩展名，返回目录下的绝对/相对磁盘路径
    fn resolve(&self, rel_path: &str) -> ServiceResult<PathBuf> {
        if !rel_path.ends_with(".lua") {
            return Err(ServiceError::invalid_parameter("仅支持 .lua 脚本文件"));
        }
        let rel = Path::new(rel_path);
        if rel_path.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(ServiceError::invalid_parameter("非法的文件路径"));
        }
        Ok(self.root.join(rel))
    }
}

async fn script_dir_from_config() -> String {
    let Ok(bytes) = fs::read(CONFIG_PATH).await else {
        return DEFAULT_SCRIPT_DIR.to_string();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| {
            v.get("program")?
                .get("mod")?
                .get("path")?
                .as_str()
                .map(str::to_owned)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SCRIPT_DIR.to_string())
}

fn collect_entries<'a>(
    root: &'a Path,
    dir: &'a Path,
    out: &'a mut Vec<ScriptEntry>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ServiceResult<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = match fs::read_dir(dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = entry.metadata().await?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if metadata.is_dir() {
                out.push(ScriptEntry {
                    path: rel,
                    is_dir: true,
                    size: 0,
                    modified: String::new(),
                });
                collect_entries(root, &path, out).await?;
            } else if name_str.ends_with(".lua") {
                let modified = metadata
                    .modified()
                    .ok()
                    .map(|t| {
                        chrono::DateTime::<chrono::Local>::from(t)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                out.push(ScriptEntry {
                    path: rel,
                    is_dir: false,
                    size: metadata.len(),
                    modified,
                });
            }
        }
        Ok(())
    })
}
