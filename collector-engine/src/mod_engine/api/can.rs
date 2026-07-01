use collector_core::dev::can_bus::SharedCanBus;
use mlua::{Lua, Table};

pub(crate) fn create_can_table(lua: &Lua, bus: SharedCanBus) -> mlua::Result<Table> {
    let can_table = lua.create_table()?;

    // can.send(dev_id, frame_id, data)
    // data: Lua 整数数组，每个元素为 0-255 的字节值
    can_table.set(
        "send",
        lua.create_function(move |_, (dev_id, frame_id, data): (String, u32, Table)| {
            let bytes: Vec<u8> = data
                .sequence_values::<u8>()
                .collect::<Result<_, _>>()
                .map_err(|e| mlua::Error::runtime(format!("can.send data 解析失败: {e}")))?;
            if !bus.send(&dev_id, frame_id, bytes) {
                tracing::warn!("[mod] can.send: 设备 {} 未就绪或不存在", dev_id);
            }
            Ok(())
        })?,
    )?;

    Ok(can_table)
}
