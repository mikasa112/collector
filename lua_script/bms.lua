TASK = {
    name     = "国轩BMS心跳",
    interval = 1000,
}

local watchdog = 0

function on_load()
end

function on_tick()
    local ok, err = pcall(function()
        dc.dispatch("bms", 2018, watchdog)
    end)
    if not ok then
        log.error("看门狗下发失败: " .. tostring(err))
    end

    watchdog = (watchdog + 1) % 65536
end
