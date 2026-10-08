PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE t_plan_curve_master (
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
INSERT INTO t_plan_curve_master VALUES(10,'廊坊工商业峰谷曲线',1,1,1,'2026-01-01','2026-12-31','1,2,3,4,5,6,7','system','2026-07-20 16:36:29','2026-10-02 23:11:07',NULL,'廊坊峰谷电价');
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
INSERT INTO t_plan_curve_detail VALUES(129,10,0,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(130,10,1,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(131,10,2,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(132,10,3,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(133,10,4,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(134,10,5,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(135,10,6,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(136,10,7,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(137,10,8,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(138,10,9,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(139,10,10,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(140,10,11,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(141,10,12,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(142,10,13,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(143,10,14,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(144,10,15,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(145,10,16,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(146,10,17,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(147,10,18,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(148,10,19,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(149,10,20,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(150,10,21,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(151,10,22,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(152,10,23,53,95,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(153,10,24,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(154,10,25,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(155,10,26,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(156,10,27,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(157,10,28,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(158,10,29,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(159,10,30,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(160,10,31,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(161,10,32,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(162,10,33,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(163,10,34,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(164,10,35,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(165,10,36,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(166,10,37,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(167,10,38,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(168,10,39,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(169,10,40,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(170,10,41,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(171,10,42,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(172,10,43,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(173,10,44,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(174,10,45,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(175,10,46,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(176,10,47,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(177,10,48,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(178,10,49,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(179,10,50,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(180,10,51,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(181,10,52,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(182,10,53,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(183,10,54,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(184,10,55,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(185,10,56,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(186,10,57,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(187,10,58,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(188,10,59,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(189,10,60,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(190,10,61,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(191,10,62,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(192,10,63,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(193,10,64,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(194,10,65,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(195,10,66,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(196,10,67,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(197,10,68,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(198,10,69,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(199,10,70,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(200,10,71,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(201,10,72,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(202,10,73,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(203,10,74,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(204,10,75,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(205,10,76,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(206,10,77,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(207,10,78,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(208,10,79,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(209,10,80,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(210,10,81,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(211,10,82,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(212,10,83,-64,5,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(213,10,84,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(214,10,85,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(215,10,86,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(216,10,87,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(217,10,88,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(218,10,89,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(219,10,90,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(220,10,91,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(221,10,92,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(222,10,93,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(223,10,94,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
INSERT INTO t_plan_curve_detail VALUES(224,10,95,0,NULL,'2026-07-20 16:36:29','2026-10-02 23:11:07',NULL);
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
INSERT INTO t_emu_function VALUES(1,'PLAN_CURVE','计划曲线控制',1,NULL,1,'2026-07-21 14:02:23','2026-10-02 22:59:31',NULL,'按计划曲线执行充放电');
CREATE TABLE t_alarm (
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    alarm_code INTEGER NOT NULL,             -- 告警码
    alarm_name VARCHAR(100) NOT NULL,        -- 告警名称
    alarm_dev VARCHAR(100) NOT NULL,         -- 告警设备
    alarm_level INTEGER NOT NULL,            -- 告警等级
    alarm_status INTEGER DEFAULT 1,          -- 0: 已恢复 1: 正在发生

    created_by VARCHAR(50),

    -- 创建时间，同时作为故障发生时间
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),

    -- 更新时间，告警恢复时更新，作为恢复时间
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),

    -- 软删除时间
    deleted_at DATETIME
);
CREATE TABLE IF NOT EXISTS "t_user" (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name TEXT,
    account TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('super_admin', 'admin', 'user')),
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    deleted_at DATETIME
);
INSERT INTO t_user VALUES(1,'admin','admin','$argon2id$v=19$m=19456,t=2,p=1$8V5g+WB0+uVCvGaJrjY9Yw$5oFDvuZloKHwEnU9EdC/6RInYs7hINigPVuf0ZOt8r8','super_admin','2026-05-09 10:29:44','2026-05-09 10:29:44',NULL);
CREATE TABLE t_field_binding_override (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    field_key VARCHAR(64) NOT NULL UNIQUE,  -- 逻辑字段名，如 soc、bcu_current
    dev_id VARCHAR(50) NOT NULL,            -- 绑定到的设备id
    point_kind TINYINT NOT NULL,            -- 点位引用方式: 1-Id 2-Key 3-Name
    point_value VARCHAR(50) NOT NULL,       -- Id存数字文本，Key/Name存字符串
    enabled TINYINT NOT NULL DEFAULT 1,     -- 0-已重置为默认(保留记录) 1-生效覆盖
    updated_by VARCHAR(50),
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
CREATE TABLE t_project_info (
    key VARCHAR(32) PRIMARY KEY,
    value TEXT NOT NULL,
    updated_by VARCHAR(50),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
INSERT INTO t_project_info VALUES('name','英博电气廊坊基地储能项目','','2026-10-07 16:55:21');
INSERT INTO t_project_info VALUES('title','英博能量管理系统','','2026-10-07 16:55:41');
INSERT INTO t_project_info VALUES('version','V1.2','','2026-10-07 16:55:54');
INSERT INTO t_project_info VALUES('rated_power_kw','125','','2026-10-07 16:56:10');
INSERT INTO t_project_info VALUES('rated_energy_kwh','261','','2026-10-07 16:56:28');
CREATE TABLE t_electricity_price (
    period_type TINYINT PRIMARY KEY,
    price REAL NOT NULL,
    updated_by VARCHAR(50),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
CREATE TABLE t_electricity_period (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    start_time VARCHAR(5) NOT NULL,
    end_time VARCHAR(5) NOT NULL,
    period_type TINYINT NOT NULL,
    updated_by VARCHAR(50),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime'))
);
PRAGMA writable_schema=ON;
CREATE TABLE IF NOT EXISTS sqlite_sequence(name,seq);
DELETE FROM sqlite_sequence;
INSERT INTO sqlite_sequence VALUES('t_plan_curve_master',10);
INSERT INTO sqlite_sequence VALUES('t_plan_curve_detail',416);
INSERT INTO sqlite_sequence VALUES('t_emu_function',1);
INSERT INTO sqlite_sequence VALUES('t_alarm',8);
INSERT INTO sqlite_sequence VALUES('t_user',1);
CREATE UNIQUE INDEX idx_curve_time ON t_plan_curve_detail(curve_id, time_index)
;
CREATE INDEX idx_curve_id ON t_plan_curve_detail(curve_id);
CREATE UNIQUE INDEX idx_emu_function_code ON t_emu_function(function_code) WHERE deleted_at IS NULL
;
PRAGMA writable_schema=OFF;
COMMIT;
