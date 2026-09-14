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

-- timer.every(1000, function()
--     local list = dc.read_all("pcs")
--     if list then
--         local table = {}
--         for _, item in ipairs(list) do
--             table[item.id] = item.value
--             override.set("/pcs", table)
--         end
--     end
-- end)
-- timer.every(1000, function()
--     local data = dc.read("pcs", 156)
--     if data then
--         if data.faults then
--             for index, value in ipairs(data.faults) do
--             end
--         end
--     end
-- end)
