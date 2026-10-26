use std::{sync::Arc, time::Duration};

use collector_dlt645::{
    Client, ClientConfig, DataIdentifier, DiSpec, MeterAddress, MeterError, decode_bcd_le,
    decode_signed_bcd_le, spec_for,
};
use mlua::{Lua, Table, UserData, UserDataMethods, Value};
use tokio::sync::Mutex;
use tokio_serial::{DataBits, Parity, SerialStream, StopBits};

/// 与 dlt645_dev 保持一致的唤醒前导字节数
const WAKEUP_BYTES: usize = 4;

/// 串口参数，断线后据此惰性重新打开
#[derive(Clone)]
struct Link {
    tty: String,
    baud: u32,
    data_bits: u8,
    parity: String,
    stop_bits: u8,
}

struct Inner {
    link: Link,
    /// 默认表地址，单次调用可覆盖
    address: Option<MeterAddress>,
    timeout: Duration,
    client: Option<Client<SerialStream>>,
}

fn open(link: &Link, timeout: Duration) -> Result<Client<SerialStream>, String> {
    let builder = tokio_serial::new(link.tty.as_str(), link.baud)
        .data_bits(match link.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })
        .parity(match link.parity.to_ascii_uppercase().as_str() {
            "E" | "EVEN" => Parity::Even,
            "O" | "ODD" => Parity::Odd,
            _ => Parity::None,
        })
        .stop_bits(match link.stop_bits {
            2 => StopBits::Two,
            _ => StopBits::One,
        });
    let port = SerialStream::open(&builder)
        .map_err(|e| format!("打开串口 {} 失败: {e}", link.tty))?;
    Ok(Client::new(
        port,
        ClientConfig {
            response_timeout: timeout,
            wakeup_bytes: WAKEUP_BYTES,
        },
    ))
}

impl Inner {
    fn resolve_address(&self, addr: Option<String>) -> Result<MeterAddress, String> {
        match addr {
            Some(s) => MeterAddress::parse(&s).map_err(|e| format!("表地址无效: {e}")),
            None => self.address.ok_or_else(|| "缺少表地址 address".to_string()),
        }
    }

    /// 读取一个 DI 的原始数据域（不含 4 字节 DI）。
    /// 串口 I/O 错误会丢弃连接，下次调用自动重开；超时、表计异常码、地址/标识不匹配
    /// 属于单次事务失败，不影响串口（迟到的响应由 Client 在下次请求前丢弃）。
    async fn read_data(
        &mut self,
        di: DataIdentifier,
        addr: Option<String>,
    ) -> Result<Vec<u8>, String> {
        let address = self.resolve_address(addr)?;
        if self.client.is_none() {
            self.client = Some(open(&self.link, self.timeout)?);
        }
        let client = self.client.as_mut().expect("client 刚刚已建立");
        match client.read_data(address, di).await {
            Ok(data) => Ok(data),
            Err(e @ MeterError::Io(_)) => {
                self.client = None;
                Err(e.to_string())
            }
            Err(MeterError::MeterException(code)) => Err(format!("电表异常: 0x{code:02X}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 脚本持有的 DL/T645 连接句柄，作为 UserData 返回给 Lua。
/// 同一串口上的请求通过 Mutex 串行化。
#[derive(Clone)]
pub struct Dlt645Handle(Arc<Mutex<Inner>>);

/// DI 接受整数（如 0x02010100）或 8 位十六进制字符串（如 "02010100"）
fn parse_di(v: Value) -> mlua::Result<DataIdentifier> {
    match v {
        Value::Integer(n) if (0..=u32::MAX as i64).contains(&n) => {
            Ok(DataIdentifier::from_u32(n as u32))
        }
        Value::String(s) => {
            let s = s.to_str()?;
            let s = s.trim_start_matches("0x").trim_start_matches("0X");
            u32::from_str_radix(s, 16)
                .map(DataIdentifier::from_u32)
                .map_err(|_| mlua::Error::runtime(format!("数据标识 {s} 不是有效的十六进制")))
        }
        _ => Err(mlua::Error::runtime(
            "数据标识 di 必须是 0~0xFFFFFFFF 的整数或 8 位十六进制字符串",
        )),
    }
}

fn spec_table(lua: &Lua, spec: &DiSpec) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("len", spec.len)?;
    t.set("decimals", spec.decimals)?;
    t.set("signed", spec.signed)?;
    t.set("unit", spec.unit)?;
    Ok(t)
}

/// 自定义格式 `{ len=, decimals=, signed= }`
fn parse_spec(di: DataIdentifier, t: &Table) -> mlua::Result<DiSpec> {
    let len: usize = t
        .get::<Option<usize>>("len")?
        .ok_or_else(|| mlua::Error::runtime("自定义格式缺少 len"))?;
    let decimals: u8 = t.get::<Option<u8>>("decimals")?.unwrap_or(0);
    let signed: bool = t.get::<Option<bool>>("signed")?.unwrap_or(false);
    // unit 仅用于展示，脚本侧不使用
    Ok(DiSpec::new(di, len, decimals, signed, ""))
}

impl UserData for Dlt645Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // read(di, address?, spec?) -> number
        // 默认按内置 DI 表解码为工程值；未收录的 DI 需传 spec={len,decimals,signed}。
        // 失败返回 nil, err
        methods.add_async_method(
            "read",
            |_, this, (di, address, spec): (Value, Option<String>, Option<Table>)| async move {
                let di = parse_di(di)?;
                let spec = match spec {
                    Some(t) => parse_spec(di, &t)?,
                    None => match spec_for(di) {
                        Some(s) => s,
                        None => {
                            return Ok((
                                Value::Nil,
                                Some(format!(
                                    "DI {:08X} 未收录，请用 read_raw 或传入 spec",
                                    di.as_u32()
                                )),
                            ));
                        }
                    },
                };
                let data = match this.0.lock().await.read_data(di, address).await {
                    Ok(d) => d,
                    Err(e) => return Ok((Value::Nil, Some(e))),
                };
                match spec.decode(&data) {
                    Ok(v) => Ok((Value::Number(v), None)),
                    Err(e) => Ok((Value::Nil, Some(format!("解码失败: {e}")))),
                }
            },
        );

        // read_raw(di, address?) -> table：数据域原始字节（线上顺序，已去 0x33 偏移，不含 DI）
        methods.add_async_method(
            "read_raw",
            |lua, this, (di, address): (Value, Option<String>)| async move {
                let di = parse_di(di)?;
                match this.0.lock().await.read_data(di, address).await {
                    Ok(d) => Ok((Value::Table(lua.create_sequence_from(d)?), None)),
                    Err(e) => Ok((Value::Nil, Some(e))),
                }
            },
        );

        methods.add_async_method("is_connected", |_, this, ()| async move {
            Ok(this.0.lock().await.client.is_some())
        });

        // 主动关闭串口；之后再调用读方法会自动重开
        methods.add_async_method("close", |_, this, ()| async move {
            this.0.lock().await.client = None;
            Ok(())
        });
    }
}

fn opt<T: mlua::FromLua>(opts: &Table, key: &str) -> Option<T> {
    opts.get::<Option<T>>(key).ok().flatten()
}

fn parse_link(opts: &Table) -> Result<Link, String> {
    Ok(Link {
        tty: opt(opts, "serial").ok_or("缺少 serial（串口设备路径）")?,
        baud: opt(opts, "baud").unwrap_or(2400),
        data_bits: opt(opts, "data_bits").unwrap_or(8),
        parity: opt(opts, "parity").unwrap_or_else(|| "E".to_string()),
        stop_bits: opt(opts, "stop_bits").unwrap_or(1),
    })
}

/// 创建 `dlt645` 全局表：脚本通过 `dlt645.connect` 自行打开串口读取 DL/T645-2007 电表。
/// 句柄随 Lua VM 销毁而释放（串口随之关闭），无需引擎额外登记。
pub fn create_dlt645_table(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    // dlt645.connect{ serial=, baud=2400, data_bits=8, parity="E", stop_bits=1,
    //                 address="000000000001", timeout=1200 } -> handle | nil, err
    table.set(
        "connect",
        lua.create_async_function(|lua, opts: Table| async move {
            let link = match parse_link(&opts) {
                Ok(l) => l,
                Err(e) => return Ok((Value::Nil, Some(e))),
            };
            let address = match opt::<String>(&opts, "address") {
                Some(s) => match MeterAddress::parse(&s) {
                    Ok(a) => Some(a),
                    Err(e) => return Ok((Value::Nil, Some(format!("表地址无效: {e}")))),
                },
                None => None,
            };
            let timeout = Duration::from_millis(opt(&opts, "timeout").unwrap_or(1200));
            let client = match open(&link, timeout) {
                Ok(c) => c,
                Err(e) => return Ok((Value::Nil, Some(e))),
            };
            let inner = Inner {
                link,
                address,
                timeout,
                client: Some(client),
            };
            let ud = lua.create_userdata(Dlt645Handle(Arc::new(Mutex::new(inner))))?;
            Ok((Value::UserData(ud), None))
        })?,
    )?;

    // dlt645.spec(di) -> { len, decimals, signed, unit } | nil
    table.set(
        "spec",
        lua.create_function(|lua, di: Value| {
            let di = parse_di(di)?;
            match spec_for(di) {
                Some(s) => Ok(Value::Table(spec_table(lua, &s)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // dlt645.decode(bytes, decimals?, signed?) -> number：配合 read_raw 解码小端压缩 BCD
    table.set(
        "decode",
        lua.create_function(
            |_, (bytes, decimals, signed): (Vec<u8>, Option<u8>, Option<bool>)| {
                let raw = if signed.unwrap_or(false) {
                    decode_signed_bcd_le(&bytes).map_err(mlua::Error::external)?
                } else {
                    let v = decode_bcd_le(&bytes).map_err(mlua::Error::external)?;
                    i64::try_from(v).map_err(mlua::Error::external)?
                };
                Ok(raw as f64 / 10f64.powi(decimals.unwrap_or(0) as i32))
            },
        )?,
    )?;

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_matches_spec() {
        let lua = Lua::new();
        let t = create_dlt645_table(&lua).unwrap();
        let f: mlua::Function = t.get("decode").unwrap();
        // 12345.67 kWh
        let v: f64 = f.call((vec![0x67u8, 0x45, 0x23, 0x01], 2, false)).unwrap();
        assert_eq!(v, 12345.67);
        // -1.5 A（最高字节 bit7 为符号）
        let v: f64 = f.call((vec![0x00u8, 0x15, 0x80], 3, true)).unwrap();
        assert_eq!(v, -1.5);
    }

    #[test]
    fn di_accepts_int_and_hex_string() {
        let lua = Lua::new();
        let a = parse_di(Value::Integer(0x0201_0100)).unwrap();
        let s = lua.create_string("02010100").unwrap();
        let b = parse_di(Value::String(s)).unwrap();
        assert_eq!(a, b);
        assert!(parse_di(Value::Integer(-1)).is_err());
    }

    #[test]
    fn spec_lookup() {
        let lua = Lua::new();
        let t = create_dlt645_table(&lua).unwrap();
        let f: mlua::Function = t.get("spec").unwrap();
        let s: Table = f.call(0x0201_0100i64).unwrap();
        assert_eq!(s.get::<String>("unit").unwrap(), "V");
        assert_eq!(s.get::<u8>("decimals").unwrap(), 1);
        let none: Value = f.call(0x1234_5678i64).unwrap();
        assert!(none.is_nil());
    }
}
