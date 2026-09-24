MOD = {
    name = "采集层北向流转主程序",
    description = "统一北向MQTT采集上送与控制驱动引擎",
}

-- 插件加载：自动发现 plugins/ 目录下的所有插件文件，无需手动维护列表；
-- 想临时禁用某个插件，只需给文件名加 "_" 前缀（如 _example_logger.lua），加载失败只记录错误不影响主流程
for _, plugin_module in ipairs(plugin.list("plugins")) do
    local ok, err = pcall(require, plugin_module)
    if not ok then
        log.error("加载插件失败 [" .. plugin_module .. "]: " .. tostring(err))
    else
        log.info("插件已加载: " .. plugin_module)
    end
end

local engine = require("_engine")

-- 当前激活的项目定义文件（存放于 projects/ 目录下）
-- 如需切换不同测试项目，只需更改 require 路径即可
local active_project_module = "projects.奥林波斯"

local ok, project = pcall(require, active_project_module)
if not ok or not project then
    log.error("加载项目规则失败 [" .. tostring(active_project_module) .. "]: " .. tostring(project))
    return
end

log.info(">>> 启动项目北向规则引擎: " .. (project.MOD and project.MOD.name or active_project_module))

-- 启动通用流转引擎
engine.start(project)
