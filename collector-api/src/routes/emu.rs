use salvo::Router;

use crate::{handlers, middleware::auth::auth_handler};

/// EMU 相关路由
pub(crate) fn router() -> Router {
    Router::with_path("emu").push(
        Router::with_path("soc_protect")
            .get(handlers::emu::soc_protect)
            .push(
                Router::new()
                    .hoop(auth_handler())
                    .post(handlers::emu::set_soc_protect),
            ),
    )
}
