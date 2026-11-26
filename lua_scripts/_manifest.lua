-- 模块注册总纲
--
-- 只有在这里列出的脚本文件名（相对本目录顶层，不含子目录）才会被引擎加载执行，
-- 未列出的 .lua 脚本即使存在于目录下也不会运行。
-- 本文件本身以 "_" 开头，不会被当作模块加载。
--
-- 允许写任意 Lua 逻辑（比如按条件判断），只需最终 return 一个字符串数组即可，
-- 引擎在启动时以及每次本文件被修改时都会重新执行取值。
--
-- 数组元素有两种写法：
--   "main.lua"                                  只注册，不授予任何数据库权限
--   { file = "curve.lua",                       注册并声明该脚本可访问的业务库表
--     db = { read  = { "t_alarm" },             只读表
--            write = { "t_plan_curve_detail" } } }  可写表（隐含读权限）
-- 表名可写 "*" 表示所有表。未声明的表，db.query / db.exec 一律拒绝；
-- 所有 db.exec 写操作（含被拒绝/失败的）都会记录到 t_operation_log（operator = "lua:<脚本名>"）。
return {
    -- "main.lua",
    -- { file = "curve.lua", db = { read = { "t_plan_curve_master" }, write = { "t_plan_curve_detail" } } },
}
