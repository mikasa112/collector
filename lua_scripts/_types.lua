-- 此文件仅用于编辑器代码提示，不会被引擎执行。
-- 需要安装 lua-language-server：https://github.com/LuaLS/lua-language-server

-----------------------------------------------------------------------
-- MOD - 脚本元信息（每个脚本必须定义）
-----------------------------------------------------------------------

---@class ModDef
---@field name        string  脚本名称，用于日志显示
---@field description string? 脚本描述（可选）

--- 脚本元信息，引擎启动时读取
---@type ModDef
MOD = {}

-----------------------------------------------------------------------
-- DataPoint
-----------------------------------------------------------------------

---@class StatusWord
---@field zh string 中文描述
---@field en string 英文描述

---@class FaultBit
---@field bit   integer 位索引（0~15）
---@field zh    string  中文描述
---@field en    string  英文描述
---@field level integer 告警等级（0=无 1=普通 2=高 3=严重）

---@class DataPoint
---@field id     integer    数据点 ID
---@field key    string     数据点 key（英文标识）
---@field name   string     数据点名称（中文描述）
---@field value  number     当前值
---@field status StatusWord|nil 状态字解析结果（仅状态字点位有效，值未匹配时为 nil）
---@field faults FaultBit[] 故障字解析结果（仅故障字点位有效，无故障时为空表）

-----------------------------------------------------------------------
-- dc - 数据中心 API
-----------------------------------------------------------------------

---@class DcApi
dc = {}

--- 返回所有设备 ID 列表
---@return string[]
function dc.dev_ids()
    return {}
end

--- 读取某设备所有数据点
---@param dev_id string 设备 ID
---@return DataPoint[]
function dc.read_all(dev_id)
    return {}
end

--- 读取单个数据点，不存在时返回 nil
---@param dev_id   string          设备 ID
---@param point_id integer|string  数据点 ID 或 key
---@return DataPoint|nil
function dc.read(dev_id, point_id) end

--- 向设备下发数值
---@param dev_id     string         设备 ID
---@param point_mark integer|string 数据点 ID 或 key
---@param value      number         要写入的值（仅支持数值类型）
function dc.dispatch(dev_id, point_mark, value) end

--- 订阅设备数据变化。调用后，该设备数据每次变化时会触发 "dc:changed" 事件。
--- 同一设备重复调用无副作用。设备不存在时记录警告并忽略。
---@param dev_id string 设备 ID
function dc.watch(dev_id) end

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
-- json - JSON 编解码 API
-----------------------------------------------------------------------

---@class JsonApi
json = {}

--- 将 JSON 字符串解析为 Lua 值（table/number/string/boolean/nil）
---@param s string
---@return any
function json.decode(s) end

--- 将 Lua 值编码为 JSON 字符串
---@param value any
---@return string
function json.encode(value) end

-----------------------------------------------------------------------
-- task - 协程任务 API
-----------------------------------------------------------------------

---@class TaskApi
task = {}

--- 启动一个协程任务，函数体内可使用 wait() 挂起
---@param fn fun() 任务函数
function task.spawn(fn) end

-----------------------------------------------------------------------
-- wait - 协程挂起
-----------------------------------------------------------------------

---@diagnostic disable-next-line: lowercase-global
--- 在 task.spawn 的协程内挂起指定毫秒，不阻塞其他协程
---@param ms integer 毫秒数
function wait(ms) end

-----------------------------------------------------------------------
-- event - 事件订阅 API
-----------------------------------------------------------------------

---@class EventApi
event = {}

--- 订阅一个事件，当引擎 emit 该事件时触发回调
---@param name string   事件名
---@param fn   fun(value: any) 回调函数
function event.on(name, fn) end

-----------------------------------------------------------------------
-- timer - 定时器 API（基于回调，适合简单场景；复杂逻辑推荐 task.spawn + wait）
-----------------------------------------------------------------------

---@class TimerApi
timer = {}

--- 延迟执行一次（一次性定时器）
---@param ms integer 延迟毫秒数
---@param fn fun()   回调函数
function timer.after(ms, fn) end

--- 周期执行（循环定时器）
---@param ms integer 间隔毫秒数
---@param fn fun()   回调函数
function timer.every(ms, fn) end

-----------------------------------------------------------------------
-- store - 脚本间共享 KV 存储（同一 ScriptManager 下所有脚本共享同一实例）
-----------------------------------------------------------------------

---@class StoreApi
store = {}

--- 写入一个值（支持 number/string/boolean/table）
---@param key   string
---@param value any
function store.set(key, value) end

--- 读取一个值，不存在时返回 nil
---@param key string
---@return any
function store.get(key) end

--- 删除一个键
---@param key string
function store.del(key) end

-----------------------------------------------------------------------
-- DcChangedEvent - dc:changed 事件 payload
-----------------------------------------------------------------------

---@class DcChangedEvent
---@field dev    string      触发变化的设备 ID
---@field points DataPoint[] 变化后的全量数据点列表

-----------------------------------------------------------------------
-- can - CAN 总线原始帧发送 API（仅在有 CAN 设备时可用）
-----------------------------------------------------------------------

---@class CanApi
can = {}

--- 向指定 CAN 设备发送原始帧
--- frame_id <= 0x7FF 时使用标准帧，否则使用扩展帧
---@param dev_id   string    设备 ID（与 config.json 中的设备 ID 一致）
---@param frame_id integer   CAN 帧 ID（十六进制或十进制均可）
---@param data     integer[] 帧数据字节数组，每个元素为 0~255
function can.send(dev_id, frame_id, data) end

-----------------------------------------------------------------------
-- override - MQTT 覆盖推送 API（仅在 MQTT 已配置时可用）
-----------------------------------------------------------------------

---@class OverrideApi
override = {}

--- 覆盖指定 topic 的推送内容
---@param topic string  MQTT topic
---@param value any     要推送的值（table/number/string/boolean）
function override.set(topic, value) end

--- 取消覆盖，恢复原始采集值
---@param topic string  MQTT topic
function override.clear(topic) end

-----------------------------------------------------------------------
-- mqtt - 通用 MQTT 客户端 API（脚本自行发起连接，与 override 表完全独立）
-----------------------------------------------------------------------

---@class MqttConnectOpts
---@field host      string  broker 地址
---@field port      integer? 端口，默认 1883
---@field client_id string?  客户端 ID，缺省自动生成
---@field username  string?  用户名（可选）
---@field password  string?  密码（可选）
---@field keepalive integer? 心跳间隔（秒），默认 30
---@field max_packet_size integer? 收发包体大小上限（字节），默认 262144（256KB）

---@class MqttPubSubOpts
---@field qos    integer? QoS 等级 0/1/2，默认 0
---@field retain boolean? 是否 retain（仅 publish 有效），默认 false

---@class MqttConn
local MqttConn = {}

--- 发布消息。payload 为字符串时按原始字节发送；为 table 时自动编码为 JSON；
--- 为 number/boolean 时转为字符串发送。
---@param topic   string
---@param payload string|table|number|boolean
---@param opts    MqttPubSubOpts?
function MqttConn:publish(topic, payload, opts) end

--- 订阅 topic（支持标准通配符 +/#），收到消息时以 (topic, payload) 触发回调，
--- payload 是原始字符串（二进制安全），需要 JSON 时自行解析。
---@param topic_filter string
---@param callback     fun(topic: string, payload: string)
---@param opts         MqttPubSubOpts?
function MqttConn:subscribe(topic_filter, callback, opts) end

--- 取消订阅
---@param topic_filter string
function MqttConn:unsubscribe(topic_filter) end

--- 主动断开连接。脚本卸载/热更新时引擎也会自动断开所有未关闭的连接。
function MqttConn:disconnect() end

---@class MqttApi
mqtt = {}

--- 发起一个独立的 MQTT 连接。连接失败（超时或被拒绝）时返回 nil, err，
--- 因此需要用两个返回值接收。
---@param opts MqttConnectOpts
---@return MqttConn|nil conn
---@return string|nil   err
function mqtt.connect(opts) end
