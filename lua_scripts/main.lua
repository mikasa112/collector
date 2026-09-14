MOD = {
    name = "采集层北向流转主程序",
    description = "统一北向MQTT采集上送与控制驱动引擎",
}

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
