-- 示例插件：演示 hook.on / hook.override 的用法
-- 本文件不出现在 _manifest.lua 里——插件不是独立顶层脚本(不是独立 VM)，
-- 而是被 require 进主脚本(main.lua)所在的同一个 VM 里的普通模块。

hook.on("before_publish", function(payload)
    log.info("[plugin:example_logger] 即将发布 " .. tostring(payload.topic))
end)

-- 如需改写/丢弃某次发布，改用 hook.override（同一钩子名只有最后注册的 override 生效）：
-- hook.override("before_publish", function(payload)
--     -- 返回改写后的 payload，或返回 nil 丢弃本次发布
--     return payload
-- end)

return {}
