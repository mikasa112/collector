//! 字段元模型：策略只认逻辑字段名（如 `soc`），字段到物理点位（设备+点位）的绑定关系
//! 可由管理端在 `t_field_binding_override` 表中覆盖；未覆盖时用策略注册的默认值兜底。
//! 覆盖表的读写由 collector-api 负责，这里只负责把覆盖表加载进内存供策略读取，
//! 写入后调用一次 [`FieldRegistry::reload`] 即可立即生效，无需重启。

use std::sync::{LazyLock, RwLock};

use ahash::AHashMap;
use dashmap::DashMap;
use sqlx::{Row, SqlitePool};

use crate::{
    center::{DataCenterError, data_center},
    core::point::{DataPoint, DownDataPoint, PointRef, Val},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldDirection {
    Read,
    Write,
    ReadWrite,
}

impl FieldDirection {
    fn readable(self) -> bool {
        matches!(self, FieldDirection::Read | FieldDirection::ReadWrite)
    }

    fn writable(self) -> bool {
        matches!(self, FieldDirection::Write | FieldDirection::ReadWrite)
    }
}

/// 策略在启动期声明的逻辑字段，`default_point` 只支持 [`PointRef::Id`]，
/// 因为这里需要以 `const` 数组的方式声明，Key/Name 变体持有 `String` 无法 const 构造。
#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub key: &'static str,
    pub name: &'static str,
    pub direction: FieldDirection,
    pub default_dev: &'static str,
    pub default_point: PointRef,
}

/// 简化 [`FieldSpec`] 常量数组的声明，例如：
/// `field_spec!("soc", "SOC", Read, "bcu", Id(32))`
/// 展开为 `FieldSpec { key: "soc", name: "SOC", direction: FieldDirection::Read, default_dev: "bcu", default_point: PointRef::Id(32) }`。
/// 用 `$crate` 限定路径，调用侧不需要额外 `use FieldDirection`/`PointRef`。
#[macro_export]
macro_rules! field_spec {
    ($key:expr, $name:expr, $direction:ident, $dev:expr, $point_variant:ident($point_value:expr)) => {
        $crate::field::FieldSpec {
            key: $key,
            name: $name,
            direction: $crate::field::FieldDirection::$direction,
            default_dev: $dev,
            default_point: $crate::core::point::PointRef::$point_variant($point_value),
        }
    };
}

#[derive(Debug, Clone)]
pub struct FieldBinding {
    pub dev_id: String,
    pub point: PointRef,
}

#[derive(Debug, thiserror::Error)]
pub enum FieldError {
    #[error("未注册的字段: {0}")]
    UnknownField(String),
    #[error("字段`{0}`不支持该操作方向")]
    WrongDirection(String),
    #[error("数据库错误: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("{0}")]
    DataCenter(#[from] DataCenterError),
}

/// 运行时字段注册表：`defaults` 由策略启动期注册，`overrides` 由管理端写入后触发 [`reload`](FieldRegistry::reload) 刷新
pub struct FieldRegistry {
    defaults: DashMap<&'static str, FieldSpec>,
    overrides: RwLock<AHashMap<String, FieldBinding>>,
}

static FIELD_REGISTRY: LazyLock<FieldRegistry> = LazyLock::new(FieldRegistry::new);

pub fn field_registry() -> &'static FieldRegistry {
    &FIELD_REGISTRY
}

impl FieldRegistry {
    fn new() -> Self {
        Self {
            defaults: DashMap::new(),
            overrides: RwLock::new(AHashMap::new()),
        }
    }

    /// 供策略在启动期注册自己需要的字段，同名字段以后注册者覆盖
    pub fn register(&self, specs: &[FieldSpec]) {
        for spec in specs {
            self.defaults.insert(spec.key, spec.clone());
        }
    }

    pub fn spec(&self, key: &str) -> Option<FieldSpec> {
        self.defaults.get(key).map(|s| s.clone())
    }

    pub fn all_specs(&self) -> Vec<FieldSpec> {
        self.defaults.iter().map(|e| e.value().clone()).collect()
    }

    /// 解析字段当前生效的绑定：覆盖表优先，否则回落到策略声明的默认值
    pub fn resolve(&self, key: &str) -> Option<FieldBinding> {
        let overrides = self
            .overrides
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(binding) = overrides.get(key) {
            return Some(binding.clone());
        }
        drop(overrides);
        self.spec(key).map(|spec| FieldBinding {
            dev_id: spec.default_dev.to_string(),
            point: spec.default_point,
        })
    }

    /// 读取字段当前值，字段未注册或方向不可读时返回错误；点位本身不存在时返回 `Ok(None)`
    pub fn read(&self, key: &str) -> Result<Option<DataPoint>, FieldError> {
        let spec = self
            .spec(key)
            .ok_or_else(|| FieldError::UnknownField(key.to_string()))?;
        if !spec.direction.readable() {
            return Err(FieldError::WrongDirection(key.to_string()));
        }
        let binding = self.resolve(key).expect("已注册字段必有绑定");
        let point = match &binding.point {
            PointRef::Id(id) => data_center().read(&binding.dev_id, *id),
            PointRef::Key(k) => data_center().read_by_key(&binding.dev_id, k),
            // DataCenter 本身不支持按 Name 读取，绑定为 Name 的只读字段在管理端会被拒绝写入
            PointRef::Name(_) => None,
        };
        Ok(point)
    }

    /// 下发字段的新值，字段未注册或方向不可写时返回错误
    pub async fn dispatch(&self, key: &str, value: Val) -> Result<(), FieldError> {
        let spec = self
            .spec(key)
            .ok_or_else(|| FieldError::UnknownField(key.to_string()))?;
        if !spec.direction.writable() {
            return Err(FieldError::WrongDirection(key.to_string()));
        }
        let binding = self.resolve(key).expect("已注册字段必有绑定");
        data_center()
            .dispatch(
                &binding.dev_id,
                vec![DownDataPoint {
                    point: binding.point,
                    value,
                }],
            )
            .await
            .map_err(FieldError::DataCenter)
    }

    /// 从覆盖表重新加载生效绑定；非法行只记录警告并跳过，全部处理完后整体替换，避免半新半旧
    pub async fn reload(&self, pool: &SqlitePool) -> Result<(), FieldError> {
        let rows = sqlx::query(
            "SELECT field_key, dev_id, point_kind, point_value
             FROM t_field_binding_override
             WHERE enabled = 1",
        )
        .fetch_all(pool)
        .await?;

        let mut next = AHashMap::with_capacity(rows.len());
        for row in rows {
            let field_key: String = row.try_get("field_key")?;
            let dev_id: String = row.try_get("dev_id")?;
            let point_kind: i64 = row.try_get("point_kind")?;
            let point_value: String = row.try_get("point_value")?;

            if self.spec(&field_key).is_none() {
                tracing::warn!("[field] 覆盖表中存在未注册的字段`{field_key}`, 已跳过");
                continue;
            }
            let point = match point_kind {
                1 => match point_value.parse::<u32>() {
                    Ok(id) => PointRef::Id(id),
                    Err(_) => {
                        tracing::warn!(
                            "[field] 字段`{field_key}`的point_value`{point_value}`不是合法的点位ID, 已跳过"
                        );
                        continue;
                    }
                },
                2 => PointRef::Key(point_value),
                3 => PointRef::Name(point_value),
                _ => {
                    tracing::warn!(
                        "[field] 字段`{field_key}`的point_kind`{point_kind}`不合法, 已跳过"
                    );
                    continue;
                }
            };
            next.insert(field_key, FieldBinding { dev_id, point });
        }

        let mut guard = self
            .overrides
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = next;
        Ok(())
    }
}
