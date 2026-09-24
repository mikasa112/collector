-- 此文件仅用于编辑器代码提示，不会被引擎执行。
-- 需要安装 lua-language-server：https://github.com/LuaLS/lua-language-server

-----------------------------------------------------------------------
-- MOD - 脚本元信息（每个脚本必须定义）
-----------------------------------------------------------------------

---@class ModDef
---@field name        string    脚本名称，用于日志显示
---@field description string?   脚本描述（可选）
---@field depends     string[]? 依赖的其它顶层脚本文件名列表（不含目录，如 "a.lua"），仅对 `_manifest.lua` 注册的顶层脚本有效；
---                              依赖脚本未注册/未运行时本脚本会被跳过加载。缺省视为无依赖
---@field api_version integer?  声明本脚本兼容的引擎模组 API 版本；缺省视为兼容，声明但与引擎当前版本不一致时会被拒绝加载

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

--- 广播一个跨模块事件：所有仍在运行的顶层脚本（包括发出者自身）中通过 event.on 注册的同名处理器都会收到，
--- 用于顶层脚本之间的协作通信。payload 经 JSON 编解码在脚本间传递，仅支持可 JSON 序列化的值。
---@param name    string 事件名
---@param payload any    传给处理器的数据
function event.emit(name, payload) end

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
-- save - 持久化存档 API（按脚本文件名隔离，落盘保存，跨热重载/进程重启保留）
-----------------------------------------------------------------------

---@class SaveApi
save = {}

--- 写入一个值并立即落盘（支持 number/string/boolean/table）
---@param key   string
---@param value any
function save.set(key, value) end

--- 读取一个值，不存在时返回 nil
---@param key string
---@return any
function save.get(key) end

--- 删除一个键并立即落盘
---@param key string
function save.del(key) end

-----------------------------------------------------------------------
-- plugin - 插件目录自动发现 API（只在加载时扫描一次，不支持热重载）
-----------------------------------------------------------------------

---@class PluginApi
plugin = {}

--- 扫描 `subdir` 目录下的 .lua 文件（跳过 "_" 开头的文件，可借此临时禁用某个插件），
--- 返回形如 "subdir.filename" 的模块路径列表（按文件名排序），可直接传给 require。
---@param subdir string 相对本脚本所在目录的子目录名（如 "plugins"）
---@return string[]
function plugin.list(subdir) return {} end

-----------------------------------------------------------------------
-- hook - 同 VM 内插件钩子 API（扩展/替换机制，与 event 并列但语义不同：带返回值、支持替换）
-----------------------------------------------------------------------

---@class HookApi
hook = {}

--- 注册一个扩展处理器：emit 时按注册顺序全部执行，仅产生副作用，返回值被忽略。
--- 同一个钩子名可注册多个 on 处理器。
---@param name string             钩子名
---@param fn   fun(payload: any)  处理器函数
function hook.on(name, fn) end

--- 注册一个替换处理器：同一个钩子名只有最后注册的生效（之前注册的会被日志警告并静默失效）。
---@param name string                  钩子名
---@param fn   fun(payload: any): any  处理器函数，返回改写后的 payload
function hook.override(name, fn) end

--- 触发一个钩子点：先跑完全部 on 处理器（出错只记警告，不中断），
--- 再跑 override 处理器（有则用其返回值，无则原样返回 payload；出错会透传给调用者）。
---@param name    string 钩子名
---@param payload any    传给处理器的数据
---@return any 经处理后的 payload
function hook.emit(name, payload) end

-----------------------------------------------------------------------
-- 内置生命周期钩子名（用 hook.on 订阅，无需额外注册 API）
-----------------------------------------------------------------------

--- 内置钩子 "on_unload"：脚本即将被卸载/热重载或引擎关闭前触发（无 payload）。
--- 用 hook.on("on_unload", function() ... end) 订阅，做收尾清理（如保存最终状态）。
--- 出错只记警告，不影响正常卸载流程。

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

--- 压缩发布：payload 编码规则与 publish 相同，编码后的字节用 zstd 压缩后再发送。
---@param topic   string
---@param payload string|table|number|boolean
---@param opts    MqttPubSubOpts?
---@param level   integer zstd 压缩等级（1~22，越大压缩率越高但耗时越长）
function MqttConn:compressed_publish(topic, payload, opts, level) end

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
