use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// EMU 相关路由
pub(crate) fn router() -> Router {
    Router::with_path("emu").push(
        Router::with_path("soc_protect")
            .get(handlers::emu::soc_protect)
            .push(
                Router::new()
                    .hoop(require_role(Role::Admin))
                    .post(handlers::emu::set_soc_protect),
            ),
    )
}
