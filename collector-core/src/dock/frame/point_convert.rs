#![allow(dead_code)]

use bytes::Bytes;

use crate::core::point::{DataPoint, DataPoints, Val};

use super::{DictEntrySpec, PointDomain, PointSpec, ValueType};

pub trait ValFrameExt {
    fn to_frame_type_and_bytes(self) -> (ValueType, Bytes);
}

impl ValFrameExt for Val {
    fn to_frame_type_and_bytes(self) -> (ValueType, Bytes) {
        match self {
            Val::U8(v) => (ValueType::U8, Bytes::copy_from_slice(&[v])),
            Val::I8(v) => (ValueType::I8, Bytes::copy_from_slice(&[v as u8])),
            Val::I16(v) => (ValueType::I16, Bytes::copy_from_slice(&v.to_be_bytes())),
            Val::I32(v) => (ValueType::I32, Bytes::copy_from_slice(&v.to_be_bytes())),
            Val::U16(v) => (ValueType::U16, Bytes::copy_from_slice(&v.to_be_bytes())),
            Val::U32(v) => (ValueType::U32, Bytes::copy_from_slice(&v.to_be_bytes())),
            Val::F32(v) => (ValueType::F32, Bytes::copy_from_slice(&v.to_be_bytes())),
        }
    }
}

pub trait DataPointFrameExt {
    fn to_frame_point_spec(self) -> PointSpec;
    fn to_frame_dict_entry_spec(self) -> DictEntrySpec;
}

impl DataPointFrameExt for DataPoint {
    fn to_frame_point_spec(self) -> PointSpec {
        let (value_type, value) = self.value.to_frame_type_and_bytes();
        PointSpec::new(self.id, domain_from_point_id(self.id), value_type, value)
    }

    fn to_frame_dict_entry_spec(self) -> DictEntrySpec {
        let (value_type, _) = self.value.to_frame_type_and_bytes();
        DictEntrySpec::new(self.id, value_type, self.name.as_bytes(), &b""[..])
    }
}

pub trait DataPointsFrameExt {
    fn to_frame_point_specs(&self) -> Vec<PointSpec>;
    fn to_frame_dict_entry_specs(&self) -> Vec<DictEntrySpec>;
}

impl DataPointsFrameExt for DataPoints {
    fn to_frame_point_specs(&self) -> Vec<PointSpec> {
        self.0
            .iter()
            .copied()
            .map(DataPoint::to_frame_point_spec)
            .collect()
    }

    fn to_frame_dict_entry_specs(&self) -> Vec<DictEntrySpec> {
        self.0
            .iter()
            .copied()
            .map(DataPoint::to_frame_dict_entry_spec)
            .collect()
    }
}

fn domain_from_point_id(point_id: u32) -> PointDomain {
    match (point_id >> 16) as u16 {
        1 => PointDomain::Yk,
        2 => PointDomain::Yx,
        3 => PointDomain::Yt,
        4 => PointDomain::Yc,
        _ => PointDomain::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn val_to_frame_type_and_bytes() {
        let (vt, raw) = Val::I16(-12).to_frame_type_and_bytes();
        assert_eq!(vt, ValueType::I16);
        assert_eq!(raw, (-12i16).to_be_bytes().to_vec());
    }

    #[test]
    fn data_point_to_frame_point_spec() {
        let point = DataPoint {
            id: 101,
            name: "p101",
            value: Val::U32(42),
        };
        let spec = point.to_frame_point_spec();
        assert_eq!(spec.point_id, 101);
        assert_eq!(spec.domain, PointDomain::Unknown);
        assert_eq!(spec.value_type, ValueType::U32);
        assert_eq!(spec.value, 42u32.to_be_bytes().to_vec());
    }

    #[test]
    fn data_point_to_frame_dict_entry_spec() {
        let point = DataPoint {
            id: 102,
            name: "p102",
            value: Val::F32(1.0),
        };
        let entry = point.to_frame_dict_entry_spec();
        assert_eq!(entry.point_id, 102);
        assert_eq!(entry.name, b"p102".to_vec());
        assert_eq!(entry.unit, b"".to_vec());
        assert_eq!(entry.value_type, ValueType::F32);
    }

    #[test]
    fn data_points_to_frame_point_specs() {
        let points = DataPoints(vec![
            DataPoint {
                id: 1,
                name: "p1",
                value: Val::U8(7),
            },
            DataPoint {
                id: 2,
                name: "p2",
                value: Val::F32(1.5),
            },
        ]);
        let specs = points.to_frame_point_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].point_id, 1);
        assert_eq!(specs[0].domain, PointDomain::Unknown);
        assert_eq!(specs[0].value_type, ValueType::U8);
        assert_eq!(specs[0].value, vec![7]);
        assert_eq!(specs[1].point_id, 2);
        assert_eq!(specs[1].domain, PointDomain::Unknown);
        assert_eq!(specs[1].value_type, ValueType::F32);
        assert_eq!(specs[1].value, 1.5f32.to_be_bytes().to_vec());
    }

    #[test]
    fn data_points_to_frame_dict_entry_specs() {
        let points = DataPoints(vec![
            DataPoint {
                id: 1,
                name: "p1",
                value: Val::U8(7),
            },
            DataPoint {
                id: 2,
                name: "p2",
                value: Val::I32(-1),
            },
        ]);
        let entries = points.to_frame_dict_entry_specs();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].point_id, 1);
        assert_eq!(entries[0].name, b"p1".to_vec());
        assert_eq!(entries[0].unit, b"".to_vec());
        assert_eq!(entries[0].value_type, ValueType::U8);
        assert_eq!(entries[1].point_id, 2);
        assert_eq!(entries[1].name, b"p2".to_vec());
        assert_eq!(entries[1].value_type, ValueType::I32);
    }
}
