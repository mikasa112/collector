use salvo::Router;

use crate::handlers;

pub(crate) fn router() -> Router {
    Router::with_path("planned_curve")
        .get(handlers::planned_curve::find_master_by_id)
        .push(Router::with_path("list").get(handlers::planned_curve::list))
}
