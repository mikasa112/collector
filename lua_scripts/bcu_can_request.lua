MOD = {
    name        = "BCU CAN Request",
    description = "永泰BCU请求响应式CAN通讯, 固定发送报文",
}

task.spawn(function()
    while true do
        can.send("bcu", 0x40B0140, { 0x00, 0x3E })
        wait(100)
        can.send("bcu", 0x40B0180, { 0x00, 0x3E })
        wait(100)
        can.send("bcu", 0x40B0080, { 0x00, 0x3B })
        wait(100)
        can.send("bcu", 0x40B00C0, { 0x00, 0x37 })
        wait(100)
        can.send("bcu", 0x40B0103, { 0x00, 0x3D })
        wait(100)
        can.send("bcu", 0x40B0A36, { 0x00, 0x01 })
        wait(100)
        can.send("bcu", 0x40B0B00, { 0x00, 0x01 })
        wait(100)
        can.send("bcu", 0x40B0B41, { 0x00, 0x0A })
        wait(100)
        can.send("bcu", 0x40B0200, { 0x00, 0x91 })
        wait(100)
        can.send("bcu", 0x40B0400, { 0x00, 0x82 })
        wait(100)
        can.send("bcu", 0x40B0482, { 0x00, 0x82 })
        wait(100)
    end
end)
