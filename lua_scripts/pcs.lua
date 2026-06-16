MOD = {
    name        = "pcs测试",
    description = "",
}

local data = dc.read("pcs", "pcsPortAPhaseVoltage")
log.info("data:" .. tostring(data.name))
