use crate::ffi;

/// 双点状态值，遥信读取和双点遥控命令共用同一套编码 (0=中间态/不允许, 1=分, 2=合, 3=不确定/不允许)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoublePointValue {
    Intermediate,
    Off,
    On,
    Indeterminate,
}

impl DoublePointValue {
    /// 将底层 FFI 常量转换为对应枚举值，未识别的值归为 `Intermediate`。
    fn from_raw(raw: ffi::DoublePointValue) -> Self {
        match raw {
            ffi::DoublePointValue_IEC60870_DOUBLE_POINT_OFF => Self::Off,
            ffi::DoublePointValue_IEC60870_DOUBLE_POINT_ON => Self::On,
            ffi::DoublePointValue_IEC60870_DOUBLE_POINT_INDETERMINATE => Self::Indeterminate,
            _ => Self::Intermediate,
        }
    }

    /// 转换为 `DoubleCommand_create` 所需的原始整数编码。
    pub(crate) fn to_raw(self) -> std::os::raw::c_int {
        match self {
            Self::Intermediate => 0,
            Self::Off => 1,
            Self::On => 2,
            Self::Indeterminate => 3,
        }
    }
}

/// 步位置命令的方向，仅用于遥调命令下发 (0/3=不允许, 1=降, 2=升)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepCommandValue {
    Invalid0,
    Lower,
    Higher,
    Invalid3,
}

impl StepCommandValue {
    /// 转换为 `StepCommand_create` 所需的原始枚举值。
    pub(crate) fn to_raw(self) -> ffi::StepCommandValue {
        match self {
            Self::Invalid0 => ffi::StepCommandValue_IEC60870_STEP_INVALID_0,
            Self::Lower => ffi::StepCommandValue_IEC60870_STEP_LOWER,
            Self::Higher => ffi::StepCommandValue_IEC60870_STEP_HIGHER,
            Self::Invalid3 => ffi::StepCommandValue_IEC60870_STEP_INVALID_3,
        }
    }
}

/// 信息对象的值，覆盖常见的遥信/遥测监视类型；不支持的类型只保留 type_id。
#[derive(Debug, Clone, Copy)]
pub enum InformationObjectValue {
    SinglePoint {
        value: bool,
        quality: u8,
    },
    DoublePoint {
        value: DoublePointValue,
        quality: u8,
    },
    MeasuredNormalized {
        value: f32,
        quality: u8,
    },
    MeasuredScaled {
        value: i32,
        quality: u8,
    },
    MeasuredShort {
        value: f32,
        quality: u8,
    },
    Unsupported {
        type_id: u32,
    },
}

/// 单个信息对象的解析结果。
#[derive(Debug, Clone, Copy)]
pub struct InformationObjectData {
    /// 信息对象地址（Information Object Address）。
    pub ioa: i32,
    /// 带 CP56Time2a 时标的类型才有值（Unix 毫秒时间戳），否则为 `None`。
    pub timestamp_ms: Option<u64>,
    pub value: InformationObjectValue,
}

/// 一个 ASDU（应用服务数据单元）的解析结果，包含头部字段和其中的所有信息对象。
#[derive(Debug, Clone)]
pub struct Asdu {
    pub type_id: u32,
    /// 传输原因（Cause Of Transmission），如总召唤确认、突发上传等。
    pub cot: u32,
    /// 公共地址（Common Address），标识所属站/扇区。
    pub ca: i32,
    /// 原发地址（Originator Address）。
    pub oa: i32,
    pub is_test: bool,
    /// 是否为否定确认（例如命令被拒绝时的 ACTIVATION_CON）。
    pub is_negative: bool,
    pub elements: Vec<InformationObjectData>,
}

impl Asdu {
    /// 从底层 `CS101_ASDU` 拷贝出安全的数据表示。
    ///
    /// `raw` 及其中的 `InformationObject` 只在回调上下文中有效，本函数会在读取每个
    /// 元素后立即 `InformationObject_destroy`，不会保留任何裸指针。
    pub(crate) unsafe fn from_raw(raw: ffi::CS101_ASDU) -> Self {
        unsafe {
            let type_id = ffi::CS101_ASDU_getTypeID(raw);
            let count = ffi::CS101_ASDU_getNumberOfElements(raw).max(0);
            let mut elements = Vec::with_capacity(count as usize);

            for index in 0..count {
                let io = ffi::CS101_ASDU_getElement(raw, index);
                if io.is_null() {
                    continue;
                }

                let ioa = ffi::InformationObject_getObjectAddress(io);
                let (value, timestamp_ms) = decode_element(type_id, io);
                ffi::InformationObject_destroy(io);

                elements.push(InformationObjectData {
                    ioa,
                    timestamp_ms,
                    value,
                });
            }

            Self {
                type_id,
                cot: ffi::CS101_ASDU_getCOT(raw),
                ca: ffi::CS101_ASDU_getCA(raw),
                oa: ffi::CS101_ASDU_getOA(raw),
                is_test: ffi::CS101_ASDU_isTest(raw),
                is_negative: ffi::CS101_ASDU_isNegative(raw),
                elements,
            }
        }
    }
}

/// 按 `type_id` 将裸指针强转为具体的 `InformationObject` 子类型并拷贝出值。
/// 不认识的 `type_id` 归为 `Unsupported`，不会 panic。
unsafe fn decode_element(
    type_id: u32,
    io: ffi::InformationObject,
) -> (InformationObjectValue, Option<u64>) {
    unsafe {
        match type_id {
            ffi::IEC60870_5_TypeID_M_SP_NA_1 => {
                let sp = io as ffi::SinglePointInformation;
                (
                    InformationObjectValue::SinglePoint {
                        value: ffi::SinglePointInformation_getValue(sp),
                        quality: ffi::SinglePointInformation_getQuality(sp),
                    },
                    None,
                )
            }
            ffi::IEC60870_5_TypeID_M_SP_TB_1 => {
                let sp = io as ffi::SinglePointInformation;
                let ts = ffi::SinglePointWithCP56Time2a_getTimestamp(
                    io as ffi::SinglePointWithCP56Time2a,
                );
                (
                    InformationObjectValue::SinglePoint {
                        value: ffi::SinglePointInformation_getValue(sp),
                        quality: ffi::SinglePointInformation_getQuality(sp),
                    },
                    Some(ffi::CP56Time2a_toMsTimestamp(ts)),
                )
            }
            ffi::IEC60870_5_TypeID_M_DP_NA_1 => {
                let dp = io as ffi::DoublePointInformation;
                (
                    InformationObjectValue::DoublePoint {
                        value: DoublePointValue::from_raw(ffi::DoublePointInformation_getValue(dp)),
                        quality: ffi::DoublePointInformation_getQuality(dp),
                    },
                    None,
                )
            }
            ffi::IEC60870_5_TypeID_M_DP_TB_1 => {
                let dp = io as ffi::DoublePointInformation;
                let ts = ffi::DoublePointWithCP56Time2a_getTimestamp(
                    io as ffi::DoublePointWithCP56Time2a,
                );
                (
                    InformationObjectValue::DoublePoint {
                        value: DoublePointValue::from_raw(ffi::DoublePointInformation_getValue(dp)),
                        quality: ffi::DoublePointInformation_getQuality(dp),
                    },
                    Some(ffi::CP56Time2a_toMsTimestamp(ts)),
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_NA_1 => {
                let mv = io as ffi::MeasuredValueNormalized;
                (
                    InformationObjectValue::MeasuredNormalized {
                        value: ffi::MeasuredValueNormalized_getValue(mv),
                        quality: ffi::MeasuredValueNormalized_getQuality(mv),
                    },
                    None,
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_TD_1 => {
                let mv = io as ffi::MeasuredValueNormalized;
                let ts = ffi::MeasuredValueNormalizedWithCP56Time2a_getTimestamp(
                    io as ffi::MeasuredValueNormalizedWithCP56Time2a,
                );
                (
                    InformationObjectValue::MeasuredNormalized {
                        value: ffi::MeasuredValueNormalized_getValue(mv),
                        quality: ffi::MeasuredValueNormalized_getQuality(mv),
                    },
                    Some(ffi::CP56Time2a_toMsTimestamp(ts)),
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_NB_1 => {
                let mv = io as ffi::MeasuredValueScaled;
                (
                    InformationObjectValue::MeasuredScaled {
                        value: ffi::MeasuredValueScaled_getValue(mv),
                        quality: ffi::MeasuredValueScaled_getQuality(mv),
                    },
                    None,
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_TE_1 => {
                let mv = io as ffi::MeasuredValueScaled;
                let ts = ffi::MeasuredValueScaledWithCP56Time2a_getTimestamp(
                    io as ffi::MeasuredValueScaledWithCP56Time2a,
                );
                (
                    InformationObjectValue::MeasuredScaled {
                        value: ffi::MeasuredValueScaled_getValue(mv),
                        quality: ffi::MeasuredValueScaled_getQuality(mv),
                    },
                    Some(ffi::CP56Time2a_toMsTimestamp(ts)),
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_NC_1 => {
                let mv = io as ffi::MeasuredValueShort;
                (
                    InformationObjectValue::MeasuredShort {
                        value: ffi::MeasuredValueShort_getValue(mv),
                        quality: ffi::MeasuredValueShort_getQuality(mv),
                    },
                    None,
                )
            }
            ffi::IEC60870_5_TypeID_M_ME_TF_1 => {
                let mv = io as ffi::MeasuredValueShort;
                let ts = ffi::MeasuredValueShortWithCP56Time2a_getTimestamp(
                    io as ffi::MeasuredValueShortWithCP56Time2a,
                );
                (
                    InformationObjectValue::MeasuredShort {
                        value: ffi::MeasuredValueShort_getValue(mv),
                        quality: ffi::MeasuredValueShort_getQuality(mv),
                    },
                    Some(ffi::CP56Time2a_toMsTimestamp(ts)),
                )
            }
            _ => (InformationObjectValue::Unsupported { type_id }, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn default_app_layer_parameters() -> ffi::sCS101_AppLayerParameters {
        ffi::sCS101_AppLayerParameters {
            sizeOfTypeId: 1,
            sizeOfVSQ: 1,
            sizeOfCOT: 2,
            originatorAddress: 0,
            sizeOfCA: 2,
            sizeOfIOA: 3,
            maxSizeOfASDU: 249,
        }
    }

    #[test]
    fn parses_single_point_information() {
        unsafe {
            let mut params = default_app_layer_parameters();
            let raw = ffi::CS101_ASDU_create(
                &mut params,
                false,
                ffi::CS101_CauseOfTransmission_CS101_COT_SPONTANEOUS,
                0,
                1,
                false,
                false,
            );
            assert!(!raw.is_null());

            let io = ffi::SinglePointInformation_create(
                ptr::null_mut(),
                100,
                true,
                ffi::IEC60870_QUALITY_GOOD as u8,
            );
            assert!(ffi::CS101_ASDU_addInformationObject(
                raw,
                io as ffi::InformationObject
            ));

            let asdu = Asdu::from_raw(raw);
            ffi::CS101_ASDU_destroy(raw);

            assert_eq!(asdu.type_id, ffi::IEC60870_5_TypeID_M_SP_NA_1);
            assert_eq!(asdu.elements.len(), 1);
            let element = &asdu.elements[0];
            assert_eq!(element.ioa, 100);
            assert_eq!(element.timestamp_ms, None);
            match element.value {
                InformationObjectValue::SinglePoint { value, quality } => {
                    assert!(value);
                    assert_eq!(quality, ffi::IEC60870_QUALITY_GOOD as u8);
                }
                other => panic!("unexpected value: {other:?}"),
            }
        }
    }

    #[test]
    fn parses_measured_scaled_with_timestamp() {
        unsafe {
            let mut params = default_app_layer_parameters();
            let raw = ffi::CS101_ASDU_create(
                &mut params,
                false,
                ffi::CS101_CauseOfTransmission_CS101_COT_SPONTANEOUS,
                0,
                1,
                false,
                false,
            );
            assert!(!raw.is_null());

            let mut time = ffi::sCP56Time2a {
                encodedValue: [0u8; 7],
            };
            ffi::CP56Time2a_createFromMsTimestamp(&mut time, 1_700_000_000_000);

            let io = ffi::MeasuredValueScaledWithCP56Time2a_create(
                ptr::null_mut(),
                200,
                1234,
                ffi::IEC60870_QUALITY_GOOD as u8,
                &mut time,
            );
            assert!(ffi::CS101_ASDU_addInformationObject(
                raw,
                io as ffi::InformationObject
            ));

            let asdu = Asdu::from_raw(raw);
            ffi::CS101_ASDU_destroy(raw);

            assert_eq!(asdu.type_id, ffi::IEC60870_5_TypeID_M_ME_TE_1);
            assert_eq!(asdu.elements.len(), 1);
            let element = &asdu.elements[0];
            assert_eq!(element.ioa, 200);
            assert_eq!(element.timestamp_ms, Some(1_700_000_000_000));
            match element.value {
                InformationObjectValue::MeasuredScaled { value, quality } => {
                    assert_eq!(value, 1234);
                    assert_eq!(quality, ffi::IEC60870_QUALITY_GOOD as u8);
                }
                other => panic!("unexpected value: {other:?}"),
            }
        }
    }
}
