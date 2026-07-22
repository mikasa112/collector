CREATE TABLE t_user (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name TEXT,
    account TEXT NOT NULL UNIQUE, -- 通常账号应该是唯一的
    password TEXT NOT NULL,
	role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('admin', 'user', 'guest')),
	-- 创建时间：插入时自动生成
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 更新时间：初始与创建时间一致
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 删除时间：默认为 NULL，不为 NULL 时表示该记录已被软删除
    deleted_at DATETIME
);

CREATE TABLE  t_plan_curve_master (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    curve_name VARCHAR(100) NOT NULL,
    curve_type TINYINT NOT NULL,          -- 1-日计划 2-周计划 3-自定义
    priority INTEGER DEFAULT 5,
    status TINYINT DEFAULT 1,             -- 0-草稿 1-已发布 2-执行中 3-已归档
    valid_start_date TEXT,                -- SQLite用TEXT存日期 (格式: YYYY-MM-DD)
    valid_end_date TEXT,
    effective_weekdays VARCHAR(20),       -- 如 "1,2,3,4,5"
    created_by VARCHAR(50),
	-- 创建时间：插入时自动生成
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 更新时间：初始与创建时间一致
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 删除时间：默认为 NULL，不为 NULL 时表示该记录已被软删除
    deleted_at DATETIME,
    remark VARCHAR(255)
);

CREATE TABLE t_plan_curve_detail (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    curve_id INTEGER NOT NULL,
    time_index TINYINT NOT NULL,          -- 0-95，对应00:00-23:45
    power_value DECIMAL(10, 3) NOT NULL,  -- 正值=充电，负值=放电
    soc_limit DECIMAL(5, 2),              -- SOC上限(%)
    	-- 创建时间：插入时自动生成
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 更新时间：初始与创建时间一致
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 删除时间：默认为 NULL，不为 NULL 时表示该记录已被软删除
    deleted_at DATETIME,
    FOREIGN KEY (curve_id) REFERENCES t_plan_curve_master(id) ON DELETE CASCADE
);

-- 联合唯一索引：确保同一曲线下无重复时间点
CREATE UNIQUE INDEX idx_curve_time ON t_plan_curve_detail(curve_id, time_index);

-- 为提升查询效率，额外创建单列索引
CREATE INDEX idx_curve_id ON t_plan_curve_detail(curve_id);

-- EMU功能开关表：控制各项EMU功能是否启用
CREATE TABLE t_emu_function (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    function_code VARCHAR(50) NOT NULL,   -- 功能唯一编码，如 PLAN_CURVE、ANTI_BACKFLOW
    function_name VARCHAR(100) NOT NULL,  -- 功能名称
    enabled TINYINT NOT NULL DEFAULT 0,   -- 0-禁用 1-启用
    config TEXT,                          -- 功能相关配置参数(JSON，可为空)
    sort_order INTEGER DEFAULT 0,         -- 展示排序
    -- 创建时间：插入时自动生成
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 更新时间：初始与创建时间一致
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    -- 删除时间：默认为 NULL，不为 NULL 时表示该记录已被软删除
    deleted_at DATETIME,
    remark VARCHAR(255)
);

-- 功能编码唯一（排除软删除记录，允许历史编码在软删后复用）
CREATE UNIQUE INDEX idx_emu_function_code ON t_emu_function(function_code) WHERE deleted_at IS NULL;

-- 启用计划曲线功能
INSERT INTO t_emu_function (function_code, function_name, enabled, sort_order, remark)
VALUES ('PLAN_CURVE', '计划曲线控制', 1, 1, '按计划曲线执行充放电');
