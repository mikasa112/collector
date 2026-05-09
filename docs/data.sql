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
