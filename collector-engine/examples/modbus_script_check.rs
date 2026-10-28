//! Lua 脚本 Modbus 连接/读写 + 注册为数据中心设备的端到端验证：
//! 本地起一个最小 Modbus TCP 模拟从站，由 Lua 脚本连接、读取、上报点位，
//! 并通过 `dc.dispatch` 触发 on_write 回调向从站下发。
//! 用法：cargo run -p collector-engine --example modbus_script_check
use std::sync::{Arc, Mutex};

use collector_core::{center::data_center, core::point::Val};
use collector_engine::mod_engine::ScriptManager;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

type Regs = Arc<Mutex<[u16; 64]>>;

async fn serve(listener: TcpListener, regs: Regs) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let regs = regs.clone();
        tokio::spawn(async move {
            loop {
                let mut head = [0u8; 6];
                if sock.read_exact(&mut head).await.is_err() {
                    return;
                }
                let len = u16::from_be_bytes([head[4], head[5]]) as usize;
                let mut body = vec![0u8; len];
                if sock.read_exact(&mut body).await.is_err() {
                    return;
                }
                let (uid, fc) = (body[0], body[1]);
                let addr = u16::from_be_bytes([body[2], body[3]]) as usize;
                let mut pdu = vec![fc];
                {
                    let mut r = regs.lock().unwrap();
                    match fc {
                        3 => {
                            let n = u16::from_be_bytes([body[4], body[5]]) as usize;
                            pdu.push((n * 2) as u8);
                            r[addr..addr + n]
                                .iter()
                                .for_each(|v| pdu.extend(v.to_be_bytes()));
                        }
                        6 => {
                            r[addr] = u16::from_be_bytes([body[4], body[5]]);
                            pdu.extend(&body[2..6]);
                        }
                        16 => {
                            let n = u16::from_be_bytes([body[4], body[5]]) as usize;
                            for i in 0..n {
                                r[addr + i] =
                                    u16::from_be_bytes([body[7 + i * 2], body[8 + i * 2]]);
                            }
                            pdu.extend(&body[2..6]);
                        }
                        _ => pdu = vec![fc | 0x80, 1], // 非法功能码
                    }
                }
                let mut resp = head[..4].to_vec();
                resp.extend(((pdu.len() + 1) as u16).to_be_bytes());
                resp.push(uid);
                resp.extend(pdu);
                if sock.write_all(&resp).await.is_err() {
                    return;
                }
            }
        });
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let regs: Regs = Arc::new(Mutex::new([0; 64]));
    // 地址 0..2 放一个 CDAB 的 U32 = 65538，地址 2 放有符号 -5
    regs.lock().unwrap()[..3].copy_from_slice(&[0x0002, 0x0001, (-5i16) as u16]);
    tokio::spawn(serve(listener, regs.clone()));

    let dir = std::env::temp_dir().join(format!("collector-modbus-check-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(
        dir.join("mb.lua"),
        format!(
            r#"MOD = {{ name = "modbus-check" }}
task.spawn(function()
    local m, err = modbus.connect{{ type = "tcp", host = "127.0.0.1", port = {port}, slave = 1, timeout = 500 }}
    assert(m, err)

    -- 注册为数据中心设备；同名设备已存在时应返回 nil, err
    local dev, derr = dc.register("lua_dev", function(points)
        for _, p in ipairs(points) do
            if p.key == "setpoint" then
                local ok, e = m:write_register(10, p.value)
                log.info("on_write setpoint=" .. p.value .. " ok=" .. tostring(ok) .. " err=" .. tostring(e))
            end
        end
    end)
    assert(dev, derr)
    local dup, dup_err = dc.register("lua_dev")
    assert(dup == nil and dup_err, "重复注册应失败")

    local r, rerr = m:read_holding(0, 3)
    assert(r, rerr)
    dev:ingest{{
        {{ id = 1, key = "energy", name = "电量", value = modbus.decode(r, "U32", "CDAB"), type = "u32", unit = "kWh" }},
        {{ id = 2, key = "temp",   name = "温度", value = modbus.decode({{ r[3] }}, "I16"), type = "i16" }},
    }}
    dev:set_comm(true)

    -- 写多个寄存器 + 越界异常不应让连接断开
    assert(m:write_registers(20, modbus.encode(-123456, "I32", "CDAB")))
    local bad, berr = m:read_coils(0, 8) -- 模拟从站不支持 FC01，返回异常码
    log.info("read_coils 预期失败: " .. tostring(berr))
    assert(bad == nil and m:is_connected(), "从站异常码不应断开连接")

end)"#
        ),
    )
    .await
    .unwrap();

    let shutdown = CancellationToken::new();
    let (dir2, sd2) = (dir.clone(), shutdown.clone());
    let join = tokio::spawn(async move {
        ScriptManager::new(None, None)
            .run(&dir2, sd2)
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let dc = data_center();
    // 等同 Lua 侧 dc.dispatch（后者依赖 get_runtime 需要数据库，示例里绕开）
    dc.dispatch(
        "lua_dev",
        vec![collector_core::down!(key: "setpoint".to_string(), Val::I32(777))],
    )
    .await
    .expect("下发失败");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let energy = dc
        .read_by_key("lua_dev", "energy")
        .expect("energy 点位未上报");
    let temp = dc.read_by_key("lua_dev", "temp").expect("temp 点位未上报");
    let comm = dc.read("lua_dev", 0xFFFF).expect("通讯状态未上报");
    assert_eq!(energy.value, Val::U32(65538));
    assert_eq!(temp.value, Val::I16(-5));
    assert_eq!(comm.value, Val::U8(0));
    assert_eq!(energy.unit.as_deref(), Some("kWh"));
    {
        let r = regs.lock().unwrap();
        assert_eq!(r[10], 777, "dispatch 应经 on_write 写入从站");
        assert_eq!(&r[20..22], &[0x1DC0, 0xFFFE], "CDAB 编码 -123456");
    }
    println!("✅ Lua Modbus 连接/读/写/设备注册/下发 全部通过");

    shutdown.cancel();
    let _ = join.await;
    assert!(
        !data_center().has_downlink("lua_dev"),
        "引擎关闭后应注销设备下行通道"
    );
    println!("✅ 引擎关闭后设备已注销");
    let _ = tokio::fs::remove_dir_all(&dir).await;
}
