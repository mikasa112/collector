use collector_iec61850::{Dbpos, Quality, Value};

use crate::config::iec61850_conf::{Iec61850Config, PointKind};
use crate::core::point::Val;

/// 品质 `q` 的输出编码: 按 IEC 61850-7-3 的位定义。
///
/// - bit0~1 有效性: `0` 良好, `1` 无效(invalid), `2` 保留, `3` 可疑(questionable)
/// - bit2~12 细节位: 溢出、超量程、坏引用、振荡、故障、旧数据、不一致、不精确、
///   被取代、测试、操作员闭锁
///
/// libiec61850 把有效性按位串的位序放进整数里(无效 = 2、保留 = 1), 与标准的枚举编号相反,
/// 这里对调回来; 细节位的位置两边一致。
pub(super) fn quality_code(q: Quality) -> u16 {
    let validity = match q.0 & 0x3 {
        1 => 2,
        2 => 1,
        v => v,
    };
    (q.0 & !0x3) | validity
}

/// MMS 的浮点绝大多数是 32 位, 转成 f64 会带出二进制误差(220.1 变成 220.10000610...)。
/// 能无损放进 f32 的值, 按 f32 的最短十进制表示还原。
fn clean_f64(v: f64) -> f64 {
    let narrow = v as f32;
    if f64::from(narrow) == v {
        narrow.to_string().parse().unwrap_or(v)
    } else {
        v
    }
}

/// 把读到的值转换为点值。点类型与值的类型不匹配、值超出范围或不是有限数时返回 `None`。
pub(super) fn decode_value(cfg: &Iec61850Config, value: &Value) -> Option<Val> {
    let measured = |raw: f64| {
        raw.is_finite()
            .then(|| Val::F64(clean_f64(raw) * cfg.scale + cfg.offset))
    };
    match (cfg.kind, value) {
        (PointKind::Yx, Value::Bool(b)) => Some(Val::U8(u8::from(*b))),
        // 编号与 104 的双点遥信一致
        (PointKind::Yx, Value::Dbpos(d)) => Some(Val::U8(match d {
            Dbpos::Intermediate => 0,
            Dbpos::Off => 1,
            Dbpos::On => 2,
            Dbpos::BadState => 3,
        })),
        (PointKind::Yx, Value::Int(i)) => u8::try_from(*i).ok().map(Val::U8),
        (PointKind::Yx, Value::Uint(u)) => u8::try_from(*u).ok().map(Val::U8),
        (PointKind::Yc, Value::Float(f)) => measured(*f),
        (PointKind::Yc, Value::Int(i)) => measured(*i as f64),
        (PointKind::Yc, Value::Uint(u)) => measured(*u as f64),
        (PointKind::Yq, Value::Quality(q)) => Some(Val::U16(quality_code(*q))),
        // 毫秒时间戳, 2^53 以内 f64 精确
        (PointKind::Ytime, Value::UtcTime(ms)) => Some(Val::F64(*ms as f64)),
        _ => None,
    }
}

/// IEC 61850-7-2 控制失败的 AddCause, 用于把错误码翻译成运维看得懂的文字。
pub(super) fn add_cause_text(code: u32) -> &'static str {
    match code {
        0 => "未知",
        1 => "不支持",
        2 => "被切换层级闭锁",
        3 => "选择失败",
        4 => "位置无效",
        5 => "已在目标位置",
        6 => "执行中参数改变",
        7 => "步数限制",
        8 => "被模式闭锁",
        9 => "被过程闭锁",
        10 => "被联锁闭锁",
        11 => "被同期检查闭锁",
        12 => "命令已在执行",
        13 => "被健康状态闭锁",
        14 => "1/N 控制",
        15 => "被取消中止",
        16 => "超过时间限制",
        17 => "被跳闸中止",
        18 => "对象未被选择",
        19 => "对象已被选择",
        20 => "无操作权限",
        21 => "超调结束",
        22 => "偏差中止",
        23 => "通信丢失中止",
        24 => "被命令中止",
        25 => "无",
        26 => "参数不一致",
        27 => "被其他客户端锁定",
        _ => "未定义",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::iec61850_conf::CtlOpts;
    use collector_iec61850::{Fc, ObjectRef};

    fn cfg(kind: PointKind, scale: f64, offset: f64) -> Iec61850Config {
        Iec61850Config {
            id: 1,
            key: "k",
            name: "n",
            reference: "L/N.A.b".parse::<ObjectRef>().unwrap().with_fc(Fc::St),
            kind,
            scale,
            offset,
            unit: None,
            enable: true,
            trans: None,
            status_words: None,
            warn_bits: None,
            level: None,
            ctl_opts: CtlOpts::default(),
            feedback: None,
        }
    }

    #[test]
    fn yx_from_bool_dbpos_and_integers() {
        let c = cfg(PointKind::Yx, 1.0, 0.0);
        assert_eq!(decode_value(&c, &Value::Bool(true)), Some(Val::U8(1)));
        assert_eq!(
            decode_value(&c, &Value::Dbpos(Dbpos::Off)),
            Some(Val::U8(1))
        );
        assert_eq!(decode_value(&c, &Value::Dbpos(Dbpos::On)), Some(Val::U8(2)));
        assert_eq!(
            decode_value(&c, &Value::Dbpos(Dbpos::BadState)),
            Some(Val::U8(3))
        );
        assert_eq!(decode_value(&c, &Value::Int(5)), Some(Val::U8(5)));
        assert_eq!(decode_value(&c, &Value::Uint(255)), Some(Val::U8(255)));
        // 超出 u8 的枚举值丢弃, 而不是截断成错误的状态
        assert_eq!(decode_value(&c, &Value::Int(256)), None);
        assert_eq!(decode_value(&c, &Value::Int(-1)), None);
    }

    #[test]
    fn yc_applies_scale_and_offset() {
        let c = cfg(PointKind::Yc, 0.1, 5.0);
        assert_eq!(decode_value(&c, &Value::Int(100)), Some(Val::F64(15.0)));
        assert_eq!(decode_value(&c, &Value::Uint(100)), Some(Val::F64(15.0)));
        assert_eq!(decode_value(&c, &Value::Float(100.0)), Some(Val::F64(15.0)));
    }

    #[test]
    fn short_float_has_no_binary_noise() {
        let c = cfg(PointKind::Yc, 1.0, 0.0);
        // libiec61850 把 32 位浮点转成 f64 后就是这个值
        let widened = f64::from(220.1_f32);
        assert_ne!(widened, 220.1);
        assert_eq!(
            decode_value(&c, &Value::Float(widened)),
            Some(Val::F64(220.1))
        );
        // 真正的 64 位值不被改动
        assert_eq!(
            decode_value(&c, &Value::Float(0.123_456_789_012_345_6)),
            Some(Val::F64(0.123_456_789_012_345_6))
        );
    }

    #[test]
    fn non_finite_and_mismatched_values_are_dropped() {
        let yc = cfg(PointKind::Yc, 1.0, 0.0);
        assert_eq!(decode_value(&yc, &Value::Float(f64::NAN)), None);
        assert_eq!(decode_value(&yc, &Value::Float(f64::INFINITY)), None);
        assert_eq!(decode_value(&yc, &Value::Bool(true)), None);
        assert_eq!(decode_value(&yc, &Value::Struct(vec![])), None);
        assert_eq!(decode_value(&yc, &Value::AccessError(10)), None);
        let yx = cfg(PointKind::Yx, 1.0, 0.0);
        assert_eq!(decode_value(&yx, &Value::Float(1.0)), None);
        assert_eq!(decode_value(&yx, &Value::Null), None);
    }

    #[test]
    fn quality_validity_is_normalized_to_the_standard_numbering() {
        // libiec61850: good=0 reserved=1 invalid=2 questionable=3
        assert_eq!(quality_code(Quality(0)), 0);
        assert_eq!(quality_code(Quality(2)), 1, "invalid -> 1");
        assert_eq!(quality_code(Quality(1)), 2, "reserved -> 2");
        assert_eq!(quality_code(Quality(3)), 3, "questionable -> 3");
        // 细节位原样保留: 溢出(bit2) + 旧数据(bit7) + 测试(bit11)
        let detail = 0b100 | (1 << 7) | (1 << 11);
        assert_eq!(quality_code(Quality(detail | 2)), detail | 1);
        let c = cfg(PointKind::Yq, 1.0, 0.0);
        assert_eq!(
            decode_value(&c, &Value::Quality(Quality(2))),
            Some(Val::U16(1))
        );
    }

    #[test]
    fn time_is_milliseconds_as_float() {
        let c = cfg(PointKind::Ytime, 1.0, 0.0);
        assert_eq!(
            decode_value(&c, &Value::UtcTime(1_792_131_767_264)),
            Some(Val::F64(1_792_131_767_264.0))
        );
    }

    #[test]
    fn add_cause_covers_the_whole_standard_range() {
        assert_eq!(add_cause_text(10), "被联锁闭锁");
        assert_eq!(add_cause_text(27), "被其他客户端锁定");
        assert!((0..=27).all(|c| add_cause_text(c) != "未定义"));
        assert_eq!(add_cause_text(99), "未定义");
    }
}
