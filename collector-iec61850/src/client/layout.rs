use crate::client::{ObjectRef, TypeSpec, Value};

struct Member {
    reference: ObjectRef,
    spec: TypeSpec,
}

/// 数据集的"布局": 每个成员的引用和类型描述。
///
/// 报告里的成员只有位置、没有字段名, 要把 DO 级成员(如整个 `Ind1` 的 `{stVal, q, t}`)
/// 对应到叶子数据属性, 必须事先查好类型描述。**查询是同步的 MMS 请求, 不能在报告回调里做**
/// (回调跑在库的接收线程上, 会死锁), 所以在订阅之前用 `Connection::prepare_data_set` 准备好,
/// 回调里只用 `leaves` 做纯内存展开。
pub struct DataSetLayout {
    data_set: String,
    members: Vec<Member>,
}

impl DataSetLayout {
    pub(crate) fn new(data_set: String, members: Vec<(ObjectRef, TypeSpec)>) -> Self {
        Self {
            data_set,
            members: members
                .into_iter()
                .map(|(reference, spec)| Member { reference, spec })
                .collect(),
        }
    }

    pub fn data_set(&self) -> &str {
        &self.data_set
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// 第 `index` 个成员的引用(带 FC)。
    pub fn member(&self, index: usize) -> Option<&ObjectRef> {
        self.members.get(index).map(|m| &m.reference)
    }

    /// 全部成员引用, 按数据集顺序。
    pub fn members(&self) -> impl Iterator<Item = &ObjectRef> {
        self.members.iter().map(|m| &m.reference)
    }

    /// 把第 `index` 个成员的值展开成叶子数据属性: `(带 FC 的完整叶子引用, 值)`。
    /// 成员本身就是叶子时, 返回成员自己; 值为 `Null`(报告里未包含)时返回空。
    pub fn leaves(&self, index: usize, value: &Value) -> Vec<(ObjectRef, Value)> {
        let Some(m) = self.members.get(index) else {
            return Vec::new();
        };
        m.spec
            .leaves(value)
            .into_iter()
            .map(|(rel, v)| (m.reference.child(&rel), v))
            .collect()
    }
}
