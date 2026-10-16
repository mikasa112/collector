use std::{fmt, str::FromStr};

use crate::client::{Error, Fc};

/// 数据对象/数据属性的引用, 同时接受两种写法并归一化:
///
/// - 点分式: `LD/LN.DO.DA`, 可带 `[FC]` 后缀, 如 `LD0/MMXU1.TotW.mag.f[MX]`
/// - MMS 式: `LD/LN$FC$DO$DA`, 报告和数据集成员用的就是这种
///
/// `ld` 保持原样(可能是 `MONT` 这样的后缀, 也可能是完整的 `PCS0001MONT`),
/// 与真实 LD 名的对应见 `Connection::resolve_ref`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub ld: String,
    pub ln: String,
    /// LN 之后的路径, 如 `["TotW", "mag", "f"]`; 数据集引用等不带路径的为空。
    pub path: Vec<String>,
    pub fc: Option<Fc>,
}

fn bad(s: &str, why: &str) -> Error {
    Error::Reference(format!("{s:?}: {why}"))
}

impl FromStr for ObjectRef {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let s = input.trim();
        let (ld, rest) = s.split_once('/').ok_or_else(|| bad(s, "缺少 LD 与 LN 之间的 '/'"))?;
        if ld.is_empty() {
            return Err(bad(s, "LD 为空"));
        }

        // 可选的 `[FC]` 后缀
        let (rest, bracket_fc) = match rest.strip_suffix(']') {
            Some(r) => {
                let (body, fc) = r.rsplit_once('[').ok_or_else(|| bad(s, "FC 方括号不匹配"))?;
                (body, Some(fc.parse::<Fc>().map_err(|e| bad(s, &e))?))
            }
            None => (rest, None),
        };

        if rest.contains(['[', ']']) {
            return Err(bad(s, "方括号只能作为末尾的 [FC] 后缀"));
        }

        let (ln, path, fc) = if rest.contains('$') {
            let mut parts = rest.split('$');
            let ln = parts.next().unwrap_or_default();
            let fc = parts
                .next()
                .ok_or_else(|| bad(s, "MMS 式引用缺少 FC"))?
                .parse::<Fc>()
                .map_err(|e| bad(s, &e))?;
            (ln, parts.map(str::to_string).collect::<Vec<_>>(), Some(fc))
        } else {
            let mut parts = rest.split('.');
            let ln = parts.next().unwrap_or_default();
            (ln, parts.map(str::to_string).collect(), bracket_fc)
        };

        if ln.is_empty() {
            return Err(bad(s, "LN 为空"));
        }
        if path.iter().any(String::is_empty) {
            return Err(bad(s, "路径中有空段"));
        }
        Ok(ObjectRef {
            ld: ld.to_string(),
            ln: ln.to_string(),
            path,
            fc,
        })
    }
}

impl ObjectRef {
    /// 点分式, 不含 FC: `LD/LN.DO.DA`。浏览和读写接口用这种。
    pub fn to_dotted(&self) -> String {
        let mut s = format!("{}/{}", self.ld, self.ln);
        for p in &self.path {
            s.push('.');
            s.push_str(p);
        }
        s
    }

    /// MMS 式 `LD/LN$FC$DO$DA`; 没有 FC 时返回 `None`。
    pub fn to_mms(&self) -> Option<String> {
        let fc = self.fc?;
        let mut s = format!("{}/{}${}", self.ld, self.ln, fc.as_str());
        for p in &self.path {
            s.push('$');
            s.push_str(p);
        }
        Some(s)
    }

    pub fn with_fc(mut self, fc: Fc) -> Self {
        self.fc = Some(fc);
        self
    }

    pub fn with_ld(mut self, ld: impl Into<String>) -> Self {
        self.ld = ld.into();
        self
    }

    /// 在末尾追加点分的相对路径, 空串返回自身的拷贝。
    pub fn child(&self, rel: &str) -> ObjectRef {
        let mut out = self.clone();
        out.path
            .extend(rel.split('.').filter(|p| !p.is_empty()).map(str::to_string));
        out
    }

    /// `self` 是否等于 `other` 或是它的祖先(忽略 FC)。
    pub fn covers(&self, other: &ObjectRef) -> bool {
        self.ld == other.ld
            && self.ln == other.ln
            && other.path.len() >= self.path.len()
            && other.path[..self.path.len()] == self.path[..]
    }
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_dotted())?;
        if let Some(fc) = self.fc {
            write!(f, "[{}]", fc.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ObjectRef {
        s.parse().unwrap()
    }

    #[test]
    fn parses_dotted_and_mms_to_same_ref() {
        let a = p("PCS0001MONT/MMXU1.TotW.mag.f[MX]");
        let b = p("PCS0001MONT/MMXU1$MX$TotW$mag$f");
        assert_eq!(a, b);
        assert_eq!(a.to_dotted(), "PCS0001MONT/MMXU1.TotW.mag.f");
        assert_eq!(a.to_mms().unwrap(), "PCS0001MONT/MMXU1$MX$TotW$mag$f");
        assert_eq!(a.to_string(), "PCS0001MONT/MMXU1.TotW.mag.f[MX]");
    }

    #[test]
    fn keeps_ln_prefix_and_short_ld() {
        let r = p("MONT/staGGIO1.Ind1.stVal");
        assert_eq!((r.ld.as_str(), r.ln.as_str()), ("MONT", "staGGIO1"));
        assert_eq!(r.fc, None);
        assert_eq!(r.path, ["Ind1", "stVal"]);
    }

    #[test]
    fn control_object_without_path_tail() {
        let r = p("LD0/CSWI1.Pos");
        assert_eq!(r.path, ["Pos"]);
        assert!(r.to_mms().is_none());
    }

    #[test]
    fn rejects_malformed() {
        for s in ["noslash", "/LN.DO", "LD/", "LD/LN..DO", "LD/LN.DO[XX]", "LD/LN$XX$DO", "LD/LN.DO[MX"] {
            assert!(s.parse::<ObjectRef>().is_err(), "{s} 应当解析失败");
        }
    }

    #[test]
    fn child_and_covers() {
        let member = p("LD/LN$ST$Ind1");
        let leaf = member.child("stVal");
        assert_eq!(leaf, p("LD/LN.Ind1.stVal[ST]"));
        assert!(member.covers(&leaf));
        assert!(!leaf.covers(&member));
        assert_eq!(member.child(""), member);
    }
}
