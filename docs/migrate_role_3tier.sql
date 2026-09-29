-- 将 t_user.role 从旧的二级体系 (admin/user/guest) 迁移到新的三级体系
-- (super_admin/admin/user)。SQLite 的 CHECK 约束无法直接 ALTER，采用
-- "建新表 -> 拷贝数据 -> 删旧表 -> 改名" 的标准做法。
--
-- 用法：对已部署的 data.db 手动执行一次
--   sqlite3 data.db < docs/migrate_role_3tier.sql
--
-- 数据映射：旧的 'admin'（原最高权限）-> 'super_admin'；'guest' -> 'user'；'user' -> 'user'

BEGIN TRANSACTION;

CREATE TABLE t_user_new (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    name TEXT,
    account TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('super_admin', 'admin', 'user')),
    created_at DATETIME DEFAULT (datetime('now', 'localtime')),
    updated_at DATETIME DEFAULT (datetime('now', 'localtime')),
    deleted_at DATETIME
);

INSERT INTO t_user_new (id, name, account, password, role, created_at, updated_at, deleted_at)
SELECT
    id,
    name,
    account,
    password,
    CASE role
        WHEN 'admin' THEN 'super_admin'
        WHEN 'guest' THEN 'user'
        ELSE role
    END,
    created_at,
    updated_at,
    deleted_at
FROM t_user;

DROP TABLE t_user;
ALTER TABLE t_user_new RENAME TO t_user;

COMMIT;
