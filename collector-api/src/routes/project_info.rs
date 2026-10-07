use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 项目信息接口：读取（含图标）对未登录用户开放，登录页需要展示标题和图标；修改要求 Admin
pub(crate) fn router() -> Router {
    Router::with_path("project")
        .push(
            Router::with_path("info")
                .get(handlers::project_info::get_info)
                .push(
                    Router::new()
                        .hoop(require_role(Role::Admin))
                        .put(handlers::project_info::update_info),
                ),
        )
        .push(
            Router::with_path("logo")
                .get(handlers::project_info::get_logo)
                .push(
                    Router::new()
                        .hoop(require_role(Role::Admin))
                        .post(handlers::project_info::upload_logo)
                        .delete(handlers::project_info::delete_logo),
                ),
        )
}
