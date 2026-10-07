-- 电价配置表迁移脚本：为已存在的数据库补充 t_electricity_price / t_electricity_period 表
-- 新建库无需执行，docs/data.sql 已包含这两张表定义

CREATE TABLE IF NOT EXISTS t_electricity_price (
    period_type TINYINT PRIMARY KEY,
    price REAL NOT NULL,
    updated_by VARCHAR(50),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS t_electricity_period (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    start_time VARCHAR(5) NOT NULL,
    end_time VARCHAR(5) NOT NULL,
    period_type TINYINT NOT NULL,
    updated_by VARCHAR(50),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
