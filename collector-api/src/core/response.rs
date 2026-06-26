use salvo::{Depot, Request, Response, Writer, async_trait, writing::Json};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ObjResponse<T>
where
    T: Serialize,
{
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ObjResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            msg: Some("OK".to_string()),
            status: 200,
            data: Some(data),
        }
    }
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct ListResponse<T>
where
    T: Serialize,
{
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_msg: Option<String>,
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
}

impl<T: Serialize> ListResponse<T> {
    pub fn ok(data: Vec<T>) -> Self {
        let len = data.len();
        Self {
            err_msg: None,
            status: 200,
            data: Some(data),
            total: Some(len),
        }
    }
}

#[async_trait]
impl<T> Writer for ObjResponse<T>
where
    T: Serialize + Send,
{
    async fn write(self, _req: &mut Request, _depot: &mut Depot, res: &mut Response) {
        res.render(Json(self))
    }
}

#[async_trait]
impl<T> Writer for ListResponse<T>
where
    T: Serialize + Send,
{
    async fn write(self, _req: &mut Request, _depot: &mut Depot, res: &mut Response) {
        res.render(Json(self))
    }
}
