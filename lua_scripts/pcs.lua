MOD = {
    name        = "pcs测试",
    description = "",
}
-- task.spawn(function()
--     while true do
--         local data = dc.read("pcs", "pcsPortAPhaseVoltage")
--         log.info("data:" .. tostring(data.value))
--         wait(2000)
--     end
-- end)

local data = dc.read("pcs", 5)
if data then
    log.info("data:" .. tostring(data))
end
