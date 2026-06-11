-- 此文件仅用于编辑器代码提示，不会被引擎执行。
-- 需要安装 lua-language-server：https://github.com/LuaLS/lua-language-server

-----------------------------------------------------------------------
-- DataPoint
-----------------------------------------------------------------------

---@class DataPoint
---@field id      integer  数据点 ID
---@field key     string   数据点 key（英文标识）
---@field name    string   数据点名称（中文描述）
---@field value   number   当前值

-----------------------------------------------------------------------
-- dc - 数据中心 API
-----------------------------------------------------------------------

---@class DcApi
dc = {}

--- 返回所有设备 ID 列表
---@return string[]
function dc.dev_ids() end

--- 读取某设备所有数据点（按 id 升序排列）
---@param dev_id string 设备 ID
---@return DataPoint[]
function dc.read_all(dev_id) end

--- 读取单个数据点，不存在时返回 nil
---@param dev_id   string  设备 ID
---@param point_id integer 数据点 ID
---@return DataPoint|nil
function dc.read(dev_id, point_id) end

--- 向设备下发数值（异步）
---@param dev_id   string  设备 ID
---@param point_id integer 数据点 ID
---@param value    number  要写入的值（仅支持数值类型）
function dc.dispatch(dev_id, point_id, value) end

-----------------------------------------------------------------------
-- log - 日志 API
-----------------------------------------------------------------------

---@class LogApi
log = {}

---@param msg string
function log.info(msg) end

---@param msg string
function log.warn(msg) end

---@param msg string
function log.error(msg) end

-----------------------------------------------------------------------
-- sleep - 延迟（异步）
-----------------------------------------------------------------------

--- 暂停执行指定毫秒数
---@param ms integer 毫秒数
function sleep(ms) end

-----------------------------------------------------------------------
-- TASK - 脚本元信息（每个脚本必须定义）
-----------------------------------------------------------------------

---@class TaskDef
---@field name     string  脚本名称，用于日志显示
---@field interval integer 执行间隔（毫秒），与 schedule 二选一
---@field schedule string  cron 表达式（6字段含秒），与 interval 二选一

--- 脚本元信息，引擎启动时读取
---@type TaskDef
TASK = {}
