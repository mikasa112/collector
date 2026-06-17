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

timer.every(1000, function()
    local list = dc.read_all("pcs")
    if list then
    end
end)
