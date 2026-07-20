use salvo::Router;

use crate::{handlers, middleware::auth::auth_handler};

pub(crate) fn router() -> Router {
    Router::with_path("planned_curve")
        .get(handlers::planned_curve::find_master_by_id)
        .push(Router::with_path("list").get(handlers::planned_curve::list))
        .push(
            Router::new()
                .hoop(auth_handler())
                .post(handlers::planned_curve::create_planned_curve_master),
        )
        .push(
            Router::with_path("details")
                .get(handlers::planned_curve::planned_curve_details)
                .push(
                    Router::new()
                        .hoop(auth_handler())
                        .post(handlers::planned_curve::bind_planned_curve_details),
                ),
        )
}
