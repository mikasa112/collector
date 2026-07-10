use salvo::Request;
use serde::Deserialize;

pub(crate) mod data;
#[cfg(target_os = "linux")]
pub(crate) mod network;
pub(crate) mod planned_curve;
pub(crate) mod user;
pub(crate) mod ws;

pub(crate) struct RequestExtensions<'a>(&'a Request);

impl<'a> RequestExtensions<'a> {
    #[allow(dead_code)]
    fn parse_path_parameter<T>(&self, t: &str) -> Option<T>
    where
        T: Deserialize<'a>,
    {
        self.0.param(t)
    }

    fn parse_reqeust_parameter<T>(&self, t: &str) -> Option<T>
    where
        T: Deserialize<'a>,
    {
        self.0.query(t)
    }
}
