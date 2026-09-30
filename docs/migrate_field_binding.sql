-- 字段绑定覆盖表迁移脚本：为已存在的数据库补充 t_field_binding_override 表
-- 新建库无需执行，docs/data.sql 已包含该表定义

CREATE TABLE IF NOT EXISTS t_field_binding_override (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    field_key VARCHAR(64) NOT NULL UNIQUE,
    dev_id VARCHAR(50) NOT NULL,
    point_kind TINYINT NOT NULL,
    point_value VARCHAR(50) NOT NULL,
    enabled TINYINT NOT NULL DEFAULT 1,
    updated_by VARCHAR(50),
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
