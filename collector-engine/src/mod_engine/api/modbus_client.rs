use std::{net::SocketAddr, sync::Arc, time::Duration};

use mlua::{Lua, Table, UserData, UserDataMethods, Value};
use tokio::{sync::Mutex, time};
use tokio_modbus::{
    Slave,
    client::{Context, Reader, Writer, rtu, tcp},
    prelude::SlaveContext,
};
use tokio_serial::{DataBits, Parity, StopBits};

/// 链路参数，断线后据此惰性重连
#[derive(Clone)]
enum Link {
    Tcp(SocketAddr),
    Rtu {
        tty: String,
        baud: u32,
        data_bits: u8,
        parity: String,
        stop_bits: u8,
    },
}

enum Op {
    ReadCoils(u16, u16),
    ReadDiscrete(u16, u16),
    ReadHolding(u16, u16),
    ReadInput(u16, u16),
    WriteCoil(u16, bool),
    WriteCoils(u16, Vec<bool>),
    WriteRegister(u16, u16),
    WriteRegisters(u16, Vec<u16>),
}

enum OpOutput {
    Bits(Vec<bool>),
    Words(Vec<u16>),
    Done,
}

struct Inner {
    link: Link,
    slave: u8,
    timeout: Duration,
    ctx: Option<Context>,
}

async fn connect(link: &Link, slave: u8, timeout: Duration) -> Result<Context, String> {
    {
        match link {
            Link::Tcp(addr) => {
                let ctx = time::timeout(timeout, tcp::connect(*addr))
                    .await
                    .map_err(|_| "连接超时".to_string())?
                    .map_err(|e| e.to_string())?;
                Ok(ctx)
            }
            Link::Rtu {
                tty,
                baud,
                data_bits,
                parity,
                stop_bits,
            } => {
                let builder = tokio_serial::new(tty.as_str(), *baud)
                    .data_bits(match data_bits {
                        5 => DataBits::Five,
                        6 => DataBits::Six,
                        7 => DataBits::Seven,
                        _ => DataBits::Eight,
                    })
                    .parity(match parity.to_ascii_uppercase().as_str() {
                        "E" | "EVEN" => Parity::Even,
                        "O" | "ODD" => Parity::Odd,
                        _ => Parity::None,
                    })
                    .stop_bits(match stop_bits {
                        2 => StopBits::Two,
                        _ => StopBits::One,
                    })
                    .timeout(timeout);
                let port = tokio_serial::SerialStream::open(&builder)
                    .map_err(|e| format!("打开串口 {tty} 失败: {e}"))?;
                Ok(rtu::attach_slave(port, Slave(slave)))
            }
        }
    }
}

impl Inner {
    /// 执行一次请求。传输层错误/超时会丢弃连接，下次调用自动重连；
    /// 从站返回的 Modbus 异常码不影响连接。
    async fn exec(&mut self, op: Op, slave: Option<u8>) -> Result<OpOutput, String> {
        if self.ctx.is_none() {
            self.ctx = Some(connect(&self.link, self.slave, self.timeout).await?);
        }
        let timeout = self.timeout;
        let ctx = self.ctx.as_mut().expect("ctx 刚刚已建立");
        ctx.set_slave(Slave(slave.unwrap_or(self.slave)));

        let fut = async {
            match op {
                Op::ReadCoils(a, n) => ctx.read_coils(a, n).await.map(|r| r.map(OpOutput::Bits)),
                Op::ReadDiscrete(a, n) => ctx
                    .read_discrete_inputs(a, n)
                    .await
                    .map(|r| r.map(OpOutput::Bits)),
                Op::ReadHolding(a, n) => ctx
                    .read_holding_registers(a, n)
                    .await
                    .map(|r| r.map(OpOutput::Words)),
                Op::ReadInput(a, n) => ctx
                    .read_input_registers(a, n)
                    .await
                    .map(|r| r.map(OpOutput::Words)),
                Op::WriteCoil(a, v) => ctx
                    .write_single_coil(a, v)
                    .await
                    .map(|r| r.map(|_| OpOutput::Done)),
                Op::WriteCoils(a, v) => ctx
                    .write_multiple_coils(a, &v)
                    .await
                    .map(|r| r.map(|_| OpOutput::Done)),
                Op::WriteRegister(a, v) => ctx
                    .write_single_register(a, v)
                    .await
                    .map(|r| r.map(|_| OpOutput::Done)),
                Op::WriteRegisters(a, v) => ctx
                    .write_multiple_registers(a, &v)
                    .await
                    .map(|r| r.map(|_| OpOutput::Done)),
            }
        };

        match time::timeout(timeout, fut).await {
            Err(_) => {
                self.ctx = None;
                Err("请求超时".to_string())
            }
            Ok(Err(e)) => {
                self.ctx = None;
                Err(e.to_string())
            }
            Ok(Ok(Err(exc))) => Err(format!("从站异常: {exc:?}")),
            Ok(Ok(Ok(out))) => Ok(out),
        }
    }
}

/// 脚本持有的 Modbus 连接句柄，作为 UserData 返回给 Lua。
/// 同一连接上的请求通过 Mutex 串行化。
#[derive(Clone)]
pub struct ModbusHandle(Arc<Mutex<Inner>>);

fn to_u16(n: i64) -> mlua::Result<u16> {
    if (-32768..=65535).contains(&n) {
        Ok(n as i32 as u16)
    } else {
        Err(mlua::Error::runtime(format!("寄存器值 {n} 超出 16 位范围")))
    }
}

fn check_count(addr: i64, count: i64, max: i64) -> mlua::Result<(u16, u16)> {
    if !(0..=65535).contains(&addr) {
        return Err(mlua::Error::runtime(format!("地址 {addr} 超出范围")));
    }
    if !(1..=max).contains(&count) {
        return Err(mlua::Error::runtime(format!("数量 {count} 必须在 1~{max}")));
    }
    Ok((addr as u16, count as u16))
}

fn output_to_lua(lua: &Lua, out: OpOutput) -> mlua::Result<Value> {
    match out {
        OpOutput::Done => Ok(Value::Boolean(true)),
        OpOutput::Bits(v) => Ok(Value::Table(lua.create_sequence_from(v)?)),
        OpOutput::Words(v) => Ok(Value::Table(lua.create_sequence_from(v)?)),
    }
}

impl ModbusHandle {
    async fn run(
        &self,
        lua: &Lua,
        op: Op,
        slave: Option<u8>,
    ) -> mlua::Result<(Value, Option<String>)> {
        let result = self.0.lock().await.exec(op, slave).await;
        match result {
            Ok(out) => Ok((output_to_lua(lua, out)?, None)),
            Err(e) => Ok((Value::Nil, Some(e))),
        }
    }
}

impl UserData for ModbusHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // 读：返回 table；失败返回 nil, err。最后一个可选参数 slave 用于同一链路上访问其他从站
        methods.add_async_method(
            "read_coils",
            |lua, this, (addr, count, slave): (i64, i64, Option<u8>)| async move {
                let (a, n) = check_count(addr, count, 2000)?;
                this.run(&lua, Op::ReadCoils(a, n), slave).await
            },
        );
        methods.add_async_method(
            "read_discrete",
            |lua, this, (addr, count, slave): (i64, i64, Option<u8>)| async move {
                let (a, n) = check_count(addr, count, 2000)?;
                this.run(&lua, Op::ReadDiscrete(a, n), slave).await
            },
        );
        methods.add_async_method(
            "read_holding",
            |lua, this, (addr, count, slave): (i64, i64, Option<u8>)| async move {
                let (a, n) = check_count(addr, count, 125)?;
                this.run(&lua, Op::ReadHolding(a, n), slave).await
            },
        );
        methods.add_async_method(
            "read_input",
            |lua, this, (addr, count, slave): (i64, i64, Option<u8>)| async move {
                let (a, n) = check_count(addr, count, 125)?;
                this.run(&lua, Op::ReadInput(a, n), slave).await
            },
        );

        // 写：成功返回 true；失败返回 nil, err
        methods.add_async_method(
            "write_coil",
            |lua, this, (addr, value, slave): (i64, bool, Option<u8>)| async move {
                let (a, _) = check_count(addr, 1, 1)?;
                this.run(&lua, Op::WriteCoil(a, value), slave).await
            },
        );
        methods.add_async_method(
            "write_coils",
            |lua, this, (addr, values, slave): (i64, Vec<bool>, Option<u8>)| async move {
                let (a, _) = check_count(addr, values.len().max(1) as i64, 1968)?;
                this.run(&lua, Op::WriteCoils(a, values), slave).await
            },
        );
        methods.add_async_method(
            "write_register",
            |lua, this, (addr, value, slave): (i64, i64, Option<u8>)| async move {
                let (a, _) = check_count(addr, 1, 1)?;
                this.run(&lua, Op::WriteRegister(a, to_u16(value)?), slave)
                    .await
            },
        );
        methods.add_async_method(
            "write_registers",
            |lua, this, (addr, values, slave): (i64, Vec<i64>, Option<u8>)| async move {
                let (a, _) = check_count(addr, values.len().max(1) as i64, 123)?;
                let words = values
                    .into_iter()
                    .map(to_u16)
                    .collect::<Result<Vec<_>, _>>()?;
                this.run(&lua, Op::WriteRegisters(a, words), slave).await
            },
        );

        methods.add_async_method("is_connected", |_, this, ()| async move {
            Ok(this.0.lock().await.ctx.is_some())
        });

        // 主动断开；之后再调用读写方法会自动重连
        methods.add_async_method("close", |_, this, ()| async move {
            this.0.lock().await.ctx = None;
            Ok(())
        });
    }
}

// ── 寄存器编解码 ────────────────────────────────────────────────────────────

fn assemble_u32(w: [u16; 2], order: &str) -> mlua::Result<u32> {
    let [w0, w1] = w;
    Ok(match order {
        "ABCD" => ((w0 as u32) << 16) | w1 as u32,
        "CDAB" => ((w1 as u32) << 16) | w0 as u32,
        "BADC" => ((w0.swap_bytes() as u32) << 16) | w1.swap_bytes() as u32,
        "DCBA" => ((w1.swap_bytes() as u32) << 16) | w0.swap_bytes() as u32,
        other => return Err(mlua::Error::runtime(format!("不支持的字节序 {other}"))),
    })
}

fn split_u32(v: u32, order: &str) -> mlua::Result<[u16; 2]> {
    let (hi, lo) = ((v >> 16) as u16, v as u16);
    Ok(match order {
        "ABCD" => [hi, lo],
        "CDAB" => [lo, hi],
        "BADC" => [hi.swap_bytes(), lo.swap_bytes()],
        "DCBA" => [lo.swap_bytes(), hi.swap_bytes()],
        other => return Err(mlua::Error::runtime(format!("不支持的字节序 {other}"))),
    })
}

/// modbus.decode(regs, type, order?, index?) -> number
/// type: U16/I16/U32/I32/F32；order: 16 位用 AB/BA，32 位用 ABCD/CDAB/BADC/DCBA；index 为 1 基起始下标
fn decode(
    regs: Vec<i64>,
    ty: String,
    order: Option<String>,
    index: Option<usize>,
) -> mlua::Result<f64> {
    let start = index.unwrap_or(1).max(1) - 1;
    let need = if matches!(ty.as_str(), "U16" | "I16") {
        1
    } else {
        2
    };
    let words: Vec<u16> = regs
        .get(start..start + need)
        .ok_or_else(|| mlua::Error::runtime("寄存器数量不足"))?
        .iter()
        .map(|v| to_u16(*v))
        .collect::<Result<_, _>>()?;
    let order = order.unwrap_or_else(|| if need == 1 { "AB" } else { "ABCD" }.to_string());
    match ty.as_str() {
        "U16" | "I16" => {
            let w = if order == "BA" {
                words[0].swap_bytes()
            } else {
                words[0]
            };
            Ok(if ty == "U16" {
                w as f64
            } else {
                w as i16 as f64
            })
        }
        "U32" | "I32" | "F32" => {
            let v = assemble_u32([words[0], words[1]], &order)?;
            Ok(match ty.as_str() {
                "U32" => v as f64,
                "I32" => v as i32 as f64,
                _ => f32::from_bits(v) as f64,
            })
        }
        other => Err(mlua::Error::runtime(format!("不支持的数据类型 {other}"))),
    }
}

/// modbus.encode(value, type, order?) -> regs
fn encode(value: f64, ty: String, order: Option<String>) -> mlua::Result<Vec<u16>> {
    let range_err = || mlua::Error::runtime(format!("值 {value} 超出 {ty} 范围"));
    match ty.as_str() {
        "U16" | "I16" => {
            let ok = if ty == "U16" {
                (0.0..=65535.0).contains(&value)
            } else {
                (-32768.0..=32767.0).contains(&value)
            };
            if !ok {
                return Err(range_err());
            }
            let w = if ty == "U16" {
                value as u16
            } else {
                value as i16 as u16
            };
            let w = if order.as_deref() == Some("BA") {
                w.swap_bytes()
            } else {
                w
            };
            Ok(vec![w])
        }
        "U32" | "I32" | "F32" => {
            let raw = match ty.as_str() {
                "U32" if (0.0..=u32::MAX as f64).contains(&value) => value as u32,
                "I32" if (i32::MIN as f64..=i32::MAX as f64).contains(&value) => {
                    value as i32 as u32
                }
                "F32" => (value as f32).to_bits(),
                _ => return Err(range_err()),
            };
            Ok(split_u32(raw, order.as_deref().unwrap_or("ABCD"))?.to_vec())
        }
        other => Err(mlua::Error::runtime(format!("不支持的数据类型 {other}"))),
    }
}

fn opt<T: mlua::FromLua>(opts: &Table, key: &str) -> Option<T> {
    opts.get::<Option<T>>(key).ok().flatten()
}

fn parse_link(opts: &Table) -> Result<Link, String> {
    let kind: String = opt(opts, "type").unwrap_or_else(|| "tcp".to_string());
    match kind.to_ascii_lowercase().as_str() {
        "tcp" => {
            let host: String = opt(opts, "host").ok_or("缺少 host")?;
            let port: u16 = opt(opts, "port").unwrap_or(502);
            let addr = format!("{host}:{port}")
                .parse()
                .map_err(|e| format!("地址解析失败: {e}"))?;
            Ok(Link::Tcp(addr))
        }
        "rtu" => Ok(Link::Rtu {
            tty: opt(opts, "serial").ok_or("缺少 serial（串口设备路径）")?,
            baud: opt(opts, "baud").unwrap_or(9600),
            data_bits: opt(opts, "data_bits").unwrap_or(8),
            parity: opt(opts, "parity").unwrap_or_else(|| "N".to_string()),
            stop_bits: opt(opts, "stop_bits").unwrap_or(1),
        }),
        other => Err(format!("不支持的 type: {other}")),
    }
}

/// 创建 `modbus` 全局表：脚本通过 `modbus.connect` 自行建立 Modbus TCP/RTU 连接。
/// 连接句柄随 Lua VM 销毁而释放（串口/套接字随之关闭），无需引擎额外登记。
pub fn create_modbus_table(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "connect",
        lua.create_async_function(|lua, opts: Table| async move {
            let link = match parse_link(&opts) {
                Ok(l) => l,
                Err(e) => return Ok((Value::Nil, Some(e))),
            };
            let mut inner = Inner {
                link,
                slave: opt(&opts, "slave").unwrap_or(1),
                timeout: Duration::from_millis(opt(&opts, "timeout").unwrap_or(1000)),
                ctx: None,
            };
            match connect(&inner.link, inner.slave, inner.timeout).await {
                Ok(ctx) => inner.ctx = Some(ctx),
                Err(e) => return Ok((Value::Nil, Some(e))),
            }
            let ud = lua.create_userdata(ModbusHandle(Arc::new(Mutex::new(inner))))?;
            Ok((Value::UserData(ud), None))
        })?,
    )?;

    table.set(
        "decode",
        lua.create_function(
            |_, (regs, ty, order, index): (Vec<i64>, String, Option<String>, Option<usize>)| {
                decode(regs, ty, order, index)
            },
        )?,
    )?;
    table.set(
        "encode",
        lua.create_function(|_, (value, ty, order): (f64, String, Option<String>)| {
            encode(value, ty, order)
        })?,
    )?;

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_encode_roundtrip() {
        for order in ["ABCD", "CDAB", "BADC", "DCBA"] {
            let regs = encode(-123456.0, "I32".into(), Some(order.into())).unwrap();
            let back = decode(
                regs.iter().map(|v| *v as i64).collect(),
                "I32".into(),
                Some(order.into()),
                None,
            )
            .unwrap();
            assert_eq!(back, -123456.0, "{order}");
        }
        let regs = encode(12.5, "F32".into(), None).unwrap();
        assert_eq!(regs, vec![0x4148, 0x0000]);
    }

    #[test]
    fn cdab_matches_existing_modbus_dev_semantics() {
        // 0x00010002 在 CDAB 下寄存器顺序为 [0x0002, 0x0001]
        assert_eq!(split_u32(0x0001_0002, "CDAB").unwrap(), [0x0002, 0x0001]);
        assert_eq!(
            decode(
                vec![0x0002, 0x0001],
                "U32".into(),
                Some("CDAB".into()),
                None
            )
            .unwrap(),
            65538.0
        );
    }

    #[test]
    fn signed_u16_inputs() {
        assert_eq!(to_u16(-1).unwrap(), 0xFFFF);
        assert!(to_u16(70000).is_err());
    }
}
